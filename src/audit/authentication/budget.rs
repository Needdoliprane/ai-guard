//! Finite, non-borrowing logical partitions of the existing private ledger.
//! A charge is accounting provenance, never tenant authorization.
use super::{format, *};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TENANTS: usize = 256;
pub(super) const TERMINAL_BYTES: u64 = 32_768;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationCapacityV1 {
    pub max_records: u64,
    pub max_serialized_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationTenantCapacityV1 {
    pub tenant_id: String,
    pub capacity: AuthenticationCapacityV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationCapacityPlanV1 {
    pub version: u32,
    pub unscoped: AuthenticationCapacityV1,
    pub tenants: Vec<AuthenticationTenantCapacityV1>,
}

fn valid_tenant(tenant: &str) -> Result<()> {
    crate::tenancy::validate_tenant_id(tenant)
        .map_err(|_| AuthenticationAuditFailureV1::InvalidInput.into())
}

impl AuthenticationCapacityPlanV1 {
    /// Validate the closed logical plan, including its complete V4 payload
    /// bound. This performs no storage access and grants no authority.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && !self.tenants.is_empty() && self.tenants.len() <= MAX_TENANTS,
            AuthenticationAuditFailureV1::InvalidInput
        );
        let mut tenants = BTreeSet::new();
        for tenant in &self.tenants {
            valid_tenant(&tenant.tenant_id)?;
            ensure!(
                tenants.insert(&tenant.tenant_id),
                AuthenticationAuditFailureV1::InvalidInput
            );
        }
        for capacity in
            std::iter::once(&self.unscoped).chain(self.tenants.iter().map(|t| &t.capacity))
        {
            ensure!(
                capacity.max_records <= i64::MAX as u64
                    && capacity.max_serialized_bytes <= i64::MAX as u64,
                AuthenticationAuditFailureV1::InvalidInput
            );
        }
        format::checked_json(
            &format::Entry::CapacityPlan { plan: self.clone() },
            &Redactor::new(vec![]),
        )?;
        Ok(())
    }
}

