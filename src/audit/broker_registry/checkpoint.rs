//! Schema12 descriptive checkpoints use the registry's checked operations and
//! mandatory Broker audit budget; no parallel SQL projection is introduced.
use super::*;

pub(super) fn require_schema12(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if matches!(version, 12 | 13) {
        Ok(())
    } else {
        Err(BrokerRegistryFailureV1::MigrationRequired.into())
    }
}

struct Retained {
    stored: StoredEvent,
    record: Record,
    broker: BrokerV1,
}
fn latest(db: &Connection, broker: &str) -> anyhow::Result<Option<Retained>> {
    let row = db.query_row(
        "SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload,o.result_json FROM broker_registry_operations o JOIN events e ON e.seq=o.event_seq WHERE o.broker_id=?1 AND e.kind='control.broker_registry.checkpoint' ORDER BY o.event_seq DESC LIMIT 1",
        [broker],
        |row| Ok((row_event(row)?, row.get::<_, String>(5)?)),
    ).optional()?;
    row.map(|(stored, broker)| {
        let record: Record = serde_json::from_value(stored.event.data.clone())?;
        require(
            record.version == 2
                && matches!(record.change, Change::Checkpoint { .. })
                && record.input_hash == record.change.input_hash()?,
        )?;
        record.change.validate()?;
        Ok(Retained {
            stored,
            record,
            broker: serde_json::from_str(&broker)?,
        })
    })
    .transpose()
}
pub(super) fn latest_publication(
    db: &Connection,
    broker: &str,
) -> anyhow::Result<Option<(BrokerCheckpointV1, String)>> {
    latest(db, broker)?
        .map(|retained| {
            let Change::Checkpoint { checkpoint, .. } = retained.record.change else {
                return Err(BrokerRegistryFailureV1::InvalidRequest.into());
            };
            Ok((checkpoint, retained.record.received_at))
        })
        .transpose()
}
fn observation(
    retained: Retained,
    current: &BrokerV1,
    evaluated_at: &str,
) -> anyhow::Result<BrokerCheckpointObservationV1> {
    let Change::Checkpoint {
        identity,
        checkpoint,
    } = &retained.record.change
    else {
        return Err(BrokerRegistryFailureV1::InvalidRequest.into());
    };
    let received = parse_time(&retained.record.received_at)?;
    let now = parse_time(evaluated_at)?;
    let expires = received
        .checked_add_signed(Duration::seconds(i64::from(retained.broker.lease_seconds)))
        .context("checkpoint lease overflow")?
        .min(parse_time(&retained.broker.not_after)?);
    let in_current_grant = !current.revoked
        && identity.grant_revision == current.grant_revision
        && checkpoint.incarnation_id == current.incarnation_id
        && current.authorities.contains(&checkpoint.snapshot.authority);
    let freshness = if !in_current_grant {
        BrokerCheckpointFreshnessV1::Historical
    } else if now < received {
        BrokerCheckpointFreshnessV1::ClockUncertain
    } else if now >= expires {
        BrokerCheckpointFreshnessV1::Stale
    } else {
        BrokerCheckpointFreshnessV1::Current
    };
    let receipt = result(
        &retained.stored,
        retained.broker,
        retained.record.input_hash.clone(),
        checkpoint.operation_id.clone(),
        false,
    )
    .receipt;
    Ok(BrokerCheckpointObservationV1 {
        checkpoint: checkpoint.clone(),
        receipt,
        received_at: retained.record.received_at,
        lease_expires_at: format_time(expires),
        in_current_grant,
        freshness,
        coverage: BrokerCheckpointCoverageV1::DeclaredPartial,
        publication_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    })
}

impl Store {
    /// The DTO is only data: the current Broker proof selects this tenant,
    /// broker, binding and grant, including before an exact operation replay.
    pub fn broker_checkpoint(
        &self,
        identity: &BrokerIdentityV1,
        checkpoint: &BrokerCheckpointV1,
    ) -> anyhow::Result<BrokerCheckpointResultV1> {
        checkpoint.validate()?;
        let result = self.persist_broker_change(
            Change::Checkpoint {
                identity: IdentityEvidence::capture(identity),
                checkpoint: checkpoint.clone(),
            },
            Some(identity),
        )?;
        Ok(BrokerCheckpointResultV1 {
            checkpoint: checkpoint.clone(),
            receipt: result.receipt,
            replayed: result.replayed,
        })
    }

    /// Compute a commitment from checked public identity data. This does not
    /// construct a Broker proof or authorize a POST/replay.
    pub(crate) fn broker_checkpoint_input_hash(
        identity: &BrokerIdentityViewV1,
        checkpoint: &BrokerCheckpointV1,
    ) -> anyhow::Result<String> {
        checkpoint.validate()?;
        crate::tenancy::validate_tenant_id(&identity.tenant_id)?;
        registry_uuid(&identity.broker_id)?;
        registry_uuid(&identity.credential_id)?;
        registry_uuid(&identity.incarnation_id)?;
        registry_uuid(&identity.control_namespace_uuid)?;
        revision(identity.binding_revision)?;
        revision(identity.grant_revision)?;
        require(
            identity.version == 1
                && matches!(identity.schema_version, 12 | 13)
                && identity.binding_revision > 0
                && identity.grant_revision > 0
                && identity.incarnation_id == checkpoint.incarnation_id,
        )?;
        Change::Checkpoint {
            identity: IdentityEvidence {
                tenant_id: identity.tenant_id.clone(),
                broker_id: identity.broker_id.clone(),
                credential_id: identity.credential_id.clone(),
                binding_revision: identity.binding_revision,
                grant_revision: identity.grant_revision,
            },
            checkpoint: checkpoint.clone(),
        }
        .input_hash()
    }

