use super::*;
use rusqlite::types::Value as Sql;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Head {
    pub(super) policy: WirePolicy,
    pub(super) epoch: u64,
    pub(super) state: EgressPolicyStateV1,
    pub(super) event_seq: i64,
}
#[derive(Default)]
pub(super) struct Projection {
    pub(super) genesis: Option<(Genesis, i64, String)>,
    pub(super) revisions: BTreeMap<(EgressPolicySelectorV1, u64), (WirePolicy, i64)>,
    pub(super) heads: BTreeMap<EgressPolicySelectorV1, Head>,
    pub(super) operations: BTreeMap<uuid::Uuid, (String, String, String, i64)>,
    pub(super) snapshot_bytes: usize,
}
pub(in crate::audit) fn marker(event: &Event) -> anyhow::Result<()> {
    event.validate()?;
    ensure(
        event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "native"
            && event.command.is_none()
            && event.decision.is_none()
            && event.result.is_none()
            && event.project_id.is_none()
            && event.sandbox_id.is_none()
            && event.agent_id.is_none()
            && event.parent_agent_id.is_none()
            && event.correlation_id.is_none()
            && event.trace_id.is_none(),
    )?;
    match event.kind.as_str() {
        GENESIS_KIND => {
            let g: Genesis = serde_json::from_value(event.data.clone())?;
            ensure(
                g.version == 1
                    && g.from_format == 14
                    && g.format == 15
                    && !g.migration_id.is_nil()
                    && !g.source_id.is_nil()
                    && !g.namespace.is_nil()
                    && g.previous_seq > 0
                    && event.id == g.migration_id.to_string()
                    && event.session_id.is_none(),
            )?;
            g.limits.validate()?;
            crate::tenancy::validate_tenant_id(&g.tenant_id)?;
        }
        REPLACE_KIND | REVOKE_KIND => {
            let record: MutationRecord = serde_json::from_value(event.data.clone())?;
            record.validate(&record.tenant_id)?;
            ensure(
                event.kind == record.change.kind() && event.id == record.change.id().to_string(),
            )?;
            let session = match record.change.selector() {
                EgressPolicySelectorV1::Tenant => None,
                EgressPolicySelectorV1::Session(id) => Some(id.as_str()),
            };
            ensure(event.session_id.as_deref() == session)?;
        }
        _ => return Err(EgressStoreFailure::Integrity.into()),
    }
    ensure(serde_json::to_vec(event)?.len() <= EVENT_BYTES)
}

pub(super) fn require15(db: &Connection) -> anyhow::Result<()> {
    if db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == 15 {
        Ok(())
    } else {
        Err(EgressStoreFailure::MigrationRequired.into())
    }
}

/// Current identity projection only. Absence is a successful exact lookup;
/// malformed payloads and database failures are never converted to absence.
/// Historical policy replay deliberately retains its event-prefix scan below.
pub(super) fn current_session_grant(
    db: &Connection,
    resource: &crate::identity::ResourceRefV1,
) -> anyhow::Result<Option<crate::identity::ResourceGrantV1>> {
    identity::json_row(
        db,
        "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
        &resource.key()?,
    )
}

/// A real active identity Session resource, selected only inside this Store.
/// Historical replay reads its last typed identity mutation before the policy.
pub(super) fn require_session(
    db: &Connection,
    selector: &EgressPolicySelectorV1,
    before_seq: Option<i64>,
    tenant: &str,
) -> anyhow::Result<()> {
    use crate::identity::{
        IdentityChangeV1, IdentityMutationV1, ResourceGrantV1, ResourceKindV1, ResourceRefV1,
    };
    let EgressPolicySelectorV1::Session(id) = selector else {
        return Ok(());
    };
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: id.clone(),
    };
    let grant: Option<ResourceGrantV1> = if let Some(before) = before_seq {
        let mut query=db.prepare("SELECT payload FROM events WHERE kind='control.identity.mutation' AND seq<?1 ORDER BY seq DESC")?;
        let mut found = None;
        for row in query.query_map([before], |r| r.get::<_, String>(0))? {
            let event: Event = serde_json::from_str(&row?)?;
            let mutation: IdentityMutationV1 =
                serde_json::from_value(event.data["mutation"].clone())?;
            if let IdentityChangeV1::ResourceGrant { grant, .. } = mutation.change
                && grant.resource == resource
            {
                found = Some(grant);
                break;
            }
        }
        found
    } else {
        current_session_grant(db, &resource)?
    };
    let grant = grant.ok_or(EgressStoreFailure::Permission)?;
    grant.validate()?;
    if grant.active && grant.tenant_id == tenant && grant.resource == resource {
        Ok(())
    } else {
        Err(EgressStoreFailure::Permission.into())
    }
}

