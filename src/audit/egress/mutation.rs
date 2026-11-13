use super::*;
use crate::audit::human_modern::{HumanDiscoveryGeneration, discovery_generation};
use crate::control::egress_policy::EgressPolicyExpectedScopeV1;

fn check_policy_context(
    store: &Store,
    tx: &CheckedTransaction<'_>,
    expected: &EgressPolicyExpectedScopeV1,
    generation: HumanDiscoveryGeneration,
) -> anyhow::Result<()> {
    tx.require_identity_authority()?;
    if discovery_generation(tx)? != generation {
        return Err(EgressStoreFailure::DiscoveryStale.into());
    }
    if store.tenant_id() != expected.tenant_id()
        || crate::audit::supervision::namespace(tx)? != expected.control_namespace_uuid() {
        return Err(EgressStoreFailure::ScopeConflict.into());
    }
    Ok(())
}

/// Minted only by an exact active head debit under the selected checked
/// transaction and revalidated machine binding. No request can construct it.
pub(in crate::audit) struct TerminalReservation<'a> {
    connection: &'a Connection,
    selector: EgressPolicySelectorV1,
    revision: u64,
    epoch: u64,
    operation_id: uuid::Uuid,
}
impl TerminalReservation<'_> {
    pub(in crate::audit) fn validate(
        &self,
        tx: &Transaction<'_>,
        event: &Event,
    ) -> anyhow::Result<()> {
        ensure(
            std::ptr::eq(self.connection, &**tx)
                && event.id == self.operation_id.to_string()
                && event.kind == REVOKE_KIND,
        )?;
        let (kind, id) = self.selector.sql();
        let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM egress_policy_heads h WHERE h.scope_kind=?1 AND h.scope_id=?2 AND h.revision=?3 AND h.epoch=?4 AND h.state='active' AND NOT EXISTS(SELECT 1 FROM egress_policy_reservations r WHERE r.scope_kind=h.scope_kind AND r.scope_id=h.scope_id))",params![kind,id,i64::try_from(self.revision)?,i64::try_from(self.epoch)?],|r|r.get(0))?;
        ensure(exact)
    }
}

fn debit<'a>(
    tx: &'a CheckedTransaction<'_>,
    change: &Change,
    head: &Head,
) -> anyhow::Result<TerminalReservation<'a>> {
    tx.require_identity_authority()?;
    cas(matches!(change, Change::Revoke { .. })
        && head.state == EgressPolicyStateV1::Active
        && change.expected() == (head.policy.revision(), head.epoch))?;
    let (kind, id) = change.selector().sql();
    let changed=tx.execute("DELETE FROM egress_policy_reservations WHERE scope_kind=?1 AND scope_id=?2 AND events=1 AND bytes=98304 AND EXISTS(SELECT 1 FROM egress_policy_heads h WHERE h.scope_kind=?1 AND h.scope_id=?2 AND h.revision=?3 AND h.epoch=?4 AND h.state='active')",params![kind,id,i64::try_from(head.policy.revision())?,i64::try_from(head.epoch)?])?;
    ensure(changed == 1)?;
    Ok(TerminalReservation {
        connection: tx,
        selector: change.selector().clone(),
        revision: head.policy.revision(),
        epoch: head.epoch,
        operation_id: change.id(),
    })
}

