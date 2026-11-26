//! Explicit persistence capability for nominative preparation on Legacy SQLite.
use super::*;

pub(super) const GENESIS_KIND: &str = "control.legacy_session_prepare.genesis";
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
}