impl Projection {
    pub(super) fn limits(&self) -> anyhow::Result<&EgressStoreLimitsV1> {
        Ok(&self
            .genesis
            .as_ref()
            .ok_or(EgressStoreFailure::Integrity)?
            .0
            .limits)
    }
    pub(super) fn active(&self) -> usize {
        self.heads
            .values()
            .filter(|h| h.state == EgressPolicyStateV1::Active)
            .count()
    }
    pub(super) fn ceiling(&self, change: &Change) -> anyhow::Result<Option<Ceiling>> {
        match change {
            Change::Replace {
                selector: EgressPolicySelectorV1::Session(_),
                policy,
                tenant_ceiling,
                ..
            } => {
                let tenant = self
                    .heads
                    .get(&EgressPolicySelectorV1::Tenant)
                    .ok_or(EgressStoreFailure::Conflict)?;
                cas(tenant.state == EgressPolicyStateV1::Active)?;
                policy.within(&tenant.policy)?;
                let current = Ceiling {
                    revision: tenant.policy.revision(),
                    epoch: tenant.epoch,
                    digest: tenant.policy.digest().into(),
                };
                cas(tenant_ceiling.as_ref() == Some(&current))?;
                Ok(Some(current))
            }
            _ => Ok(None),
        }
    }
    pub(super) fn preflight(&self, record: &MutationRecord, at: u64) -> anyhow::Result<Head> {
        record.validate(
            &self
                .genesis
                .as_ref()
                .ok_or(EgressStoreFailure::Integrity)?
                .0
                .tenant_id,
        )?;
        let old = self.heads.get(record.change.selector());
        let expected = old.map_or((0, 0), |h| (h.policy.revision(), h.epoch));
        cas(record.change.expected() == expected
            && !self.operations.contains_key(&record.change.id()))?;
        ensure(record.tenant_ceiling == self.ceiling(&record.change)?)?;
        let (policy, state, bytes, revisions, active) = match &record.change {
            Change::Replace { policy, .. } => {
                cas(policy.active_at(at))?;
                let bytes = self
                    .snapshot_bytes
                    .checked_add(bounded_json(policy, POLICY_BYTES)?.len())
                    .ok_or(EgressStoreFailure::Capacity)?;
                let active = self.active()
                    + usize::from(old.is_none_or(|h| h.state != EgressPolicyStateV1::Active));
                (
                    policy.clone(),
                    EgressPolicyStateV1::Active,
                    bytes,
                    self.revisions.len() + 1,
                    active,
                )
            }
            Change::Revoke { .. } => {
                let old = old.ok_or(EgressStoreFailure::Conflict)?;
                cas(old.state == EgressPolicyStateV1::Active)?;
                // Revoke is still admitted after session end or old expiry.
                (
                    old.policy.clone(),
                    EgressPolicyStateV1::RevokeRequested,
                    self.snapshot_bytes,
                    self.revisions.len(),
                    self.active()
                        .checked_sub(1)
                        .ok_or(EgressStoreFailure::Integrity)?,
                )
            }
        };
        self.limits()?
            .admits(self.operations.len() + 1, revisions, active, bytes)?;
        Ok(Head {
            policy,
            epoch: record.after_epoch,
            state,
            event_seq: 0,
        })
    }
    fn apply(
        &mut self,
        db: &Connection,
        stored: &StoredEvent,
        record: MutationRecord,
    ) -> anyhow::Result<()> {
        let at = u64::try_from(
            chrono::DateTime::parse_from_rfc3339(&stored.received_at)?.timestamp_millis(),
        )?;
        let mut next = self.preflight(&record, at)?;
        if matches!(record.change, Change::Replace { .. }) {
            require_session(
                db,
                record.change.selector(),
                Some(stored.seq),
                &record.tenant_id,
            )?;
        }
        let genesis = &self
            .genesis
            .as_ref()
            .ok_or(EgressStoreFailure::Integrity)?
            .0;
        ensure(record.actor.source_id == genesis.source_id)?;
        verify_historical_actor(db, stored.seq, &record.actor, &record.tenant_id)?;
        next.event_seq = stored.seq;
        let receipt = EgressPolicyReceiptV1::from_event(stored, &record, &next.policy);
        let receipt = bounded_json(&receipt, RECEIPT_BYTES)?;
        if matches!(record.change, Change::Replace { .. }) {
            self.snapshot_bytes = self
                .snapshot_bytes
                .checked_add(bounded_json(&next.policy, POLICY_BYTES)?.len())
                .ok_or(EgressStoreFailure::Capacity)?;
            ensure(
                self.revisions
                    .insert(
                        (record.change.selector().clone(), next.policy.revision()),
                        (next.policy.clone(), stored.seq),
                    )
                    .is_none(),
            )?;
        }
        self.heads.insert(record.change.selector().clone(), next);
        self.operations.insert(
            record.change.id(),
            (
                record.change.action().into(),
                record.input_hash,
                receipt,
                stored.seq,
            ),
        );
        Ok(())
    }
}

