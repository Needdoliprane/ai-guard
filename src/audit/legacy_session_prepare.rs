//! Explicit persistence capability for nominative preparation on Legacy SQLite.
use super::{control::supervision, *};
use crate::{
    broker_registry::BrokerRuntimeKindV1,
    control::{RuntimeKindV1, session_control::ProvisionLegacySessionTargetV1},
    identity::{IdentityFailureV1, LegacyControlRoleV1, PrincipalRefV1},
};
use uuid::Uuid;

mod confirmation;
pub(crate) use confirmation::LegacySessionLocalProof;
pub(super) use confirmation::{REFERENCE_KINDS, protect_manual, validate_reference_event};

pub(super) const GENESIS_KIND: &str = "control.legacy_session_prepare.genesis";
pub(super) const TARGET_KIND: &str = "control.legacy_session_prepare.target";
const PROTOCOL: &str = "legacy_session_prepare_v1";
const META_TABLE: &str = "legacy_session_prepare_meta";
pub(super) const SCHEMA: [(&str, &str, &str); 1] = [(
    "table",
    META_TABLE,
    "CREATE TABLE legacy_session_prepare_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),genesis_seq INTEGER NOT NULL REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
)];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    protocol: String,
    tenant_id: String,
    namespace: String,
    human_rights_seq: i64,
    human_rights_hash: String,
    previous_seq: i64,
    previous_hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetRecord {
    version: u32,
    namespace: String,
    operator: PrincipalRefV1,
    command: ProvisionLegacySessionTargetV1,
}

#[derive(Serialize)]
pub(crate) struct LegacyTargetReceipt {
    version: u32,
    target_id: Uuid,
    target_revision: u64,
    audit_seq: i64,
    audit_hash: String,
    replayed: bool,
}

pub(super) fn format_admits(version: i64) -> bool {
    matches!(version, 10..=13)
}

pub(super) fn capability_present(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
        [GENESIS_KIND],
        |row| row.get(0),
    )?)
}

pub(super) fn validate_capability(db: &Connection, version: i64) -> anyhow::Result<()> {
    let mut query = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2",
    )?;
    let mut rows = query.query([GENESIS_KIND])?;
    let Some(row) = rows.next()? else {
        return Ok(());
    };
    let event = row_event(row)?;
    anyhow::ensure!(
        format_admits(version) && rows.next()?.is_none(),
        "invalid Legacy session prepare capability format or marker count"
    );
    let marker: Genesis = serde_json::from_value(event.event.data.clone())?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let rights = human_rights::genesis(db)?
        .context("Legacy session prepare requires human rights maintenance")?
        .0;
    let projection: (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM legacy_session_prepare_meta WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    anyhow::ensure!(
        marker.version == 1
            && marker.protocol == PROTOCOL
            && marker.tenant_id == tenant
            && marker.namespace == supervision::namespace(db)?
            && marker.human_rights_seq == rights.seq
            && marker.human_rights_hash == rights.hash
            && rights.seq < event.seq
            && marker.previous_seq == event.seq - 1
            && marker.previous_hash == event.prev_hash
            && event.event.kind == GENESIS_KIND
            && event.event.source == "gateway"
            && event.event.provenance == "observed"
            && event.event.coverage == "native"
            && event.event.session_id.is_none()
            && projection == (event.seq, event.hash),
        "invalid Legacy session prepare genesis"
    );
    Ok(())
}

fn target_event_id(target_id: Uuid, revision: u64) -> String {
    let domain = format!("ai-guard:legacy-session-target:v1:{revision}");
    session_prepare::bound_event_id(domain.as_bytes(), target_id)
}

fn event_by_id(db: &Connection, id: &str) -> anyhow::Result<Option<StoredEvent>> {
    Ok(db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [id],
            row_event,
        )
        .optional()?)
}

fn validate_target_broker(
    db: &Connection,
    command: &ProvisionLegacySessionTargetV1,
    at: chrono::DateTime<chrono::Utc>,
    before_seq: i64,
) -> anyhow::Result<()> {
    let target = &command.target;
    if !target.catalogue.active {
        return Ok(());
    }
    let broker = broker_registry::broker_at(db, &target.broker_id.to_string(), before_seq)?
        .ok_or(IdentityFailureV1::NotFound)?;
    let runtime = match target.launch.runtime_kind {
        RuntimeKindV1::Docker => BrokerRuntimeKindV1::Docker,
        RuntimeKindV1::Podman => BrokerRuntimeKindV1::Podman,
    };
    anyhow::ensure!(
        !broker.revoked
            && broker.incarnation_id == target.incarnation_id.to_string()
            && broker.grant_revision == target.broker_grant_revision
            && crate::broker_registry::parse_time(&broker.not_after)? > at
            && broker.authorities.iter().any(|authority| {
                authority.runtime_kind == runtime
                    && authority.authority_id == target.launch.authority_id
            }),
        IdentityFailureV1::Conflict
    );
    Ok(())
}

fn require_offline_operator(operator: &PrincipalRefV1) -> anyhow::Result<()> {
    operator.validate()?;
    anyhow::ensure!(
        matches!(
            operator,
            PrincipalRefV1::LegacyControl {
                role: LegacyControlRoleV1::Runner,
                ..
            }
        ),
        IdentityFailureV1::PermissionDenied
    );
    Ok(())
}