impl Store {
    pub(crate) fn egress_replace_policy_scoped(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        credential: &AuthenticatedReviewRunnerCredential,
        expected_scope: &EgressPolicyExpectedScopeV1,
        generation: HumanDiscoveryGeneration,
        request: &EgressReplacePolicyV1,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        self.mutate_egress_policy(epoch, source, credential, expected_scope, generation, request.change()?)
    }
    pub(crate) fn egress_revoke_policy_scoped(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        credential: &AuthenticatedReviewRunnerCredential,
        expected_scope: &EgressPolicyExpectedScopeV1,
        generation: HumanDiscoveryGeneration,
        request: &EgressRevokePolicyV1,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        self.mutate_egress_policy(epoch, source, credential, expected_scope, generation, request.change())
    }
    fn mutate_egress_policy(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        credential: &AuthenticatedReviewRunnerCredential,
        expected_scope: &EgressPolicyExpectedScopeV1,
        generation: HumanDiscoveryGeneration,
        change: Change,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        change.validate()?;
        expected_scope.validate().map_err(|_| EgressStoreFailure::Invalid)?;
        epoch.validate_source(source)?;
        let access = HumanSourceAccess::write(epoch, source)?;
        self.with_scoped_nowait(&access, StoreOperationClass::IdentityAuthority, |db| {
        let tx = self.begin_checked_with_access(db, Some(&access), StoreOperationClass::IdentityAuthority)?;
        tx.require_identity_authority()?;
        require15(&tx)?;
        // Required origin/integrity guards above remain intact. This assertion
        // precedes business policy replay, operation lookup, CAS and clock work.
        check_policy_context(self, &tx, expected_scope, generation)?;
        let verified = self.verify_review_binding_for_registration_in_transaction(
            &tx, epoch, source, credential,
        )?;
        let clock = workload::capture_control_time(&tx)?;
        let actor = MachineActor {
            kind: MachineActorKind::MachineControl,
            source_id: source.source_id(),
            credential_id: credential.credential_id(),
            credential_generation: credential.credential_generation(),
            binding_id: verified.binding().binding_id.clone(),
            binding_revision: verified.binding().revision,
            binding_digest: verified.binding_digest().into(),
        };
        actor.validate()?;
        let projection = replay(&tx)?;
        let input_hash = MutationRecord::input_hash(self.tenant_id(), &actor, &change)?;
        if let Some((action, hash, json, _)) = projection.operations.get(&change.id()) {
            cas(action == change.action() && hash == &input_hash)?;
            let receipt: EgressPolicyReceiptV1 = serde_json::from_str(json)?;
            ensure(bounded_json(&receipt, RECEIPT_BYTES)? == *json)?;
            verified.revalidate()?;
            epoch.validate_source(source)?;
            check_policy_context(self, &tx, expected_scope, generation)?;
            drop(verified);
            self.commit_authorized(tx, &clock)?;
            epoch.validate_source(source)?;
            return Ok(receipt);
        }
        let record = MutationRecord {
            version: 1,
            tenant_id: self.tenant_id().into(),
            actor,
            input_hash,
            tenant_ceiling: projection.ceiling(&change)?,
            after_epoch: change
                .expected()
                .1
                .checked_add(1)
                .ok_or(EgressStoreFailure::Capacity)?,
            change,
        };
        let at = u64::try_from(clock.at().timestamp_millis())?;
        let mut head = projection.preflight(&record, at)?;
        if matches!(record.change, Change::Replace { .. }) {
            require_session(&tx, record.change.selector(), None, self.tenant_id())?;
        }
        let mut event = Event::new(record.change.kind());
        event.id = record.change.id().to_string();
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = match record.change.selector() {
            EgressPolicySelectorV1::Tenant => None,
            EgressPolicySelectorV1::Session(id) => Some(id.clone()),
        };
        event.data = serde_json::to_value(&record)?;
        marker(&event)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        // Sensitive DNS metadata undergoes normal redaction; any collision
        // refuses the whole mutation rather than corrupting a canonical policy.
        ensure(serde_json::to_value(&event)? == original && payload.len() <= EVENT_BYTES)?;
        marker(&event)?;
        let (kind, id) = record.change.selector().sql();
        let stored = match &record.change {
            Change::Replace { .. } => {
                let old = projection.heads.get(record.change.selector());
                if old.is_none_or(|h| h.state != EgressPolicyStateV1::Active) {
                    ensure(tx.execute("INSERT INTO egress_policy_reservations(scope_kind,scope_id,events,bytes) VALUES(?1,?2,1,98304)",params![kind,id])?==1)?;
                }
                self.append_authorized_in_transaction(&tx, event, payload, &clock)?
            }
            Change::Revoke { .. } => {
                verified.revalidate()?;
                let old = projection
                    .heads
                    .get(record.change.selector())
                    .ok_or(EgressStoreFailure::Conflict)?;
                let reservation = debit(&tx, &record.change, old)?;
                self.append_with_authorized_capacity_class_at(
                    &tx,
                    event,
                    payload,
                    capacity::Class::EgressTerminal(&reservation),
                    &clock,
                    chrono::Utc::now(),
                )?
            }
        };
        head.event_seq = stored.seq;
        if matches!(record.change, Change::Replace { .. }) {
            ensure(tx.execute("INSERT INTO egress_policy_revisions(scope_kind,scope_id,revision,policy_json,policy_digest,event_seq) VALUES(?1,?2,?3,?4,?5,?6)",params![kind,id,i64::try_from(head.policy.revision())?,bounded_json(&head.policy,POLICY_BYTES)?,head.policy.digest(),stored.seq])?==1)?;
        }
        write_head(&tx, record.change.selector(), &head)?;
        let receipt = EgressPolicyReceiptV1::from_event(&stored, &record, &head.policy);
        let receipt_json = bounded_json(&receipt, RECEIPT_BYTES)?;
        ensure(tx.execute("INSERT INTO egress_policy_operations(operation_id,action,input_hash,receipt_json,event_seq) VALUES(?1,?2,?3,?4,?5)",params![record.change.id().to_string(),record.change.action(),record.input_hash,receipt_json,stored.seq])?==1)?;
        replay(&tx)?;
        verified.revalidate()?;
        epoch.validate_source(source)?;
        check_policy_context(self, &tx, expected_scope, generation)?;
        drop(verified);
        self.commit_authorized(tx, &clock)?;
        epoch.validate_source(source)?;
        Ok(receipt)
        })
    }