/// A historical machine binding is descriptive evidence of the authenticated
/// control actor; it never manufactures a live credential or human approval.
fn verify_historical_actor(
    db: &Connection,
    before: i64,
    actor: &MachineActor,
    tenant: &str,
) -> anyhow::Result<()> {
    let mut history = BTreeMap::new();
    let mut query = db.prepare("SELECT payload FROM events WHERE seq<?1 AND kind IN ('control.review_binding.provisioned','control.review_binding.revoked') ORDER BY seq")?;
    for row in query.query_map([before], |r| r.get::<_, String>(0))? {
        let event: Event = serde_json::from_str(&row?)?;
        let snapshot: crate::control::review_binding::ReviewBindingSnapshotV1 =
            serde_json::from_value(event.data["snapshot"].clone())?;
        snapshot.validate()?;
        history.insert(snapshot.binding.binding_id.clone(), snapshot);
    }
    ensure(history.values().filter(|s| s.binding.active).count() == 1)?;
    let snapshot = history
        .get(&actor.binding_id)
        .ok_or(EgressStoreFailure::Integrity)?;
    snapshot.validate()?;
    ensure(
        snapshot.binding.active
            && snapshot.policy.active
            && snapshot.binding.tenant_id == tenant
            && snapshot.binding.binding_id == actor.binding_id
            && snapshot.binding.revision == actor.binding_revision
            && snapshot.binding_digest == actor.binding_digest
            && snapshot.binding.credential_id == actor.credential_id.to_string()
            && snapshot.binding.credential_generation == actor.credential_generation,
    )
}

pub(super) fn replay(db: &Connection) -> anyhow::Result<Projection> {
    require15(db)?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let authority = review_authority::replay(db)?;
    let mut projection = Projection::default();
    // This family did not exist in legacy histories. Select its ASCII case
    // variants too; the closed marker then rejects every noncanonical name.
    // Inherited families deliberately retain their separate canonical GLOB.
    let mut statement=db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE lower(kind) GLOB 'control.egress_policy.*' ORDER BY seq")?;
    for row in statement.query_map([], row_event)? {
        let stored = row?;
        marker(&stored.event)?;
        if stored.event.kind == GENESIS_KIND {
            ensure(projection.genesis.is_none())?;
            let g: Genesis = serde_json::from_value(stored.event.data.clone())?;
            require_genesis_anchor_order(&g, stored.seq)?;
            ensure(
                g.tenant_id == tenant
                    && g.source_id == authority.anchor.source_id
                    && g.namespace == authority.anchor.durable_namespace_id
                    && g.source_anchor == authority.anchor_receipt
                    && g.previous_seq.checked_add(1) == Some(stored.seq)
                    && g.previous_hash == stored.prev_hash,
            )?;
            projection.genesis = Some((g, stored.seq, stored.hash));
        } else {
            let record: MutationRecord = serde_json::from_value(stored.event.data.clone())?;
            record.validate(&tenant)?;
            projection.apply(db, &stored, record)?;
        }
    }
    ensure(projection.genesis.is_some())?;
    verify_rows(db, &projection)?;
    Ok(projection)
}
pub(super) fn require_genesis_anchor_order(
    genesis: &Genesis,
    genesis_seq: i64,
) -> anyhow::Result<()> {
    // Format15 extends the committed14 prefix; its referenced original anchor
    // must already be in that prefix. No source receipt can point forward.
    ensure(
        genesis.source_anchor.seq > 0
            && genesis.source_anchor.seq <= genesis.previous_seq
            && genesis.previous_seq.checked_add(1) == Some(genesis_seq),
    )
}
pub(in crate::audit) fn validate(db: &Connection) -> anyhow::Result<()> {
    let format: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if format == 15 {
        replay(db).map(|_| ())
    } else {
        let count: i64 = db.query_row(
            "SELECT count(*) FROM events WHERE lower(kind) GLOB 'control.egress_policy.*'",
            [],
            |r| r.get(0),
        )?;
        ensure(count == 0)
    }
}

