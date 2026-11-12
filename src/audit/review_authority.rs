//! Exact source-anchor/incarnation replay for the modern tenant format.
//! A marker is descriptive. Only the retained token plus an epoch and this
//! transaction's verified backing/replay can produce a retained live reference.
use super::*;
use crate::human_authority::{
    HumanAuthorityInvalidationReason, HumanSourceKindV1, HumanSourcesWriteEpoch, OwnedSourceToken,
};

pub(super) const FORMAT: u32 = 14;
pub(super) const SOURCE_PROTOCOL: &str = "human_source_authority_v1";
pub(super) const ANCHOR_KIND: &str = "control.review_authority.source_anchor";
pub(super) const INCARNATION_KIND: &str = "control.review_authority.incarnation";
pub(super) const SCHEMA: [(&str, &str, &str); 1] = [(
    "table",
    "review_authority_meta",
    "CREATE TABLE review_authority_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),source_id TEXT NOT NULL UNIQUE,anchor_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),anchor_hash TEXT NOT NULL,incarnation_seq INTEGER UNIQUE REFERENCES events(seq),incarnation_hash TEXT,incarnation_revision INTEGER NOT NULL CHECK(incarnation_revision>=0),snapshot_json TEXT NOT NULL,snapshot_hash TEXT NOT NULL)",
)];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 1] = [("review_authority_meta", 3)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewAuthorityFailure {
    Required,
    Mismatch,
    Unavailable,
    ForeignTransaction,
}
impl std::fmt::Display for ReviewAuthorityFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Required => "retained human source authority required",
            Self::Mismatch => "human source authority differs from durable source",
            Self::Unavailable => "human source authority unavailable",
            Self::ForeignTransaction => "SQLite transaction does not name the retained main source",
        })
    }
}
impl std::error::Error for ReviewAuthorityFailure {}
fn exact(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ReviewAuthorityFailure::Mismatch.into())
    }
}
fn id(value: uuid::Uuid) -> anyhow::Result<()> {
    exact(!value.is_nil())
}
fn hash(value: &str) -> anyhow::Result<()> {
    exact(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthorityReceiptV1 {
    pub(crate) seq: i64,
    pub(crate) hash: String,
}
impl AuthorityReceiptV1 {
    fn validate(&self) -> anyhow::Result<()> {
        exact(self.seq > 0)?;
        hash(&self.hash)
    }
}

/// No closure digest here: this event's receipt goes into the private manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceAnchorV1 {
    pub(super) version: u32,
    pub(super) protocol: String,
    pub(super) from_format: u32,
    pub(super) format: u32,
    pub(super) migration_id: uuid::Uuid,
    pub(super) installation_id: uuid::Uuid,
    pub(super) source_id: uuid::Uuid,
    pub(super) kind: HumanSourceKindV1,
    pub(super) tenant_id: String,
    pub(super) durable_namespace_id: uuid::Uuid,
    pub(super) previous_seq: i64,
    pub(super) previous_hash: String,
}
impl SourceAnchorV1 {
    fn validate(&self) -> anyhow::Result<()> {
        exact(
            self.version == 1
                && self.protocol == SOURCE_PROTOCOL
                && self.from_format == 13
                && self.format == FORMAT
                && self.kind == HumanSourceKindV1::TenantStore
                && self.previous_seq > 0,
        )?;
        for value in [
            self.migration_id,
            self.installation_id,
            self.source_id,
            self.durable_namespace_id,
        ] {
            id(value)?;
        }
        crate::tenancy::validate_tenant_id(&self.tenant_id)
            .map_err(|_| ReviewAuthorityFailure::Mismatch)?;
        hash(&self.previous_hash)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceIncarnationV1 {
    version: u32,
    protocol: String,
    anchor: AuthorityReceiptV1,
    closure_id: uuid::Uuid,
    closure_revision: u64,
    closure_digest: String,
    control_config_public_digest: String,
    owner_incarnation: uuid::Uuid,
    revision: u64,
    previous_incarnation: Option<AuthorityReceiptV1>,
    previous_seq: i64,
    previous_hash: String,
}
impl SourceIncarnationV1 {
    fn validate(&self) -> anyhow::Result<()> {
        exact(
            self.version == 1
                && self.protocol == SOURCE_PROTOCOL
                && self.closure_revision > 0
                && self.closure_revision <= i64::MAX as u64
                && self.revision > 0
                && self.revision <= i64::MAX as u64
                && self.previous_seq > 0,
        )?;
        id(self.closure_id)?;
        id(self.owner_incarnation)?;
        self.anchor.validate()?;
        hash(&self.closure_digest)?;
        hash(&self.control_config_public_digest)?;
        hash(&self.previous_hash)?;
        if let Some(previous) = &self.previous_incarnation {
            previous.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "record_type",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum AuthorityRecordV1 {
    SourceAnchor(SourceAnchorV1),
    Incarnation(SourceIncarnationV1),
}
impl AuthorityRecordV1 {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::SourceAnchor(value) => value.validate()?,
            Self::Incarnation(value) => value.validate()?,
        };
        exact(serde_json::to_vec(self)?.len() <= 16 * 1024)
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::SourceAnchor(_) => ANCHOR_KIND,
            Self::Incarnation(_) => INCARNATION_KIND,
        }
    }
    fn previous(&self) -> (i64, &str) {
        match self {
            Self::SourceAnchor(v) => (v.previous_seq, &v.previous_hash),
            Self::Incarnation(v) => (v.previous_seq, &v.previous_hash),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoritySnapshot {
    pub(super) anchor: SourceAnchorV1,
    pub(super) anchor_receipt: AuthorityReceiptV1,
    incarnation: Option<(SourceIncarnationV1, AuthorityReceiptV1)>,
}
impl AuthoritySnapshot {
    /// Descriptive history only; the snapshot is not retained source authority.
    pub(super) fn incarnation_revision(&self) -> u64 {
        self.incarnation
            .as_ref()
            .map_or(0, |(record, _)| record.revision)
    }

    pub(super) fn incarnation_receipt(&self) -> Option<&AuthorityReceiptV1> {
        self.incarnation.as_ref().map(|(_, receipt)| receipt)
    }

    /// Startup precheck for an expected absent/previous owner incarnation.
    /// Retention still independently verifies the full source and transaction.
    pub(super) fn require_current_incarnation_for_startup(
        &self,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<()> {
        let Some((incarnation, _)) = self.incarnation.as_ref() else {
            return Err(ReviewAuthorityFailure::Required.into());
        };
        let closure = source.closure();
        if incarnation.owner_incarnation != source.incarnation()
            || incarnation.closure_id != closure.closure_id
            || incarnation.closure_revision != closure.closure_revision
            || incarnation.closure_digest != closure.closure_digest
            || incarnation.control_config_public_digest
                != closure.configuration.public_config_digest
        {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        Ok(())
    }
}
#[derive(Default)]
struct AuthorityProjection {
    snapshot: Option<AuthoritySnapshot>,
}
impl AuthorityProjection {
    fn preflight(&self, record: &AuthorityRecordV1) -> anyhow::Result<()> {
        record.validate()?;
        match record {
            AuthorityRecordV1::SourceAnchor(_) => exact(self.snapshot.is_none()),
            AuthorityRecordV1::Incarnation(next) => {
                let current = self
                    .snapshot
                    .as_ref()
                    .ok_or(ReviewAuthorityFailure::Mismatch)?;
                let (revision, previous) = current
                    .incarnation
                    .as_ref()
                    .map_or((0, None), |(value, receipt)| {
                        (value.revision, Some(receipt))
                    });
                exact(
                    next.anchor == current.anchor_receipt
                        && revision.checked_add(1) == Some(next.revision)
                        && next.previous_incarnation.as_ref() == previous,
                )
            }
        }
    }
    fn apply(
        &mut self,
        record: AuthorityRecordV1,
        receipt: AuthorityReceiptV1,
    ) -> anyhow::Result<()> {
        self.preflight(&record)?;
        receipt.validate()?;
        exact(record.previous().0.checked_add(1) == Some(receipt.seq))?;
        match record {
            AuthorityRecordV1::SourceAnchor(anchor) => {
                self.snapshot = Some(AuthoritySnapshot {
                    anchor,
                    anchor_receipt: receipt,
                    incarnation: None,
                })
            }
            AuthorityRecordV1::Incarnation(value) => {
                self.snapshot
                    .as_mut()
                    .ok_or(ReviewAuthorityFailure::Mismatch)?
                    .incarnation = Some((value, receipt))
            }
        }
        Ok(())
    }
}
fn decode(stored: &StoredEvent) -> anyhow::Result<AuthorityRecordV1> {
    let event = &stored.event;
    exact(
        event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "native"
            && event.session_id.is_none()
            && event.project_id.is_none()
            && event.sandbox_id.is_none()
            && event.agent_id.is_none()
            && event.parent_agent_id.is_none()
            && event.correlation_id.is_none()
            && event.trace_id.is_none()
            && event.command.is_none()
            && event.decision.is_none()
            && event.result.is_none()
            && serde_json::to_vec(&event.data)?.len() <= 16 * 1024,
    )?;
    let record: AuthorityRecordV1 =
        serde_json::from_value(event.data.clone()).map_err(|_| ReviewAuthorityFailure::Mismatch)?;
    record.validate()?;
    let previous = record.previous();
    exact(
        event.kind == record.kind()
            && previous.0.checked_add(1) == Some(stored.seq)
            && previous.1 == stored.prev_hash,
    )?;
    if let AuthorityRecordV1::SourceAnchor(anchor) = &record {
        exact(event.id == anchor.migration_id.to_string())?;
    }
    exact(
        crate::broker_registry::parse_time(&event.timestamp)?
            <= crate::broker_registry::parse_time(&stored.received_at)?,
    )?;
    Ok(record)
}
pub(super) fn marker(event: &Event) -> anyhow::Result<()> {
    let value: AuthorityRecordV1 =
        serde_json::from_value(event.data.clone()).map_err(|_| ReviewAuthorityFailure::Mismatch)?;
    value.validate()?;
    exact(event.kind == value.kind())
}
pub(super) fn namespace(db: &Connection) -> anyhow::Result<uuid::Uuid> {
    let value: String = db
        .query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(ReviewAuthorityFailure::Mismatch)?;
    let parsed = uuid::Uuid::parse_str(&value).map_err(|_| ReviewAuthorityFailure::Mismatch)?;
    id(parsed)?;
    exact(parsed.to_string() == value)?;
    Ok(parsed)
}
fn replay_events(db: &Connection) -> anyhow::Result<AuthorityProjection> {
    let mut query=db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.review_authority.*' ORDER BY seq")?;
    let mut result = AuthorityProjection::default();
    for stored in query.query_map([], row_event)? {
        let stored = stored?;
        result.apply(
            decode(&stored)?,
            AuthorityReceiptV1 {
                seq: stored.seq,
                hash: stored.hash,
            },
        )?;
    }
    Ok(result)
}
fn projection_row(snapshot: &AuthoritySnapshot) -> anyhow::Result<Vec<rusqlite::types::Value>> {
    use rusqlite::types::Value as V;
    let json = serde_json::to_string(snapshot)?;
    let (seq, hash, revision) = match &snapshot.incarnation {
        Some((value, receipt)) => (
            V::Integer(receipt.seq),
            V::Text(receipt.hash.clone()),
            i64::try_from(value.revision)?,
        ),
        None => (V::Null, V::Null, 0),
    };
    Ok(vec![
        V::Integer(1),
        V::Text(snapshot.anchor.source_id.to_string()),
        V::Integer(snapshot.anchor_receipt.seq),
        V::Text(snapshot.anchor_receipt.hash.clone()),
        seq,
        hash,
        V::Integer(revision),
        V::Text(json),
        V::Text(crate::control::content_hash(
            "ai-guard:review-authority-projection:v1",
            snapshot,
        )?),
    ])
}
fn verify_projection(db: &Connection, snapshot: &AuthoritySnapshot) -> anyhow::Result<()> {
    let mut query=db.prepare("SELECT singleton,source_id,anchor_seq,anchor_hash,incarnation_seq,incarnation_hash,incarnation_revision,snapshot_json,snapshot_hash FROM review_authority_meta ORDER BY singleton")?;
    let actual = query
        .query_map([], |row| {
            (0..9)
                .map(|i| row.get(i))
                .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    exact(actual == vec![projection_row(snapshot)?])
}
fn write_projection(db: &Connection, snapshot: &AuthoritySnapshot) -> anyhow::Result<()> {
    let changed=db.execute("INSERT INTO review_authority_meta(singleton,source_id,anchor_seq,anchor_hash,incarnation_seq,incarnation_hash,incarnation_revision,snapshot_json,snapshot_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(singleton) DO UPDATE SET source_id=excluded.source_id,anchor_seq=excluded.anchor_seq,anchor_hash=excluded.anchor_hash,incarnation_seq=excluded.incarnation_seq,incarnation_hash=excluded.incarnation_hash,incarnation_revision=excluded.incarnation_revision,snapshot_json=excluded.snapshot_json,snapshot_hash=excluded.snapshot_hash",rusqlite::params_from_iter(projection_row(snapshot)?))?;
    exact(changed == 1)
}
pub(super) fn replay(db: &Connection) -> anyhow::Result<AuthoritySnapshot> {
    let format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    exact(format == i64::from(FORMAT))?;
    let snapshot = replay_events(db)?
        .snapshot
        .ok_or(ReviewAuthorityFailure::Mismatch)?;
    let tenant: String = db
        .query_row(
            "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(ReviewAuthorityFailure::Mismatch)?;
    exact(
        snapshot.anchor.tenant_id == tenant
            && snapshot.anchor.durable_namespace_id == namespace(db)?,
    )?;
    verify_projection(db, &snapshot)?;
    Ok(snapshot)
}
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if format < 14 {
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM events WHERE kind GLOB 'control.review_authority.*'",
            [],
            |r| r.get(0),
        )?;
        exact(count == 0)
    } else {
        replay(db).map(|_| ())
    }
}
pub(super) fn append_record(
    store: &Store,
    tx: &Transaction<'_>,
    operation_id: uuid::Uuid,
    record: AuthorityRecordV1,
    clock: Option<&workload::AuthorizedControlTimeV1>,
) -> anyhow::Result<AuthorityReceiptV1> {
    id(operation_id)?;
    let format: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    exact(format == 14)?;
    if let AuthorityRecordV1::SourceAnchor(anchor) = &record {
        exact(anchor.migration_id == operation_id)?;
    }
    let mut projection = replay_events(tx)?;
    projection.preflight(&record)?;
    let prefix = audit_head(tx)?;
    exact(record.previous().0 == prefix.0 && record.previous().1 == prefix.1)?;
    let mut event = Event::new(record.kind());
    event.id = operation_id.to_string();
    event.coverage = "native".into();
    event.data = serde_json::to_value(&record)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    exact(serde_json::to_value(&event)? == original)?;
    let stored = match clock {
        Some(clock) => {
            store.append_authorized_identity_in_transaction(tx, event, payload, clock)?
        }
        None => store.append_in_transaction(tx, event, payload)?,
    };
    let receipt = AuthorityReceiptV1 {
        seq: stored.seq,
        hash: stored.hash.clone(),
    };
    projection.apply(decode(&stored)?, receipt.clone())?;
    write_projection(
        tx,
        projection
            .snapshot
            .as_ref()
            .ok_or(ReviewAuthorityFailure::Mismatch)?,
    )?;
    replay(tx)?;
    Ok(receipt)
}

/// Retains the actual owner's token. Never serializable and never a Ready flag.
#[derive(Clone)]
pub(crate) struct RetainedReviewAuthority {
    source: OwnedSourceToken,
    snapshot: AuthoritySnapshot,
}
impl std::fmt::Debug for RetainedReviewAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedReviewAuthority(<private retained source>)")
    }
}
/// Bind this exact connection's main filename/context to the retained private
/// backing, in addition to the existing named-entry and VFS HasMoved checks.
/// Only the file-backed main opened by our authorized SQLite paths is admitted:
/// an initialized TEMP namespace is allowed only while it has no objects.
/// Attached schemas, TEMP objects, unknown/empty/noncanonical main filenames and
/// disagreement between sqlite3_db_filename and database_list refuse as foreign
/// arguments.
/// This is not an attestation of arbitrary malicious replacement VFS code.
pub(super) fn verify_connection_main(
    backing: &StoreStorage,
    db: &Connection,
) -> anyhow::Result<()> {
    backing.verify(db)?;
    let foreign = || ReviewAuthorityFailure::ForeignTransaction;
    let filename = db
        .path()
        .filter(|path| !path.is_empty())
        .ok_or_else(foreign)?;
    let filename = std::path::Path::new(filename);
    if !filename.is_absolute() {
        return Err(foreign().into());
    }
    let canonical = crate::control_fs::absolute_path(filename).map_err(|_| foreign())?;
    if canonical.as_path() != filename || canonical.as_path() != backing.path() {
        return Err(foreign().into());
    }
    let mut query = db.prepare("PRAGMA database_list")?;
    let schemas = query
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .take(3)
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_temp = match schemas.as_slice() {
        [(0, main, _)] if main == "main" => false,
        [(0, main, _), (1, temp, filename)]
            if main == "main" && temp == "temp" && filename.is_empty() =>
        {
            true
        }
        _ => return Err(foreign().into()),
    };
    let listed = std::path::Path::new(&schemas[0].2);
    if !listed.is_absolute()
        || crate::control_fs::absolute_path(listed).map_err(|_| foreign())? != listed
        || listed != canonical.as_path()
    {
        return Err(foreign().into());
    }
    drop(query);
    // Inspect TEMP only when database_list already reported it: preparing a
    // TEMP query on a main-only connection can initialize that namespace.
    if has_temp {
        let has_objects: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema LIMIT 1)",
            [],
            |row| row.get(0),
        )?;
        if has_objects {
            return Err(foreign().into());
        }
    }
    backing.verify(db)
}

fn physical(store: &Store, db: &Connection, source: &OwnedSourceToken) -> anyhow::Result<()> {
    source.revalidate_authority()?;
    let descriptor = source.descriptor();
    let (path, identity) = store.backing.identity_key();
    exact(
        descriptor.kind == HumanSourceKindV1::TenantStore
            && descriptor.tenant_id.as_deref() == Some(store.tenant_id())
            && descriptor.required_format == FORMAT
            && path == descriptor.storage_binding.path()
            && identity.physical_identity()
                == (
                    descriptor.storage_binding.source_device,
                    descriptor.storage_binding.source_inode,
                ),
    )?;
    // Explicit main-connection association plus retained pathname/HasMoved;
    // the public-base StoreStorage::verify alone does not make this association.
    verify_connection_main(&store.backing, db)?;
    Ok(())
}
fn match_token(snapshot: &AuthoritySnapshot, source: &OwnedSourceToken) -> anyhow::Result<()> {
    let descriptor = source.descriptor();
    let closure = source.closure();
    let anchor = &snapshot.anchor;
    let (incarnation, _) = snapshot
        .incarnation
        .as_ref()
        .ok_or(ReviewAuthorityFailure::Required)?;
    exact(
        anchor.installation_id == descriptor.installation_id
            && anchor.source_id == source.source_id()
            && anchor.durable_namespace_id == descriptor.durable_namespace_id
            && anchor.kind == descriptor.kind
            && descriptor.tenant_id.as_ref() == Some(&anchor.tenant_id)
            && snapshot.anchor_receipt.seq == descriptor.authority_marker_receipt.seq
            && snapshot.anchor_receipt.hash == descriptor.authority_marker_receipt.hash
            && incarnation.anchor == snapshot.anchor_receipt
            && incarnation.owner_incarnation == source.incarnation()
            && incarnation.closure_id == closure.closure_id
            && incarnation.closure_revision == closure.closure_revision
            && incarnation.closure_digest == closure.closure_digest
            && incarnation.control_config_public_digest
                == closure.configuration.public_config_digest,
    )
}
/// Startup-only descriptive append. Root owns the writer transaction and must
/// keep every mutating route closed until the whole closure has committed and
/// reverified this incarnation. This helper emits no Ready or launch authority.
pub(super) fn append_incarnation_in_transaction(
    store: &Store,
    tx: &Transaction<'_>,
    epoch: &HumanSourcesWriteEpoch,
    source: &OwnedSourceToken,
    operation_id: uuid::Uuid,
    expected_revision: u64,
) -> anyhow::Result<AuthorityReceiptV1> {
    epoch.validate_source(source)?;
    physical(store, tx, source)?;
    let snapshot = replay(tx)?;
    let descriptor = source.descriptor();
    let closure = source.closure();
    exact(
        snapshot.anchor.source_id == source.source_id()
            && snapshot.anchor.installation_id == descriptor.installation_id
            && snapshot.anchor.durable_namespace_id == descriptor.durable_namespace_id
            && snapshot.anchor_receipt.seq == descriptor.authority_marker_receipt.seq
            && snapshot.anchor_receipt.hash == descriptor.authority_marker_receipt.hash,
    )?;
    let old = tx
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [operation_id.to_string()],
            row_event,
        )
        .optional()?;
    if let Some(old) = old {
        let record = decode(&old)?;
        let AuthorityRecordV1::Incarnation(value) = record else {
            return Err(ReviewAuthorityFailure::Mismatch.into());
        };
        exact(
            value.anchor == snapshot.anchor_receipt
                && value.owner_incarnation == source.incarnation()
                && value.closure_id == closure.closure_id
                && value.closure_revision == closure.closure_revision
                && value.closure_digest == closure.closure_digest
                && value.control_config_public_digest == closure.configuration.public_config_digest
                && expected_revision.checked_add(1) == Some(value.revision)
                && snapshot
                    .incarnation
                    .as_ref()
                    .is_some_and(|(_, receipt)| receipt.seq == old.seq && receipt.hash == old.hash),
        )?;
        epoch.validate_source(source)?;
        return Ok(AuthorityReceiptV1 {
            seq: old.seq,
            hash: old.hash,
        });
    }
    let current = snapshot
        .incarnation
        .as_ref()
        .map_or(0, |(value, _)| value.revision);
    exact(current == expected_revision)?;
    let prefix = audit_head(tx)?;
    let clock = workload::capture_control_time(tx)?;
    let record = SourceIncarnationV1 {
        version: 1,
        protocol: SOURCE_PROTOCOL.into(),
        anchor: snapshot.anchor_receipt,
        closure_id: closure.closure_id,
        closure_revision: closure.closure_revision,
        closure_digest: closure.closure_digest.clone(),
        control_config_public_digest: closure.configuration.public_config_digest.clone(),
        owner_incarnation: source.incarnation(),
        revision: current
            .checked_add(1)
            .ok_or(ReviewAuthorityFailure::Mismatch)?,
        previous_incarnation: snapshot.incarnation.map(|(_, receipt)| receipt),
        previous_seq: prefix.0,
        previous_hash: prefix.1,
    };
    epoch.validate_source(source)?;
    let receipt = append_record(
        store,
        tx,
        operation_id,
        AuthorityRecordV1::Incarnation(record),
        Some(&clock),
    )?;
    epoch.validate_source(source)?;
    Ok(receipt)
}
impl RetainedReviewAuthority {
    pub(super) fn retain_in_transaction(
        store: &Store,
        tx: &Transaction<'_>,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<Self> {
        let result = (|| {
            epoch.validate_source(source)?;
            physical(store, tx, source)?;
            let snapshot = replay(tx)?;
            match_token(&snapshot, source)?;
            epoch.validate_source(source)?;
            Ok(Self {
                source: source.clone(),
                snapshot,
            })
        })();
        invalidate_mismatch(source, &result);
        result
    }
    pub(crate) fn source_token(&self) -> &OwnedSourceToken {
        &self.source
    }
    fn revalidate(
        &self,
        store: &Store,
        db: &Connection,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<()> {
        if !self.source.same_owner_and_source(source) {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        physical(store, db, source)?;
        let snapshot = replay(db)?;
        exact(snapshot == self.snapshot)?;
        match_token(&snapshot, source)?;
        source.revalidate_authority().map_err(Into::into)
    }
}
fn invalidate_mismatch<T>(source: &OwnedSourceToken, result: &anyhow::Result<T>) {
    if result.as_ref().is_err_and(|e| {
        e.downcast_ref::<ReviewAuthorityFailure>() == Some(&ReviewAuthorityFailure::Mismatch)
    }) {
        source.invalidate_authority(HumanAuthorityInvalidationReason::DurableAuthorityMismatch);
    }
}
impl Store {
    pub(crate) fn revalidate_review_binding_source_in_transaction(
        &self,
        tx: &Transaction<'_>,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<()> {
        self.revalidate_review_source_on_connection(tx, source)
    }
    // The same retained connection is available after Transaction::commit has
    // consumed the transaction. This performs the identical physical/marker/
    // incarnation check without manufacturing a postcommit Transaction.
    pub(super) fn revalidate_review_source_on_connection(
        &self,
        db: &Connection,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<()> {
        let retained = self
            .review_authority
            .as_ref()
            .ok_or(ReviewAuthorityFailure::Required)?;
        let result = retained.revalidate(self, db, source);
        // Only an actual compared durable/physical mismatch latches globally.
        // Busy/IO/absence of retained authority is an unavailable operation.
        invalidate_mismatch(source, &result);
        result
    }
}

#[cfg(test)]
mod tests;
