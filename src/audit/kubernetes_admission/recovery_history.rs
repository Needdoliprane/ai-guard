//! V3 descriptive history is revalidated; it never reconstructs a capability.
use super::*;

pub(super) fn validate_basis(
    db: &Connection,
    evidence: &AuthorizationEvidenceV3,
    intent: &KubernetesCreateIntentV1,
    tenant: &str,
    record_seq: Option<i64>,
) -> anyhow::Result<(StoredEvent, Record)> {
    migration::validate_recovery_marker(db)?;
    let marker: i64 = db.query_row(
        "SELECT seq FROM events WHERE kind=?1",
        [migration::RECOVERY_GENESIS_KIND],
        |row| row.get(0),
    )?;
    permission(
        evidence.origin_seq >= marker && record_seq.is_none_or(|seq| evidence.origin_seq < seq),
    )?;
    let hash: String = db.query_row(
        "SELECT hash FROM events WHERE seq=?1",
        [evidence.origin_seq],
        |row| row.get(0),
    )?;
    permission(hash == evidence.origin_hash)?;
    let (reserved, original) = replay::reservation(db, intent, tenant)?;
    let expected = model::transition(
        intent,
        None,
        &Change::Reserve {
            intent: intent.clone(),
        },
        None,
    )?;
    let RecoveryBasisV3::RecoverBeforeEmission {
        reservation_seq,
        reservation_hash,
        admission_decision_hash,
        creator_incarnation_id,
        reserved_snapshot_hash,
        ..
    } = &evidence.basis;
    permission(
        *reservation_seq == reserved.seq
            && reserved.seq <= evidence.origin_seq
            && *reservation_hash == reserved.hash
            && *admission_decision_hash == *original.authorization.decision_hash()
            && *creator_incarnation_id == intent.incarnation_id
            && *reserved_snapshot_hash == original.snapshot.snapshot_hash()?
            && original.snapshot == expected,
    )?;
    Ok((reserved, original))
}

/// A full checked Store already validates every transition. This additional
/// prefix test makes the no-effect condition explicit for individual V3 reads:
/// the most recent earlier fact for this create must be the exact Reserve.
fn initial_prefix(
    db: &Connection,
    create_id: &str,
    before: i64,
    reserve_seq: i64,
) -> anyhow::Result<()> {
    let mut latest = None;
    let mut query = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.kubernetes.create.*' AND kind NOT IN ('control.kubernetes.create.genesis','control.kubernetes.create.policy.genesis','control.kubernetes.create.recovery.genesis') AND seq<?1 ORDER BY seq")?;
    for item in query.query_map([before], row_event)? {
        let stored = item?;
        let record = replay::record(&stored)?;
        if record.intent.create_id == create_id {
            latest = Some((stored.seq, matches!(record.change, Change::Reserve { .. })));
        }
    }
    permission(latest == Some((reserve_seq, true)))
}

pub(super) fn validate_historical_identity(
    db: &Connection,
    stored: &StoredEvent,
    record: &Record,
    tenant: &str,
    evidence: &AuthorizationEvidenceV3,
) -> anyhow::Result<()> {
    let identity = record.identity.identity()?;
    let at = parse_time(&record.received_at)?;
    permission(
        record.version == 3 && identity.tenant_id() == tenant && evidence.tenant_id == tenant,
    )?;
    validate_evidence(evidence, &identity, &record.intent, at)?;
    let (reserved, original) =
        validate_basis(db, evidence, &record.intent, tenant, Some(stored.seq))?;
    let Change::RecoverBeforeEmission { recovery } = &record.change else {
        return Err(KubernetesCreateFailureV1::PermissionDenied.into());
    };
    let RecoveryBasisV3::RecoverBeforeEmission {
        recovery_input_hash,
        ..
    } = &evidence.basis;
    permission(
        recovery.input_hash()? == *recovery_input_hash
            && record.input_hash == *recovery_input_hash
            && record.previous_revision == 1,
    )?;
    initial_prefix(db, &record.intent.create_id, stored.seq, reserved.seq)?;
    let expected = model::transition(
        &record.intent,
        Some(&original.snapshot),
        &record.change,
        None,
    )?;
    permission(record.snapshot == expected)?;
    let issued_at = parse_time(&evidence.issued_at)?;
    let before_origin = evidence
        .origin_seq
        .checked_add(1)
        .ok_or(KubernetesCreateFailureV1::InvalidRequest)?;
    workload::validate_broker_identity_at(db, &identity, at, stored.seq)?;
    workload::validate_broker_identity_at(db, &identity, issued_at, before_origin)?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    permission(namespace == record.intent.control_namespace_uuid)?;
    for before in [before_origin, stored.seq] {
        let binding = workload::prefix_binding(db, identity.credential_id(), before)?
            .ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
        let broker = broker_registry::broker_at(db, identity.broker_id(), before)?
            .ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
        permission(
            broker.incarnation_id == evidence.incarnation_id
                && broker.broker_id == record.intent.broker_id
                && broker.authorities.iter().any(|authority| {
                    authority.authority_id == record.intent.authority.authority_id().to_string()
                        && authority.runtime_kind == BrokerRuntimeKindV1::Kubernetes
                })
                && parse_time(&evidence.expires_at)? <= parse_time(&binding.expires_at)?
                && parse_time(&evidence.expires_at)? <= parse_time(&broker.not_after)?
                && parse_time(&binding.updated_at)? <= issued_at
                && parse_time(&broker.granted_at)? <= issued_at,
        )?;
    }
    Ok(())
}