pub(super) fn validate_target_event(
    db: &Connection,
    version: i64,
    stored: &StoredEvent,
) -> anyhow::Result<()> {
    let record: TargetRecord = serde_json::from_value(stored.event.data.clone())?;
    let target = &record.command.target.catalogue;
    record.command.validate()?;
    require_offline_operator(&record.operator)?;
    let (marker_seq, tenant): (i64, String) = db.query_row(
        "SELECT m.genesis_seq,t.tenant_id FROM legacy_session_prepare_meta m JOIN tenant_binding t ON t.singleton=1 WHERE m.singleton=1",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    anyhow::ensure!(
        format_admits(version)
            && marker_seq < stored.seq
            && record.version == 1
            && record.namespace == supervision::namespace(db)?
            && target.tenant_id == tenant
            && stored.event.id == target_event_id(target.launch_target_id, target.revision)
            && stored.event.kind == TARGET_KIND
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "native"
            && stored.event.session_id.is_none(),
        "invalid Legacy session launch target audit"
    );
    if record.command.expected_revision > 0 {
        let prior = event_by_id(
            db,
            &target_event_id(target.launch_target_id, record.command.expected_revision),
        )?
        .context("Legacy session launch target revision has no predecessor")?;
        anyhow::ensure!(
            prior.event.kind == TARGET_KIND && prior.seq < stored.seq,
            "invalid target revision order"
        );
    }
    validate_target_broker(
        db,
        &record.command,
        crate::broker_registry::parse_time(&stored.event.timestamp)?,
        stored.seq,
    )
}

fn target_receipt(
    target_id: Uuid,
    target_revision: u64,
    event: StoredEvent,
    replayed: bool,
) -> LegacyTargetReceipt {
    LegacyTargetReceipt {
        version: 1,
        target_id,
        target_revision,
        audit_seq: event.seq,
        audit_hash: event.hash,
        replayed,
    }
}

impl Store {
    pub(super) fn initialize_legacy_session_prepare(
        &self,
        tx: &CheckedTransaction<'_>,
    ) -> anyhow::Result<StoredEvent> {
        let rights = human_rights::genesis(tx)?
            .context("Legacy session prepare requires human rights maintenance")?
            .0;
        let (previous_seq, previous_hash) = audit_head(tx)?;
        let clock = workload::capture_control_time(tx)?;
        let mut event = Event::new(GENESIS_KIND);
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.data = serde_json::to_value(Genesis {
            version: 1,
            protocol: PROTOCOL.into(),
            tenant_id: self.tenant_id.clone(),
            namespace: supervision::namespace(tx)?,
            human_rights_seq: rights.seq,
            human_rights_hash: rights.hash,
            previous_seq,
            previous_hash,
        })?;
        let expected = event.data.clone();
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            event.data == expected,
            "Legacy session prepare capability requires redaction"
        );
        for (_, _, sql) in SCHEMA {
            tx.execute_batch(sql)?;
        }
        let stored = self.append_authorized_in_transaction(tx, event, payload, &clock)?;
        tx.execute(
            "INSERT INTO legacy_session_prepare_meta(singleton,genesis_seq,genesis_hash) VALUES(1,?1,?2)",
            params![stored.seq, stored.hash],
        )?;
        Ok(stored)
    }

    /// Called by the offline filesystem-authorized target provisioning command.
    pub(crate) fn provision_legacy_session_target(
        &self,
        operator: PrincipalRefV1,
        command: ProvisionLegacySessionTargetV1,
    ) -> anyhow::Result<LegacyTargetReceipt> {
        command.validate()?;
        require_offline_operator(&operator)?;
        let catalogue = &command.target.catalogue;
        anyhow::ensure!(
            catalogue.tenant_id == self.tenant_id,
            IdentityFailureV1::PermissionDenied
        );
        let target_id = catalogue.launch_target_id;
        let revision = catalogue.revision;
        let id = target_event_id(target_id, revision);
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        anyhow::ensure!(
            capability_present(&tx)?,
            IdentityFailureV1::MigrationRequired
        );
        anyhow::ensure!(!tx.lease.active, IdentityFailureV1::PermissionDenied);
        let record = TargetRecord {
            version: 1,
            namespace: supervision::namespace(&tx)?,
            operator,
            command,
        };
        let expected = serde_json::to_value(&record)?;
        if let Some(stored) = event_by_id(&tx, &id)? {
            anyhow::ensure!(
                stored.event.kind == TARGET_KIND && stored.event.data == expected,
                IdentityFailureV1::Conflict
            );
            self.commit_checked(tx)?;
            return Ok(target_receipt(target_id, revision, stored, true));
        }
        let clock = workload::capture_control_time(&tx)?;
        let mut event = Event::new(TARGET_KIND);
        event.id = id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.data = expected.clone();
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            event.data == expected,
            "Legacy session launch target requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        let version = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        validate_target_event(&tx, version, &stored)?;
        self.commit_authorized(tx, &clock)?;
        Ok(target_receipt(target_id, revision, stored, false))
    }
}