    /// Tenant-local read for an already authorized control/API caller. A stale
    /// or revoked broker retains historical data; absence is explicit None.
    pub fn broker_checkpoint_view(
        &self,
        broker_id: &str,
    ) -> anyhow::Result<BrokerCheckpointViewV1> {
        registry_uuid(broker_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema12(&tx)?;
        let broker = load_broker(&tx, broker_id)?.ok_or(BrokerRegistryFailureV1::NotFound)?;
        let evaluated_at = format_time(Utc::now());
        let checkpoint = latest(&tx, broker_id)?
            .map(|r| observation(r, &broker, &evaluated_at))
            .transpose()?;
        let view = BrokerCheckpointViewV1 {
            version: 1,
            broker: broker.view_at(&evaluated_at)?,
            checkpoint,
        };
        let value = serde_json::to_value(&view)?;
        require(self.redactor.value_unclipped(&value) == value)?;
        require(serde_json::to_vec(&view)?.len() <= MAX_BROKER_CHECKPOINT_BYTES)?;
        self.commit_checked(tx)?;
        Ok(view)
    }

    pub fn broker_checkpoint_for_identity(
        &self,
        identity: &BrokerIdentityV1,
    ) -> anyhow::Result<BrokerCheckpointViewV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema12(&tx)?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let clock = super::super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        super::super::workload::revalidate_broker_identity(&tx, identity, now)?;
        let broker = load_broker(&tx, identity.broker_id())?
            .ok_or(BrokerRegistryFailureV1::PermissionDenied)?;
        let evaluated_at = format_time(now);
        broker.require_current_grant(identity, &evaluated_at)?;
        let checkpoint = latest(&tx, identity.broker_id())?
            .map(|r| observation(r, &broker, &evaluated_at))
            .transpose()?;
        let view = BrokerCheckpointViewV1 {
            version: 1,
            broker: broker.view_at(&evaluated_at)?,
            checkpoint,
        };
        let value = serde_json::to_value(&view)?;
        require(self.redactor.value_unclipped(&value) == value)?;
        require(serde_json::to_vec(&view)?.len() <= MAX_BROKER_CHECKPOINT_BYTES)?;
        self.commit_authorized(tx, &clock)?;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn checkpoint_freshness_expires_without_heartbeat_refresh_or_runtime_claim() {
        let id = || Uuid::new_v4().to_string();
        let broker = BrokerV1 {
            version: 1,
            broker_id: id(),
            grant_revision: 1,
            heartbeat_revision: 0,
            credential_id: id(),
            incarnation_id: id(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "declared-host".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
            revoked: false,
            granted_at: "2026-11-09T10:00:00.000Z".into(),
            revoked_at: None,
            last_heartbeat: None,
        };
        let publication = BrokerCheckpointV1 {
            version: 1,
            operation_id: id(),
            incarnation_id: broker.incarnation_id.clone(),
            previous_sequence: 0,
            sequence: 1,
            snapshot: BrokerCheckpointSnapshotV1 {
                version: 1,
                authority: broker.authorities[0].clone(),
                jobs: vec![],
                has_more: false,
            },
        };
        let change = Change::Checkpoint {
            identity: IdentityEvidence {
                tenant_id: "tenant-a".into(),
                broker_id: broker.broker_id.clone(),
                credential_id: broker.credential_id.clone(),
                binding_revision: 1,
                grant_revision: 1,
            },
            checkpoint: publication,
        };
        let record = Record {
            version: 2,
            input_hash: change.input_hash().unwrap(),
            received_at: "2026-11-09T10:00:10.000Z".into(),
            change,
        };
        for (at, expected) in [
            (
                "2026-11-09T10:00:11.000Z",
                BrokerCheckpointFreshnessV1::Current,
            ),
            (
                "2026-11-09T10:00:40.000Z",
                BrokerCheckpointFreshnessV1::Stale,
            ),
            (
                "2026-11-09T10:00:09.000Z",
                BrokerCheckpointFreshnessV1::ClockUncertain,
            ),
        ] {
            let mut event = Event::new("control.broker_registry.checkpoint");
            event.data = serde_json::to_value(&record).unwrap();
            let stored = StoredEvent {
                seq: 3,
                received_at: record.received_at.clone(),
                prev_hash: "a".repeat(64),
                hash: "b".repeat(64),
                event,
            };
            let retained = Retained {
                stored,
                record: serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap(),
                broker: broker.clone(),
            };
            let view = observation(retained, &broker, at).unwrap();
            assert_eq!(view.freshness, expected);
            assert_eq!(view.coverage, BrokerCheckpointCoverageV1::DeclaredPartial);
            assert_eq!(
                view.publication_provenance,
                BrokerClaimProvenanceV1::Declared
            );
            assert!(broker.last_heartbeat.is_none());
        }
    }
}