    /// Internal permission helper only; public read delivery/audit envelopes and
    /// HTTP exposure remain unavailable. No mutation permission is conferred.
    pub(crate) fn egress_read_policy_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &crate::identity::AuthorizedScopeV1,
        selector: &EgressPolicySelectorV1,
    ) -> anyhow::Result<Option<EgressPolicySnapshotV1>> {
        use crate::identity::{
            ContextModeV1, HumanCapabilityV1, ResourceKindV1, ResourcePermissionV1, ResourceRefV1,
        };
        selector.validate()?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let tx = self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?;
        require15(&tx)?;
        let current = identity::revalidate_scope_checked(&tx, scope, self.tenant_id())?;
        let allowed = match selector {
            EgressPolicySelectorV1::Tenant => {
                current.mode() == ContextModeV1::Administration
                    && current.has_capability(HumanCapabilityV1::ReadTenantMetadata)
            }
            EgressPolicySelectorV1::Session(id) => {
                let resource = ResourceRefV1 {
                    kind: ResourceKindV1::Session,
                    resource_id: id.clone(),
                };
                let grant: Option<crate::identity::ResourceGrantV1> = identity::json_row(
                    &tx,
                    "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
                    &resource.key()?,
                )?;
                grant.is_some_and(|g| current.permits_resource(&g, ResourcePermissionV1::Metadata))
            }
        };
        if !allowed {
            return Err(EgressStoreFailure::Permission.into());
        }
        let projection = replay(&tx)?;
        let result = projection
            .heads
            .get(selector)
            .map(|h| -> anyhow::Result<_> {
                Ok(EgressPolicySnapshotV1 {
                    selector: selector.clone(),
                    policy: h.policy.core()?,
                    epoch: h.epoch,
                    state: h.state,
                })
            })
            .transpose()?;
        // A configured redaction dictionary can make the exact policy metadata
        // undisclosable. Refuse rather than return an unmasked/altered snapshot.
        if let Some(head) = projection.heads.get(selector) {
            let json = serde_json::to_value(&head.policy)?;
            ensure(self.redactor.value(&json) == json)?;
        }
        tx.require_business_access()?;
        self.commit_checked(tx)?;
        Ok(result)
    }
}