/// Only the Rust gateway constructs this after verified, bounded membership
/// reads. It contains no request-selected tenant or authorization capability.
pub(crate) struct AuthenticationTenantCandidatesV1 {
    principal: PrincipalRefV1,
    tenant_ids: BTreeSet<String>,
}
impl AuthenticationTenantCandidatesV1 {
    pub(crate) fn from_verified_memberships(
        principal: PrincipalRefV1,
        tenant_ids: Vec<String>,
    ) -> Result<Self> {
        principal
            .validate()
            .map_err(|_| AuthenticationAuditFailureV1::InvalidInput)?;
        ensure!(
            tenant_ids.len() <= MAX_TENANTS
                && matches!(principal, PrincipalRefV1::HumanOidc { .. }),
            AuthenticationAuditFailureV1::InvalidInput
        );
        for tenant in &tenant_ids {
            valid_tenant(tenant)?;
        }
        Ok(Self {
            principal,
            tenant_ids: tenant_ids.into_iter().collect(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CapacityOwner {
    /// Selected by the control from a verified membership snapshot.
    VerifiedMembership { tenant_id: String },
    /// No positive membership snapshot; this partition can never admit.
    Unscoped,
}

#[derive(Clone)]
struct Ticket {
    owner: Option<CapacityOwner>, // None is strictly pre-plan historical debt.
    intention: AuthenticationReceiptV1,
    session_id: Option<Uuid>,
    // Reconstructed at the plan boundary, never invented tenant attribution.
    historical_binding: Option<AuthenticationSessionBindingV1>,
}

#[derive(Default)]
pub(super) struct BudgetReplay {
    pub plan: Option<AuthenticationCapacityPlanV1>,
    spent: BTreeMap<CapacityOwner, (u64, u64)>,
    tickets: BTreeMap<Uuid, Ticket>,
    sessions: BTreeMap<Uuid, CapacityOwner>,
}

fn plus(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| AuthenticationAuditFailureV1::Capacity.into())
}

impl BudgetReplay {
    pub fn install<'a>(
        &mut self,
        plan: AuthenticationCapacityPlanV1,
        historical: impl Iterator<
            Item = (
                &'a AuthenticationRecordV1,
                &'a AuthenticationReceiptV1,
                Option<&'a AuthenticationSessionBindingV1>,
            ),
        >,
        records: u64,
        bytes: u64,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        ensure!(self.plan.is_none(), AuthenticationAuditFailureV1::Format);
        plan.validate()?;
        let mut tickets = BTreeMap::new();
        for (record, receipt, binding) in historical {
            tickets.insert(
                record.operation_id,
                Ticket {
                    owner: None,
                    intention: receipt.clone(),
                    session_id: record.session_id,
                    historical_binding: binding.cloned(),
                },
            );
        }
        let reserved =
            u64::try_from(tickets.len()).map_err(|_| AuthenticationAuditFailureV1::Capacity)?;
        let mut total_records = plus(records, reserved)?;
        let mut total_bytes = plus(
            bytes,
            reserved
                .checked_mul(TERMINAL_BYTES)
                .ok_or(AuthenticationAuditFailureV1::Capacity)?,
        )?;
        for capacity in
            std::iter::once(&plan.unscoped).chain(plan.tenants.iter().map(|t| &t.capacity))
        {
            total_records = plus(total_records, capacity.max_records)?;
            total_bytes = plus(total_bytes, capacity.max_serialized_bytes)?;
        }
        ensure!(
            total_records <= policy.max_records && total_bytes <= policy.max_serialized_bytes,
            AuthenticationAuditFailureV1::Capacity
        );
        self.tickets = tickets;
        self.plan = Some(plan);
        Ok(())
    }

    fn capacity(&self, owner: &CapacityOwner) -> Result<&AuthenticationCapacityV1> {
        let plan = self
            .plan
            .as_ref()
            .ok_or(AuthenticationAuditFailureV1::Format)?;
        match owner {
            CapacityOwner::Unscoped => Ok(&plan.unscoped),
            CapacityOwner::VerifiedMembership { tenant_id } => plan
                .tenants
                .iter()
                .find(|t| t.tenant_id == *tenant_id)
                .map(|t| &t.capacity)
                .ok_or_else(|| AuthenticationAuditFailureV1::Format.into()),
        }
    }

    fn check_charge(
        &self,
        owner: &CapacityOwner,
        record: &AuthenticationRecordV1,
        bytes: u64,
    ) -> Result<()> {
        let capacity = self.capacity(owner)?;
        let used = self.spent.get(owner).copied().unwrap_or_default();
        let reserved = self
            .tickets
            .values()
            .filter(|ticket| ticket.owner.as_ref() == Some(owner))
            .count() as u64;
        let remaining = match record.phase {
            AuthenticationPhaseV1::Intention => plus(reserved, 1)?,
            AuthenticationPhaseV1::Result { .. } => {
                let ticket = self
                    .tickets
                    .get(&record.operation_id)
                    .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
                ensure!(
                    ticket.owner.as_ref() == Some(owner)
                        && record.causal.as_ref() == Some(&ticket.intention)
                        && record.session_id == ticket.session_id,
                    AuthenticationAuditFailureV1::PhaseOrder
                );
                reserved
                    .checked_sub(1)
                    .ok_or(AuthenticationAuditFailureV1::Integrity)?
            }
        };
        ensure!(
            plus(plus(used.0, 1)?, remaining)? <= capacity.max_records
                && plus(
                    plus(used.1, bytes)?,
                    remaining
                        .checked_mul(TERMINAL_BYTES)
                        .ok_or(AuthenticationAuditFailureV1::Capacity)?
                )? <= capacity.max_serialized_bytes,
            AuthenticationAuditFailureV1::Capacity
        );
        Ok(())
    }

    /// An authority fact spends only existing unscoped capacity. Outstanding
    /// unscoped terminal promises stay reserved and no new ticket is created.
    pub fn validate_authority(&self, owner: Option<&CapacityOwner>, bytes: u64) -> Result<()> {
        ensure!(
            owner == Some(&CapacityOwner::Unscoped),
            AuthenticationAuditFailureV1::Format
        );
        let capacity = self.capacity(&CapacityOwner::Unscoped)?;
        let used = self
            .spent
            .get(&CapacityOwner::Unscoped)
            .copied()
            .unwrap_or_default();
        let reserved = self
            .tickets
            .values()
            .filter(|t| t.owner.as_ref() == Some(&CapacityOwner::Unscoped))
            .count() as u64;
        ensure!(
            plus(plus(used.0, 1)?, reserved)? <= capacity.max_records
                && plus(
                    plus(used.1, bytes)?,
                    reserved
                        .checked_mul(TERMINAL_BYTES)
                        .ok_or(AuthenticationAuditFailureV1::Capacity)?
                )? <= capacity.max_serialized_bytes,
            AuthenticationAuditFailureV1::Capacity
        );
        Ok(())
    }
    pub fn accept_authority(&mut self, owner: Option<&CapacityOwner>, bytes: u64) -> Result<()> {
        self.validate_authority(owner, bytes)?;
        let used = self.spent.entry(CapacityOwner::Unscoped).or_default();
        used.0 = plus(used.0, 1)?;
        used.1 = plus(used.1, bytes)?;
        Ok(())
    }

    pub fn choose_initial(
        &self,
        entry: &format::Entry,
        candidates: &AuthenticationTenantCandidatesV1,
        redactor: &Redactor,
    ) -> Result<CapacityOwner> {
        let format::Entry::VerifiedInitial {
            record, principal, ..
        } = entry
        else {
            return Err(AuthenticationAuditFailureV1::InvalidInput.into());
        };
        ensure!(
            *principal == candidates.principal,
            AuthenticationAuditFailureV1::InvalidInput
        );
        let owners = if candidates.tenant_ids.is_empty() {
            vec![CapacityOwner::Unscoped]
        } else {
            candidates
                .tenant_ids
                .iter()
                .map(|tenant_id| CapacityOwner::VerifiedMembership {
                    tenant_id: tenant_id.clone(),
                })
                .collect()
        };
        for owner in owners {
            // The full wrapper is sized before allocating any terminal promise.
            let bytes = format::checked_json(
                &format::Entry::Charged {
                    charge: owner.clone(),
                    entry: Box::new(entry.clone()),
                },
                redactor,
            )?
            .len() as u64;
            match self.check_charge(&owner, record, bytes) {
                Ok(()) => return Ok(owner),
                Err(error)
                    if error.downcast_ref::<AuthenticationAuditFailureV1>()
                        == Some(&AuthenticationAuditFailureV1::Capacity) => {}
                Err(error) => return Err(error),
            }
        }
        Err(AuthenticationAuditFailureV1::Capacity.into())
    }

    pub fn owner(&self, entry: &format::Entry) -> Result<Option<CapacityOwner>> {
        let record = entry
            .record()
            .ok_or(AuthenticationAuditFailureV1::InvalidInput)?;
        match record.phase {
            AuthenticationPhaseV1::Result { .. } => Ok(self
                .tickets
                .get(&record.operation_id)
                .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?
                .owner
                .clone()),
            AuthenticationPhaseV1::Intention => {
                ensure!(
                    matches!(entry,
                        format::Entry::Authentication { record } if matches!(record.action, AuthenticationActionV1::Logout | AuthenticationActionV1::Expiration)
                    ) || matches!(entry,
                        format::Entry::Session { mutation } if matches!(mutation.as_ref(), format::SessionMutation::RefreshPending { .. } | format::SessionMutation::BeginNativeLogoutDuringRefresh { .. })
                    ),
                    AuthenticationAuditFailureV1::Format
                );
                Ok(Some(
                    self.sessions
                        .get(
                            &record
                                .session_id
                                .ok_or(AuthenticationAuditFailureV1::Format)?,
                        )
                        .ok_or(AuthenticationAuditFailureV1::Format)?
                        .clone(),
                ))
            }
        }
    }

    pub fn validate(
        &self,
        entry: &format::Entry,
        owner: Option<&CapacityOwner>,
        payload_bytes: u64,
    ) -> Result<()> {
        let record = entry
            .record()
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        match owner {
            Some(owner) => {
                match entry {
                    format::Entry::VerifiedInitial { .. } => ensure!(
                        record.phase == AuthenticationPhaseV1::Intention,
                        AuthenticationAuditFailureV1::PhaseOrder
                    ),
                    _ => ensure!(
                        self.owner(entry)?.as_ref() == Some(owner),
                        AuthenticationAuditFailureV1::PhaseOrder
                    ),
                }
                if matches!(owner, CapacityOwner::Unscoped) {
                    ensure!(
                        !matches!(entry, format::Entry::Session { mutation } if matches!(mutation.as_ref(), format::SessionMutation::Admit { .. })),
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                    ensure!(
                        !matches!(
                            record.phase,
                            AuthenticationPhaseV1::Result {
                                outcome: AuthenticationOutcomeV1::Succeeded,
                                ..
                            }
                        ),
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                }
                self.check_charge(owner, record, payload_bytes)?;
            }
            None => {
                // The only post-plan raw entry is an exact historical terminal.
                let ticket = self
                    .tickets
                    .get(&record.operation_id)
                    .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
                ensure!(
                    ticket.owner.is_none()
                        && matches!(record.phase, AuthenticationPhaseV1::Result { .. })
                        && record.causal.as_ref() == Some(&ticket.intention)
                        && record.session_id == ticket.session_id,
                    AuthenticationAuditFailureV1::PhaseOrder
                );
                if let Some(binding) = &ticket.historical_binding
                    && let format::Entry::Session { mutation } = entry
                {
                    let matches_binding = match mutation.as_ref() {
                        format::SessionMutation::Rotate {
                            expected_generation,
                            expected_access_fingerprint,
                            ..
                        }
                        | format::SessionMutation::Close {
                            expected_generation,
                            expected_access_fingerprint,
                            ..
                        } => {
                            *expected_generation == binding.generation
                                && *expected_access_fingerprint == binding.access_fingerprint
                        }
                        format::SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                            expected_binding,
                            ..
                        } => expected_binding == binding,
                        _ => false,
                    };
                    ensure!(matches_binding, AuthenticationAuditFailureV1::PhaseOrder);
                }
            }
        }
        Ok(())
    }

    pub fn accept(
        &mut self,
        entry: &format::Entry,
        owner: Option<&CapacityOwner>,
        payload_bytes: u64,
        receipt: &AuthenticationReceiptV1,
    ) -> Result<()> {
        self.validate(entry, owner, payload_bytes)?;
        let record = entry
            .record()
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        if let Some(owner) = owner {
            let used = self.spent.entry(owner.clone()).or_default();
            used.0 = plus(used.0, 1)?;
            used.1 = plus(used.1, payload_bytes)?;
            if let format::Entry::Session { mutation } = entry
                && let format::SessionMutation::Admit { binding, .. } = mutation.as_ref()
            {
                self.sessions.insert(binding.session_id, owner.clone());
            }
        }
        match record.phase {
            AuthenticationPhaseV1::Intention => {
                ensure!(
                    self.tickets
                        .insert(
                            record.operation_id,
                            Ticket {
                                owner: owner.cloned(),
                                intention: receipt.clone(),
                                session_id: record.session_id,
                                historical_binding: None
                            }
                        )
                        .is_none(),
                    AuthenticationAuditFailureV1::Integrity
                );
            }
            AuthenticationPhaseV1::Result { .. } => {
                self.tickets
                    .remove(&record.operation_id)
                    .ok_or(AuthenticationAuditFailureV1::Integrity)?;
            }
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Barrier},
        thread,
    };

    const ISSUER: &str = "https://identity.example.test/realm";
    const CLIENT: &str = "public-client";

    fn cap(records: u64, bytes: u64) -> AuthenticationCapacityV1 {
        AuthenticationCapacityV1 {
            max_records: records,
            max_serialized_bytes: bytes,
        }
    }
    fn plan(
        a: AuthenticationCapacityV1,
        b: AuthenticationCapacityV1,
    ) -> AuthenticationCapacityPlanV1 {
        AuthenticationCapacityPlanV1 {
            version: 1,
            unscoped: cap(4, 70_000),
            tenants: vec![
                AuthenticationTenantCapacityV1 {
                    tenant_id: "tenant-a".into(),
                    capacity: a,
                },
                AuthenticationTenantCapacityV1 {
                    tenant_id: "tenant-b".into(),
                    capacity: b,
                },
            ],
        }
    }
    fn policy() -> AuthenticationAuditPolicyV1 {
        AuthenticationAuditPolicyV1 {
            max_records: 64,
            max_serialized_bytes: 1_000_000,
            approved_clients: vec![ApprovedAuthenticationClientV1 {
                issuer: ISSUER.into(),
                client_id: CLIENT.into(),
            }],
        }
    }
    fn private_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    fn binding(subject: &str) -> AuthenticationSessionBindingV1 {
        AuthenticationSessionBindingV1 {
            session_id: Uuid::new_v4(),
            generation: 1,
            principal: PrincipalRefV1::HumanOidc {
                issuer: ISSUER.into(),
                subject: subject.into(),
            },
            issuer: ISSUER.into(),
            client_id: CLIENT.into(),
            kind: OidcSessionKindV1::NativeCli,
            access_fingerprint: AuthenticationTokenFingerprintV1::access_token(&format!(
                "synthetic-access-{subject}"
            )),
            refresh_fingerprint: Some(AuthenticationTokenFingerprintV1::refresh_token(&format!(
                "synthetic-refresh-{subject}"
            ))),
            expires_at_unix_seconds: 4_000_000_000,
        }
    }
    fn intention(
        binding: &AuthenticationSessionBindingV1,
        action: AuthenticationActionV1,
    ) -> AuthenticationRecordV1 {
        AuthenticationRecordV1 {
            operation_id: Uuid::new_v4(),
            action,
            phase: AuthenticationPhaseV1::Intention,
            issuer: binding.issuer.clone(),
            client_id: binding.client_id.clone(),
            principal: Some(binding.principal.clone()),
            transaction_id: None,
            session_id: Some(binding.session_id),
            causal: None,
        }
    }
    fn initial(binding: &AuthenticationSessionBindingV1) -> AuthenticationRecordV1 {
        AuthenticationRecordV1 {
            principal: None,
            ..intention(binding, AuthenticationActionV1::SessionAdmission)
        }
    }
    fn terminal(
        record: &AuthenticationRecordV1,
        receipt: AuthenticationReceiptV1,
        principal: PrincipalRefV1,
        outcome: AuthenticationOutcomeV1,
        reason: AuthenticationReasonV1,
    ) -> AuthenticationRecordV1 {
        AuthenticationRecordV1 {
            phase: AuthenticationPhaseV1::Result { outcome, reason },
            principal: Some(principal),
            causal: Some(receipt),
            ..record.clone()
        }
    }
    fn candidates(
        binding: &AuthenticationSessionBindingV1,
        tenants: &[&str],
    ) -> AuthenticationTenantCandidatesV1 {
        AuthenticationTenantCandidatesV1::from_verified_memberships(
            binding.principal.clone(),
            tenants.iter().map(|t| (*t).into()).collect(),
        )
        .unwrap()
    }
    fn admit(
        audit: &AuthenticationAudit,
        binding: &AuthenticationSessionBindingV1,
        tenant: &str,
    ) -> (AuthenticationRecordV1, AuthenticationReceiptV1) {
        let record = initial(binding);
        let pending = audit
            .begin_verified_initial_scoped(
                record.clone(),
                binding.principal.clone(),
                binding.access_fingerprint,
                candidates(binding, &[tenant]),
            )
            .unwrap();
        assert!(pending.inserted);
        audit
            .admit_session(
                terminal(
                    &record,
                    pending.receipt.clone(),
                    binding.principal.clone(),
                    AuthenticationOutcomeV1::Succeeded,
                    AuthenticationReasonV1::Verified,
                ),
                binding.clone(),
            )
            .unwrap();
        (record, pending.receipt)
    }
    fn failure(error: anyhow::Error, expected: AuthenticationAuditFailureV1) {
        assert_eq!(
            error.downcast_ref::<AuthenticationAuditFailureV1>(),
            Some(&expected)
        );
    }

    #[test]
    fn record_partitions_preserve_terminal_and_replay_across_two_handles() {
        let root = private_dir();
        let path = root.path().join("auth.sqlite");
        let installation = Uuid::new_v4();
        let policy = policy();
        let audit = AuthenticationAudit::init_new_v4(
            &path,
            installation,
            policy.clone(),
            plan(cap(4, 100_000), cap(4, 100_000)),
            Redactor::new(vec![]),
        )
        .unwrap();
        let other = AuthenticationAudit::open_existing(
            &path,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        let a = binding("a");
        let (initial_a, original) = admit(&audit, &a, "tenant-a");
        let refresh = intention(&a, AuthenticationActionV1::Refresh);
        let pending = audit
            .refresh_pending(refresh.clone(), 1, a.access_fingerprint)
            .unwrap();
        let before_refusal = audit.verify().unwrap();
        let another_a = binding("a-next");
        failure(
            other
                .begin_verified_initial_scoped(
                    initial(&another_a),
                    another_a.principal.clone(),
                    another_a.access_fingerprint,
                    candidates(&another_a, &["tenant-a"]),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::Capacity,
        );
        assert_eq!(audit.verify().unwrap(), before_refusal);
        let b = binding("b");
        admit(&other, &b, "tenant-b");
        // Empty/changed candidates are ignored on exact replay; the original
        // principal and access proof still form part of the compared content.
        let replay = other
            .begin_verified_initial_scoped(
                initial_a.clone(),
                a.principal.clone(),
                a.access_fingerprint,
                candidates(&a, &[]),
            )
            .unwrap();
        assert!(!replay.inserted);
        assert_eq!(replay.receipt, original);
        failure(
            other
                .begin_verified_initial_scoped(
                    initial_a,
                    b.principal.clone(),
                    b.access_fingerprint,
                    candidates(&b, &["tenant-b"]),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::Conflict,
        );
        let closed = terminal(
            &refresh,
            pending.receipt,
            a.principal.clone(),
            AuthenticationOutcomeV1::Unknown,
            AuthenticationReasonV1::Interrupted,
        );
        let receipt = other
            .close_session(closed.clone(), 1, a.access_fingerprint)
            .unwrap();
        assert_eq!(
            audit
                .close_session(closed, 1, a.access_fingerprint)
                .unwrap(),
            receipt
        );
        let verified = audit.verify().unwrap();
        drop(other);
        drop(audit);
        let reopened =
            AuthenticationAudit::open_existing(&path, installation, policy, Redactor::new(vec![]))
                .unwrap();
        assert_eq!(reopened.verify().unwrap(), verified);
        assert!(matches!(
            reopened
                .session_current(a.session_id)
                .unwrap()
                .unwrap()
                .state,
            AuthenticationSessionStateV1::Closed { .. }
        ));
    }

    #[test]
    fn competing_handles_cannot_overallocate_record_or_byte_partitions() {
        for a_capacity in [cap(2, 70_000), cap(20, 38_000)] {
            let root = private_dir();
            let path = root.path().join("race.sqlite");
            let installation = Uuid::new_v4();
            let policy = policy();
            let first = AuthenticationAudit::init_new_v4(
                &path,
                installation,
                policy.clone(),
                plan(a_capacity, cap(4, 100_000)),
                Redactor::new(vec![]),
            )
            .unwrap();
            let second = AuthenticationAudit::open_existing(
                &path,
                installation,
                policy.clone(),
                Redactor::new(vec![]),
            )
            .unwrap();
            let start = Arc::new(Barrier::new(3));
            let racers: Vec<_> = [first, second]
                .into_iter()
                .enumerate()
                .map(|(index, audit)| {
                    let start = start.clone();
                    thread::spawn(move || {
                        let binding = binding(&format!("racer-{index}"));
                        let record = initial(&binding);
                        start.wait();
                        let result = audit.begin_verified_initial_scoped(
                            record.clone(),
                            binding.principal.clone(),
                            binding.access_fingerprint,
                            candidates(&binding, &["tenant-a"]),
                        );
                        (binding, record, result)
                    })
                })
                .collect();
            start.wait();
            let results: Vec<_> = racers
                .into_iter()
                .map(|racer| racer.join().unwrap())
                .collect();
            assert_eq!(
                results
                    .iter()
                    .filter(|(_, _, result)| result.is_ok())
                    .count(),
                1
            );
            let audit = AuthenticationAudit::open_existing(
                &path,
                installation,
                policy,
                Redactor::new(vec![]),
            )
            .unwrap();
            assert_eq!(audit.verify().unwrap().records, 3);
            admit(&audit, &binding("unrelated-b"), "tenant-b");
            for (binding, record, result) in results {
                match result {
                    Ok(pending) => {
                        audit
                            .append_typed(terminal(
                                &record,
                                pending.receipt,
                                binding.principal,
                                AuthenticationOutcomeV1::Rejected,
                                AuthenticationReasonV1::NoMembership,
                            ))
                            .unwrap();
                    }
                    Err(error) => failure(error, AuthenticationAuditFailureV1::Capacity),
                }
            }
            assert_eq!(audit.verify().unwrap().records, 6);
        }
    }

    #[test]
    fn byte_partition_refusal_does_not_spend_b_and_multimembership_owner_stays_fixed() {
        let root = private_dir();
        let policy = policy();
        // Records are plentiful; A cannot fund even one terminal in bytes.
        let audit = AuthenticationAudit::init_new_v4(
            &root.path().join("auth.sqlite"),
            Uuid::new_v4(),
            policy,
            plan(cap(20, TERMINAL_BYTES), cap(20, 100_000)),
            Redactor::new(vec![]),
        )
        .unwrap();
        let a = binding("only-a");
        let before = audit.verify().unwrap();
        failure(
            audit
                .begin_verified_initial_scoped(
                    initial(&a),
                    a.principal.clone(),
                    a.access_fingerprint,
                    candidates(&a, &["tenant-a"]),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::Capacity,
        );
        assert_eq!(audit.verify().unwrap(), before);
        let ab = binding("both");
        let record = initial(&ab);
        let pending = audit
            .begin_verified_initial_scoped(
                record.clone(),
                ab.principal.clone(),
                ab.access_fingerprint,
                candidates(&ab, &["tenant-b", "tenant-a"]),
            )
            .unwrap();
        audit
            .admit_session(
                terminal(
                    &record,
                    pending.receipt.clone(),
                    ab.principal.clone(),
                    AuthenticationOutcomeV1::Succeeded,
                    AuthenticationReasonV1::Verified,
                ),
                ab.clone(),
            )
            .unwrap();
        let replay = audit
            .begin_verified_initial_scoped(
                record,
                ab.principal.clone(),
                ab.access_fingerprint,
                candidates(&ab, &["tenant-a"]),
            )
            .unwrap();
        assert_eq!(pending.receipt, replay.receipt);
        let refresh = intention(&ab, AuthenticationActionV1::Refresh);
        assert!(
            audit
                .refresh_pending(refresh, 1, ab.access_fingerprint)
                .unwrap()
                .inserted,
            "refresh retains the available B owner despite later A-only candidates"
        );
    }

    #[test]
    fn unscoped_cannot_admit_and_its_exhaustion_leaves_tenant_part_intact() {
        let root = private_dir();
        let mut plan = plan(cap(4, 100_000), cap(4, 100_000));
        plan.unscoped = cap(2, 70_000);
        let audit = AuthenticationAudit::init_new_v4(
            &root.path().join("auth.sqlite"),
            Uuid::new_v4(),
            policy(),
            plan,
            Redactor::new(vec![]),
        )
        .unwrap();
        let denied = binding("no-membership");
        let record = initial(&denied);
        let pending = audit
            .begin_verified_initial_scoped(
                record.clone(),
                denied.principal.clone(),
                denied.access_fingerprint,
                candidates(&denied, &[]),
            )
            .unwrap();
        failure(
            audit
                .admit_session(
                    terminal(
                        &record,
                        pending.receipt.clone(),
                        denied.principal.clone(),
                        AuthenticationOutcomeV1::Succeeded,
                        AuthenticationReasonV1::Verified,
                    ),
                    denied.clone(),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::PhaseOrder,
        );
        audit
            .append_typed(terminal(
                &record,
                pending.receipt,
                denied.principal.clone(),
                AuthenticationOutcomeV1::Rejected,
                AuthenticationReasonV1::NoMembership,
            ))
            .unwrap();
        let next = binding("another-unscoped");
        failure(
            audit
                .begin_verified_initial_scoped(
                    initial(&next),
                    next.principal.clone(),
                    next.access_fingerprint,
                    candidates(&next, &[]),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::Capacity,
        );
        admit(&audit, &binding("member-b"), "tenant-b");
    }

    #[test]
    fn migration_preserves_pending_refresh_terminal_without_a_logout() {
        let root = private_dir();
        let path = root.path().join("pending.sqlite");
        let installation = Uuid::new_v4();
        let policy = policy();
        let audit = AuthenticationAudit::init_new(
            &path,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        let a = binding("legacy-pending");
        let initial = initial(&a);
        let original = audit.append_typed(initial.clone()).unwrap();
        audit
            .admit_session(
                terminal(
                    &initial,
                    original.clone(),
                    a.principal.clone(),
                    AuthenticationOutcomeV1::Succeeded,
                    AuthenticationReasonV1::Verified,
                ),
                a.clone(),
            )
            .unwrap();
        let refresh = intention(&a, AuthenticationActionV1::Refresh);
        let pending = audit
            .refresh_pending(refresh.clone(), 1, a.access_fingerprint)
            .unwrap();
        audit
            .migrate_tenant_capacity_v4(
                &root.path().join("before-v4.sqlite"),
                plan(cap(4, 100_000), cap(4, 100_000)),
            )
            .unwrap();
        let result = terminal(
            &refresh,
            pending.receipt,
            a.principal.clone(),
            AuthenticationOutcomeV1::Unknown,
            AuthenticationReasonV1::Interrupted,
        );
        let receipt = audit
            .close_session(result.clone(), 1, a.access_fingerprint)
            .unwrap();
        admit(&audit, &binding("new-b"), "tenant-b");
        let verified = audit.verify().unwrap();
        let reopened =
            AuthenticationAudit::open_existing(&path, installation, policy, Redactor::new(vec![]))
                .unwrap();
        assert_eq!(
            reopened
                .close_session(result, 1, a.access_fingerprint)
                .unwrap(),
            receipt
        );
        assert_eq!(reopened.append_typed(initial).unwrap(), original);
        assert_eq!(reopened.verify().unwrap(), verified);
    }

    #[test]
    fn migration_preserves_exact_raw_logout_ticket_after_refresh_closed() {
        let root = private_dir();
        let path = root.path().join("auth.sqlite");
        let installation = Uuid::new_v4();
        let mut policy = policy();
        policy.max_records = 14; // 6 history + plan + ticket + 6 partition records.
        let audit = AuthenticationAudit::init_new(
            &path,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        let a = binding("historical");
        let record = initial(&a);
        let receipt = audit.append_typed(record.clone()).unwrap();
        audit
            .admit_session(
                terminal(
                    &record,
                    receipt.clone(),
                    a.principal.clone(),
                    AuthenticationOutcomeV1::Succeeded,
                    AuthenticationReasonV1::Verified,
                ),
                a.clone(),
            )
            .unwrap();
        let refresh = intention(&a, AuthenticationActionV1::Refresh);
        let pending = audit
            .refresh_pending(refresh.clone(), 1, a.access_fingerprint)
            .unwrap();
        let logout = intention(&a, AuthenticationActionV1::Logout);
        let ilogout = audit
            .begin_native_logout_during_refresh(
                logout.clone(),
                a.clone(),
                refresh.operation_id,
                pending.receipt.clone(),
            )
            .unwrap();
        let refresh_result = audit
            .close_session(
                terminal(
                    &refresh,
                    pending.receipt.clone(),
                    a.principal.clone(),
                    AuthenticationOutcomeV1::Unknown,
                    AuthenticationReasonV1::Interrupted,
                ),
                1,
                a.access_fingerprint,
            )
            .unwrap();
        let before = audit.verify().unwrap();
        assert_eq!(before.records, 6);
        let old_lane = AuthenticationAudit::open_existing(
            &path,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        let mut plan = plan(cap(2, 70_000), cap(2, 70_000));
        plan.unscoped.max_records = 2;
        // Size M explicitly, then assign the entire remaining byte capacity to
        // unscoped. Iteration accounts for the width of that field in M itself.
        for _ in 0..10 {
            let m = format::checked_json(
                &format::Entry::CapacityPlan { plan: plan.clone() },
                &Redactor::new(vec![]),
            )
            .unwrap()
            .len() as u64;
            let exact = policy.max_serialized_bytes
                - before.serialized_bytes
                - TERMINAL_BYTES
                - 140_000
                - m;
            if exact == plan.unscoped.max_serialized_bytes {
                break;
            }
            plan.unscoped.max_serialized_bytes = exact;
        }
        let backup = root.path().join("before-v4.sqlite");
        let migrated = audit
            .migrate_tenant_capacity_v4(&backup, plan.clone())
            .unwrap();
        assert_eq!(migrated.records, before.records + 1);
        let saved = AuthenticationAudit::open_existing(
            &backup,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        assert_eq!(saved.format_version().unwrap(), 3);
        assert_eq!(saved.verify().unwrap(), before);
        assert_eq!(saved.append_typed(record.clone()).unwrap(), receipt);
        let end = terminal(
            &logout,
            ilogout,
            a.principal.clone(),
            AuthenticationOutcomeV1::Succeeded,
            AuthenticationReasonV1::UserRequested,
        );
        let result = audit
            .record_native_logout_after_refresh_closed(
                end.clone(),
                a.clone(),
                refresh.operation_id,
                pending.receipt.clone(),
                refresh_result.clone(),
            )
            .unwrap();
        assert_eq!(
            audit
                .record_native_logout_after_refresh_closed(
                    end.clone(),
                    a.clone(),
                    refresh.operation_id,
                    pending.receipt.clone(),
                    refresh_result.clone()
                )
                .unwrap(),
            result
        );
        let verified = audit.verify().unwrap();
        let fresh = binding("preopened-uncharged-writer");
        failure(
            old_lane
                .begin_verified_initial(
                    initial(&fresh),
                    fresh.principal.clone(),
                    fresh.access_fingerprint,
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::Format,
        );
        assert_eq!(old_lane.verify().unwrap(), verified);
        let reopened = AuthenticationAudit::open_existing(
            &path,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        assert_eq!(reopened.verify().unwrap(), verified);
        assert_eq!(
            reopened
                .record_native_logout_after_refresh_closed(
                    end,
                    a.clone(),
                    refresh.operation_id,
                    pending.receipt,
                    refresh_result
                )
                .unwrap(),
            result
        );
        assert_eq!(reopened.append_typed(record).unwrap(), receipt);
        assert_eq!(reopened.capacity_plan().unwrap(), Some(plan));
        failure(
            reopened
                .append_typed(intention(&a, AuthenticationActionV1::Expiration))
                .unwrap_err(),
            AuthenticationAuditFailureV1::Format,
        );
        assert_eq!(reopened.verify().unwrap(), verified);
        assert!(
            audit
                .migrate_tenant_capacity_v4(&backup, self::plan(cap(0, 0), cap(0, 0)))
                .is_err()
        );
        assert_eq!(saved.verify().unwrap(), before);
    }

    #[test]
    fn format_plan_scope_and_unfunded_legacy_migration_are_refused() {
        let root = private_dir();
        let path = root.path().join("auth.sqlite");
        let mut policy = policy();
        policy.max_records = 4;
        let audit =
            AuthenticationAudit::init_new(&path, Uuid::new_v4(), policy, Redactor::new(vec![]))
                .unwrap();
        failure(
            audit
                .validate_capacity_tenants(&BTreeSet::from(["tenant-a".into(), "tenant-b".into()]))
                .unwrap_err(),
            AuthenticationAuditFailureV1::Format,
        );
        let before = audit.verify().unwrap();
        failure(
            audit
                .migrate_tenant_capacity_v4(
                    &root.path().join("backup.sqlite"),
                    plan(cap(2, 70_000), cap(2, 70_000)),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::Capacity,
        );
        assert_eq!(audit.format_version().unwrap(), 3);
        assert_eq!(audit.verify().unwrap(), before);
        let path4 = root.path().join("auth4.sqlite");
        let audit4 = AuthenticationAudit::init_new_v4(
            &path4,
            Uuid::new_v4(),
            self::policy(),
            plan(cap(2, 70_000), cap(2, 70_000)),
            Redactor::new(vec![]),
        )
        .unwrap();
        audit4
            .validate_capacity_tenants(&BTreeSet::from(["tenant-a".into(), "tenant-b".into()]))
            .unwrap();
        for tenants in [
            vec!["tenant-a"],
            vec!["tenant-a", "tenant-b", "tenant-c"],
            vec!["tenant-a", "tenant-c"],
        ] {
            failure(
                audit4
                    .validate_capacity_tenants(&tenants.into_iter().map(str::to_owned).collect())
                    .unwrap_err(),
                AuthenticationAuditFailureV1::Format,
            );
        }
        let a = binding("old-lane");
        failure(
            audit4
                .begin_verified_initial(initial(&a), a.principal.clone(), a.access_fingerprint)
                .unwrap_err(),
            AuthenticationAuditFailureV1::Format,
        );
    }

    #[test]
    fn maximum_identity_terminal_wrappers_fit_and_oversized_plan_creates_no_promise() {
        let root = private_dir();
        let mut a = binding("max");
        let prefix = "https://identity.example.test/";
        a.issuer = format!("{prefix}{}", "a".repeat(2048 - prefix.len()));
        a.client_id = "\\\"".repeat(256); // 512 bytes, every byte needs JSON escaping.
        a.principal = PrincipalRefV1::HumanOidc {
            issuer: a.issuer.clone(),
            subject: "\\\"".repeat(256),
        };
        let mut policy = policy();
        policy.approved_clients = vec![ApprovedAuthenticationClientV1 {
            issuer: a.issuer.clone(),
            client_id: a.client_id.clone(),
        }];
        let tenant = "a".repeat(128);
        let mut plan = plan(cap(8, 300_000), cap(4, 70_000));
        plan.tenants[0].tenant_id = tenant.clone();
        let audit = AuthenticationAudit::init_new_v4(
            &root.path().join("max.sqlite"),
            Uuid::new_v4(),
            policy,
            plan,
            Redactor::new(vec![]),
        )
        .unwrap();
        admit(&audit, &a, &tenant);
        let refresh = intention(&a, AuthenticationActionV1::Refresh);
        let pending = audit
            .refresh_pending(refresh.clone(), 1, a.access_fingerprint)
            .unwrap();
        let logout = intention(&a, AuthenticationActionV1::Logout);
        let ilogout = audit
            .begin_native_logout_during_refresh(
                logout.clone(),
                a.clone(),
                refresh.operation_id,
                pending.receipt.clone(),
            )
            .unwrap();
        let closed = audit
            .close_session(
                terminal(
                    &refresh,
                    pending.receipt.clone(),
                    a.principal.clone(),
                    AuthenticationOutcomeV1::Unknown,
                    AuthenticationReasonV1::Interrupted,
                ),
                1,
                a.access_fingerprint,
            )
            .unwrap();
        audit
            .record_native_logout_after_refresh_closed(
                terminal(
                    &logout,
                    ilogout,
                    a.principal.clone(),
                    AuthenticationOutcomeV1::Succeeded,
                    AuthenticationReasonV1::UserRequested,
                ),
                a,
                refresh.operation_id,
                pending.receipt,
                closed,
            )
            .unwrap();
        audit.verify().unwrap();
        let before_oversized = audit.verify().unwrap();
        let mut oversized_record = initial(&binding("oversized-envelope"));
        // Deliberately invalid hostile input: the complete entry cannot fit.
        // Refusal must leave no durable intention/ticket or quota change.
        oversized_record.client_id = "x".repeat(32_768);
        let oversized_binding = binding("oversized-envelope");
        failure(
            audit
                .begin_verified_initial_scoped(
                    oversized_record,
                    oversized_binding.principal.clone(),
                    oversized_binding.access_fingerprint,
                    candidates(&oversized_binding, &[&tenant]),
                )
                .unwrap_err(),
            AuthenticationAuditFailureV1::InvalidInput,
        );
        assert_eq!(audit.verify().unwrap(), before_oversized);
        let mut oversized = self::plan(cap(0, 0), cap(0, 0));
        oversized.tenants = (0..256)
            .map(|index| AuthenticationTenantCapacityV1 {
                tenant_id: format!("{index:03}{}", "a".repeat(125)),
                capacity: cap(0, 0),
            })
            .collect();
        failure(
            oversized.validate().unwrap_err(),
            AuthenticationAuditFailureV1::InvalidInput,
        );
        let new_path = root.path().join("oversized.sqlite");
        assert!(
            AuthenticationAudit::init_new_v4(
                &new_path,
                Uuid::new_v4(),
                self::policy(),
                oversized,
                Redactor::new(vec![])
            )
            .is_err()
        );
        assert!(!new_path.exists());
    }

    // Deliberately writes only an in-memory synthetic journal, through SQL,
    // bypassing all writer admission checks. Hashes, canonical bytes, phase
    // columns and quota are coherent so replay's semantic guards are decisive.
    fn append_synthetic(
        db: &rusqlite::Connection,
        entry: format::Entry,
    ) -> AuthenticationReceiptV1 {
        let (sequence, previous): (i64, String) = db
            .query_row(
                "SELECT seq,hash FROM authentication_entries ORDER BY seq DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let namespace: String = db
            .query_row(
                "SELECT namespace_uuid FROM authentication_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let sequence = u64::try_from(sequence).unwrap() + 1;
        let payload = format::checked_json(&entry, &Redactor::new(vec![])).unwrap();
        let received = format::timestamp();
        let hash = format::digest(sequence, &received, &previous, &payload);
        let operation = entry.record().map(|record| record.operation_id.to_string());
        let phase = entry
            .record()
            .map(|record| format::phase_key(record.phase).0);
        db.execute("INSERT INTO authentication_entries(seq,received_at,prev_hash,hash,payload,operation_id,phase) VALUES(?1,?2,?3,?4,?5,?6,?7)", rusqlite::params![i64::try_from(sequence).unwrap(), received, previous, hash, payload, operation, phase]).unwrap();
        db.execute("UPDATE authentication_quota SET records=(SELECT count(*) FROM authentication_entries),bytes=(SELECT sum(length(CAST(payload AS BLOB))) FROM authentication_entries) WHERE singleton=1", []).unwrap();
        AuthenticationReceiptV1 {
            namespace_id: Uuid::parse_str(&namespace).unwrap(),
            sequence,
            hash,
        }
    }

    fn synthetic_v4() -> (rusqlite::Connection, Uuid, AuthenticationAuditPolicyV1) {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let installation = Uuid::new_v4();
        let policy = policy();
        format::initialize(
            &db,
            installation,
            Uuid::new_v4(),
            &policy,
            &Redactor::new(vec![]),
        )
        .unwrap();
        append_synthetic(
            &db,
            format::Entry::CapacityPlan {
                plan: plan(cap(10, 150_000), cap(10, 150_000)),
            },
        );
        db.pragma_update(None, "user_version", format::TENANT_CAPACITY_VERSION)
            .unwrap();
        assert_eq!(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .unwrap()
                .verified
                .records,
            2
        );
        (db, installation, policy)
    }

    fn charged(owner: CapacityOwner, entry: format::Entry) -> format::Entry {
        format::Entry::Charged {
            charge: owner,
            entry: Box::new(entry),
        }
    }

    fn synthetic_initial(
        db: &rusqlite::Connection,
        binding: &AuthenticationSessionBindingV1,
        owner: CapacityOwner,
    ) -> (AuthenticationRecordV1, AuthenticationReceiptV1) {
        let record = initial(binding);
        let receipt = append_synthetic(
            db,
            charged(
                owner,
                format::Entry::VerifiedInitial {
                    record: record.clone(),
                    principal: binding.principal.clone(),
                    access_fingerprint: binding.access_fingerprint,
                    membership_recovery: true,
                },
            ),
        );
        (record, receipt)
    }

    fn assert_synthetic_chain_and_quota(db: &rusqlite::Connection) {
        let mut previous = "0".repeat(64);
        let mut records = 0u64;
        let mut bytes = 0u64;
        let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload,operation_id,phase FROM authentication_entries ORDER BY seq").unwrap();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let sequence = u64::try_from(row.get::<_, i64>(0).unwrap()).unwrap();
            let received: String = row.get(1).unwrap();
            let stored_previous: String = row.get(2).unwrap();
            let hash: String = row.get(3).unwrap();
            let payload: String = row.get(4).unwrap();
            let projected_operation: Option<String> = row.get(5).unwrap();
            let projected_phase: Option<String> = row.get(6).unwrap();
            records += 1;
            bytes += payload.len() as u64;
            assert_eq!(sequence, records);
            assert_eq!(stored_previous, previous);
            assert_eq!(
                hash,
                format::digest(sequence, &received, &previous, &payload)
            );
            let decoded: format::Entry = serde_json::from_str(&payload).unwrap();
            assert_eq!(
                projected_operation,
                decoded
                    .record()
                    .map(|record| record.operation_id.to_string())
            );
            assert_eq!(
                projected_phase.as_deref(),
                decoded
                    .record()
                    .map(|record| format::phase_key(record.phase).0)
            );
            assert_eq!(
                format::checked_json(&decoded, &Redactor::new(vec![])).unwrap(),
                payload
            );
            previous = hash;
        }
        let quota: (i64, i64) = db
            .query_row(
                "SELECT records,bytes FROM authentication_quota WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (
                u64::try_from(quota.0).unwrap(),
                u64::try_from(quota.1).unwrap()
            ),
            (records, bytes)
        );
    }

    #[test]
    fn replay_rejects_coherent_raw_intention_owner_change_and_unscoped_admit() {
        let owner_a = CapacityOwner::VerifiedMembership {
            tenant_id: "tenant-a".into(),
        };
        let owner_b = CapacityOwner::VerifiedMembership {
            tenant_id: "tenant-b".into(),
        };
        // Positive control: this synthetic route is accepted without invoking
        // the audit writer, including the exact pending/result session shapes.
        let (db, installation, policy) = synthetic_v4();
        let a = binding("synthetic-replay-a");
        let (initial, receipt) = synthetic_initial(&db, &a, owner_a.clone());
        append_synthetic(
            &db,
            charged(
                owner_a.clone(),
                format::Entry::Session {
                    mutation: Box::new(format::SessionMutation::Admit {
                        record: terminal(
                            &initial,
                            receipt,
                            a.principal.clone(),
                            AuthenticationOutcomeV1::Succeeded,
                            AuthenticationReasonV1::Verified,
                        ),
                        binding: a.clone(),
                    }),
                },
            ),
        );
        assert_eq!(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .unwrap()
                .verified
                .records,
            4
        );
        // Raw logout is otherwise a valid new lifecycle intention, but has no
        // historical ticket and no charge after the plan. Only replay runs.
        append_synthetic(
            &db,
            format::Entry::Authentication {
                record: intention(&a, AuthenticationActionV1::Logout),
            },
        );
        assert_synthetic_chain_and_quota(&db);
        failure(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .err()
                .unwrap(),
            AuthenticationAuditFailureV1::PhaseOrder,
        );

        let (db, installation, policy) = synthetic_v4();
        let (initial, receipt) = synthetic_initial(&db, &a, owner_a.clone());
        append_synthetic(
            &db,
            charged(
                owner_a.clone(),
                format::Entry::Session {
                    mutation: Box::new(format::SessionMutation::Admit {
                        record: terminal(
                            &initial,
                            receipt,
                            a.principal.clone(),
                            AuthenticationOutcomeV1::Succeeded,
                            AuthenticationReasonV1::Verified,
                        ),
                        binding: a.clone(),
                    }),
                },
            ),
        );
        let refresh = intention(&a, AuthenticationActionV1::Refresh);
        let pending = append_synthetic(
            &db,
            charged(
                owner_a.clone(),
                format::Entry::Session {
                    mutation: Box::new(format::SessionMutation::RefreshPending {
                        record: refresh.clone(),
                        expected_generation: 1,
                        expected_access_fingerprint: a.access_fingerprint,
                    }),
                },
            ),
        );
        assert_eq!(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .unwrap()
                .verified
                .records,
            5
        );
        append_synthetic(
            &db,
            charged(
                owner_b,
                format::Entry::Session {
                    mutation: Box::new(format::SessionMutation::Close {
                        record: terminal(
                            &refresh,
                            pending,
                            a.principal.clone(),
                            AuthenticationOutcomeV1::Unknown,
                            AuthenticationReasonV1::Interrupted,
                        ),
                        expected_generation: 1,
                        expected_access_fingerprint: a.access_fingerprint,
                    }),
                },
            ),
        );
        assert_synthetic_chain_and_quota(&db);
        failure(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .err()
                .unwrap(),
            AuthenticationAuditFailureV1::PhaseOrder,
        );

        let (db, installation, policy) = synthetic_v4();
        let unscoped = binding("synthetic-unscoped-replay");
        let (initial, receipt) = synthetic_initial(&db, &unscoped, CapacityOwner::Unscoped);
        assert_eq!(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .unwrap()
                .verified
                .records,
            3
        );
        append_synthetic(
            &db,
            charged(
                CapacityOwner::Unscoped,
                format::Entry::Session {
                    mutation: Box::new(format::SessionMutation::Admit {
                        record: terminal(
                            &initial,
                            receipt,
                            unscoped.principal.clone(),
                            AuthenticationOutcomeV1::Succeeded,
                            AuthenticationReasonV1::Verified,
                        ),
                        binding: unscoped,
                    }),
                },
            ),
        );
        assert_synthetic_chain_and_quota(&db);
        failure(
            format::replay(&db, installation, &policy, &Redactor::new(vec![]), None)
                .err()
                .unwrap(),
            AuthenticationAuditFailureV1::PhaseOrder,
        );
    }

    #[test]
    fn maximal_json_escaped_issuer_passes_real_validators_and_terminal_replay() {
        let root = private_dir();
        let mut a = binding("escaped-issuer");
        let prefix = "https://identity.example.test/";
        a.issuer = format!("{prefix}{}", "\"".repeat(2048 - prefix.len()));
        a.client_id = "\\\"".repeat(256);
        a.principal = PrincipalRefV1::HumanOidc {
            issuer: a.issuer.clone(),
            subject: "\\\"".repeat(256),
        };
        // The real identity validator accepts the original issuer bytes. URL
        // parsing percent-encodes the quotes; it does not authorize or rewrite
        // the exact issuer retained by this ledger.
        assert_eq!(a.issuer.len(), 2048);
        assert!(
            reqwest::Url::parse(&a.issuer)
                .unwrap()
                .as_str()
                .contains("%22")
        );
        a.principal.validate().unwrap();
        assert_eq!(
            serde_json::to_string(&a.issuer)
                .unwrap()
                .matches("\\\"")
                .count(),
            2048 - prefix.len()
        );
        let installation = Uuid::new_v4();
        let mut policy = policy();
        policy.approved_clients = vec![ApprovedAuthenticationClientV1 {
            issuer: a.issuer.clone(),
            client_id: a.client_id.clone(),
        }];
        format::validate_policy(installation, &policy, &Redactor::new(vec![])).unwrap();
        let tenant = "a".repeat(128);
        let mut plan = plan(cap(8, 400_000), cap(4, 70_000));
        plan.tenants[0].tenant_id = tenant.clone();
        let path = root.path().join("escaped-issuer.sqlite");
        let audit = AuthenticationAudit::init_new_v4(
            &path,
            installation,
            policy.clone(),
            plan,
            Redactor::new(vec![]),
        )
        .unwrap();
        admit(&audit, &a, &tenant);
        let refresh = intention(&a, AuthenticationActionV1::Refresh);
        let pending = audit
            .refresh_pending(refresh.clone(), 1, a.access_fingerprint)
            .unwrap();
        let logout = intention(&a, AuthenticationActionV1::Logout);
        let ilogout = audit
            .begin_native_logout_during_refresh(
                logout.clone(),
                a.clone(),
                refresh.operation_id,
                pending.receipt.clone(),
            )
            .unwrap();
        let closed = audit
            .close_session(
                terminal(
                    &refresh,
                    pending.receipt.clone(),
                    a.principal.clone(),
                    AuthenticationOutcomeV1::Unknown,
                    AuthenticationReasonV1::Interrupted,
                ),
                1,
                a.access_fingerprint,
            )
            .unwrap();
        let end = terminal(
            &logout,
            ilogout,
            a.principal.clone(),
            AuthenticationOutcomeV1::Succeeded,
            AuthenticationReasonV1::UserRequested,
        );
        let receipt = audit
            .record_native_logout_after_refresh_closed(
                end.clone(),
                a.clone(),
                refresh.operation_id,
                pending.receipt.clone(),
                closed.clone(),
            )
            .unwrap();
        let verified = audit.verify().unwrap();
        drop(audit);
        let reopened =
            AuthenticationAudit::open_existing(&path, installation, policy, Redactor::new(vec![]))
                .unwrap();
        assert_eq!(reopened.verify().unwrap(), verified);
        assert_eq!(
            reopened
                .record_native_logout_after_refresh_closed(
                    end,
                    a,
                    refresh.operation_id,
                    pending.receipt,
                    closed
                )
                .unwrap(),
            receipt
        );
        assert_eq!(reopened.verify().unwrap(), verified);
    }
}