fn text(v: impl Into<String>) -> Sql {
    Sql::Text(v.into())
}
fn number(v: u64) -> anyhow::Result<Sql> {
    Ok(Sql::Integer(i64::try_from(v)?))
}
fn compare_rows(a: &[Sql], b: &[Sql]) -> std::cmp::Ordering {
    for (a, b) in a.iter().zip(b) {
        let order = match (a, b) {
            (Sql::Integer(a), Sql::Integer(b)) => a.cmp(b),
            (Sql::Text(a), Sql::Text(b)) => a.cmp(b),
            (Sql::Null, Sql::Null) => std::cmp::Ordering::Equal,
            // Exact projections never contain mixed/blob/real columns. This
            // fallback is only an ordering for malformed rows before refusal.
            _ => format!("{a:?}").cmp(&format!("{b:?}")),
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}
fn rows(db: &Connection, sql: &str, columns: usize) -> anyhow::Result<Vec<Vec<Sql>>> {
    Ok(db
        .prepare(sql)?
        .query_map([], |r| {
            (0..columns)
                .map(|i| r.get(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
fn expected_rows(projection: &Projection) -> anyhow::Result<[Vec<Vec<Sql>>; 5]> {
    let (g, seq, hash) = projection
        .genesis
        .as_ref()
        .ok_or(EgressStoreFailure::Integrity)?;
    let meta = vec![vec![
        Sql::Integer(1),
        Sql::Integer(1),
        Sql::Integer(*seq),
        text(hash),
        text(serde_json::to_string(&g.limits)?),
    ]];
    let mut revisions = Vec::new();
    let mut heads = Vec::new();
    let mut reservations = Vec::new();
    let mut operations = Vec::new();
    for ((selector, revision), (policy, seq)) in &projection.revisions {
        let (kind, id) = selector.sql();
        revisions.push(vec![
            text(kind),
            text(id),
            number(*revision)?,
            text(bounded_json(policy, POLICY_BYTES)?),
            text(policy.digest()),
            Sql::Integer(*seq),
        ]);
    }
    for (selector, head) in &projection.heads {
        let (kind, id) = selector.sql();
        heads.push(vec![
            text(kind),
            text(id),
            number(head.policy.revision())?,
            number(head.epoch)?,
            text(head.state.sql()),
            Sql::Integer(head.event_seq),
        ]);
        if head.state == EgressPolicyStateV1::Active {
            reservations.push(vec![
                text(kind),
                text(id),
                Sql::Integer(1),
                Sql::Integer(EVENT_BYTES as i64),
            ]);
        }
    }
    for (id, (action, hash, receipt, seq)) in &projection.operations {
        operations.push(vec![
            text(id.to_string()),
            text(action),
            text(hash),
            text(receipt),
            Sql::Integer(*seq),
        ]);
    }
    // Enum order is not SQLite lexical order (session sorts before tenant).
    for collection in [
        &mut revisions,
        &mut heads,
        &mut operations,
        &mut reservations,
    ] {
        collection.sort_by(|a, b| compare_rows(a, b));
    }
    Ok([meta, revisions, heads, operations, reservations])
}
pub(super) fn verify_rows(db: &Connection, projection: &Projection) -> anyhow::Result<()> {
    let expected = expected_rows(projection)?;
    for(index,(query,count))in [
        ("SELECT singleton,version,genesis_seq,genesis_hash,limits_json FROM egress_policy_meta",5),
        ("SELECT scope_kind,scope_id,revision,policy_json,policy_digest,event_seq FROM egress_policy_revisions",6),
        ("SELECT scope_kind,scope_id,revision,epoch,state,event_seq FROM egress_policy_heads",6),
        ("SELECT operation_id,action,input_hash,receipt_json,event_seq FROM egress_policy_operations",5),
        ("SELECT scope_kind,scope_id,events,bytes FROM egress_policy_reservations",4),
    ].into_iter().enumerate() {
        let mut actual=rows(db,query,count)?;actual.sort_by(|a,b|compare_rows(a,b));ensure(actual==expected[index])?;
    }
    projection.limits()?.admits(
        projection.operations.len(),
        projection.revisions.len(),
        projection.active(),
        projection.snapshot_bytes,
    )
}

pub(super) fn write_head(
    db: &Connection,
    selector: &EgressPolicySelectorV1,
    head: &Head,
) -> anyhow::Result<()> {
    let (kind, id) = selector.sql();
    let changed=db.execute("INSERT INTO egress_policy_heads(scope_kind,scope_id,revision,epoch,state,event_seq) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope_kind,scope_id) DO UPDATE SET revision=excluded.revision,epoch=excluded.epoch,state=excluded.state,event_seq=excluded.event_seq",params![kind,id,i64::try_from(head.policy.revision())?,i64::try_from(head.epoch)?,head.state.sql(),head.event_seq])?;
    ensure(changed == 1)
}
