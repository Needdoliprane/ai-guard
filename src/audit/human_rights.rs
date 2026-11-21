//! Explicit human rights capability marker (`human_rights_v2`).
//!
//! The marker is an identity-chain event, not a store format number: identity
//! replay refuses every unknown `control.identity.*` kind, so a binary that
//! predates the marker fails closed on a store that carries it. The marker is
//! appended only by the explicit, backed-up offline maintenance, and only on a
//! store whose identity chain already carries the worker-access marker. It
//! admits the capabilities of `HumanCapabilityV1::requires_human_rights_marker`
//! and the human read actions of `read_action_requires_marker`; it grants no
//! permission, context, read or mutation by itself.
use super::*;
use crate::identity::*;

pub(super) const GENESIS_KIND: &str = "control.identity.human_rights.genesis";
/// The only versioned constant of this fence. It never changes `user_version`.
pub(crate) const MARKER_VERSION: u32 = 2;
const PROTOCOL: &str = "human_rights_v2";

/// Store formats on which the worker-access marker, hence this one, may exist.
pub(super) fn format_admits_marker(version: i64) -> bool {
    matches!(version, 10..=15)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    protocol: String,
    control_namespace_uuid: String,
    identity_genesis_seq: i64,
    identity_genesis_hash: String,
    workers_access_seq: i64,
    authorization_revision: u64,
    previous_seq: i64,
    previous_hash: String,
}

/// Human read actions recorded under `control.identity.access` that may exist
/// only after the marker. The existing actions keep their historical meaning.
pub(crate) fn read_action_requires_marker(action: &str) -> bool {
    matches!(
        action,
        "summary" | "memberships" | "quota" | "usage" | "configuration"
    )
}

/// The validated marker, or `None` when the chain does not carry it. More than
/// one marker, or one that differs from its durable prefix, is invalid.
pub(super) fn genesis(db: &Connection) -> anyhow::Result<Option<(StoredEvent, u64)>> {
    let records = {
        let mut statement = db.prepare(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2",
        )?;
        statement
            .query_map([GENESIS_KIND], row_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    anyhow::ensure!(records.len() <= 1, "invalid human rights marker count");
    let Some(stored) = records.into_iter().next() else {
        return Ok(None);
    };
    let marker: Genesis = serde_json::from_value(stored.event.data.clone())?;
    let (identity_seq, identity_hash): (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM identity_meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let workers: Option<i64> = db.query_row(
        "SELECT MAX(seq) FROM events WHERE kind=?1",
        [super::human_workers::GENESIS_KIND],
        |r| r.get(0),
    )?;
    anyhow::ensure!(
        marker.version == MARKER_VERSION
            && marker.protocol == PROTOCOL
            && marker.control_namespace_uuid == namespace
            && marker.identity_genesis_seq == identity_seq
            && marker.identity_genesis_hash == identity_hash
            && workers == Some(marker.workers_access_seq)
            && stored.seq > marker.workers_access_seq
            && marker.previous_seq == stored.seq - 1
            && marker.previous_hash == stored.prev_hash
            && i64::try_from(marker.authorization_revision).is_ok()
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "observed",
        "invalid human rights marker"
    );
    Ok(Some((stored, marker.authorization_revision)))
}

/// Schema validation: the marker may only exist on a format that carries the
/// worker-access marker, and it must match its durable prefix there.
pub(super) fn validate_capability(db: &Connection, version: i64) -> anyhow::Result<()> {
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
        [GENESIS_KIND],
        |r| r.get(0),
    )?;
    if present {
        anyhow::ensure!(
            format_admits_marker(version),
            "human rights marker requires the worker access marker"
        );
        genesis(db)?;
    }
    Ok(())
}

pub(super) fn installed(db: &Connection) -> anyhow::Result<bool> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(format_admits_marker(version) && genesis(db)?.is_some())
}

/// Format fence for a membership mutation; never an authorization decision.
pub(super) fn require_mutation_capability(
    input: &IdentityMutationV1,
    installed: bool,
) -> anyhow::Result<()> {
    if !installed
        && matches!(&input.change, IdentityChangeV1::Membership { membership, .. }
            if membership.capabilities.iter().any(|c| c.requires_human_rights_marker()))
    {
        return Err(IdentityFailureV1::MigrationRequired.into());
    }
    Ok(())
}

impl Store {
    /// Whether this store's identity chain carries the validated marker.
    pub fn human_rights_installed(&self) -> anyhow::Result<bool> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        installed(&db)
    }
    pub(super) fn initialize_human_rights(&self, tx: &Transaction<'_>) -> anyhow::Result<()> {
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        anyhow::ensure!(
            format_admits_marker(version),
            "human rights marker requires the worker access marker"
        );
        crate::control::invalid(genesis(tx)?.is_none())?;
        let (identity_genesis_seq, identity_genesis_hash): (i64, String) = tx.query_row(
            "SELECT genesis_seq,genesis_hash FROM identity_meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let control_namespace_uuid = tx.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let workers_access_seq: Option<i64> = tx.query_row(
            "SELECT MAX(seq) FROM events WHERE kind=?1",
            [super::human_workers::GENESIS_KIND],
            |r| r.get(0),
        )?;
        let workers_access_seq = workers_access_seq.ok_or_else(|| {
            anyhow::anyhow!("human rights marker requires the worker access marker")
        })?;
        let (previous_seq, previous_hash) = audit_head(tx)?;
        let mut event = Event::new(GENESIS_KIND);
        event.data = serde_json::to_value(Genesis {
            version: MARKER_VERSION,
            protocol: PROTOCOL.into(),
            control_namespace_uuid,
            identity_genesis_seq,
            identity_genesis_hash,
            workers_access_seq,
            authorization_revision: identity::authorization_revision(tx)?,
            previous_seq,
            previous_hash,
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "human rights marker requires redaction"
        );
        self.append_identity_in_transaction(tx, event, payload)?;
        Ok(())
    }
}
