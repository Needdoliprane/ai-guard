//! SQLite capability marker, segment ledger and its replay validation.
//!
//! The capability is an identity in the hash chain, not a store format: the
//! offline `install` appends `control.audit_retention.genesis` and creates the
//! ledger table in one checked transaction after a verified new backup. A
//! binary that predates it refuses the table as an unexpected schema object.
//! Each applied segment is a `control.audit_retention.applied` event and the
//! ledger row it projects; the replay below requires both to agree.
use super::{
    archive::ClassUsageV1,
    chain::{SegmentBound, ZERO_HASH},
    classify::ReclaimClassV1,
    snapshot,
};
use crate::{
    audit::{audit_head, row_event},
    control::invalid,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) const GENESIS_KIND: &str = "control.audit_retention.genesis";
pub(crate) const APPLIED_KIND: &str = "control.audit_retention.applied";
pub(crate) const PROTOCOL: &str = "audit_retention_v1";
pub(crate) const TABLE: &str = "audit_retention_segments";

pub(crate) const SCHEMA: [(&str, &str, &str); 1] = [(
    "table",
    TABLE,
    "CREATE TABLE audit_retention_segments(segment_index INTEGER PRIMARY KEY CHECK(segment_index>=1),event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq) DEFERRABLE INITIALLY DEFERRED,archive_id TEXT NOT NULL UNIQUE,manifest_sha256 TEXT NOT NULL UNIQUE,first_seq INTEGER NOT NULL CHECK(first_seq>=1),last_seq INTEGER NOT NULL CHECK(last_seq>=first_seq),last_hash TEXT NOT NULL,floor_at_last TEXT NOT NULL,archived_events INTEGER NOT NULL,reclaimed_events INTEGER NOT NULL CHECK(reclaimed_events>=1 AND reclaimed_events<archived_events),reclaimed_bytes INTEGER NOT NULL CHECK(reclaimed_bytes>=0),retained_sha256 TEXT NOT NULL,snapshot_json TEXT NOT NULL,snapshot_sha256 TEXT NOT NULL)",
)];
/// UNIQUE event_seq, archive_id and manifest_sha256.
pub(crate) const AUTO_INDEX_COUNTS: [(&str, u32); 1] = [(TABLE, 3)];

/// Store formats whose closed schema admits the capability.
pub(crate) fn format_admits(version: i64) -> bool {
    matches!(version, 13..=15)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetentionGenesisV1 {
    pub(crate) version: u32,
    pub(crate) protocol: String,
    pub(crate) control_namespace_uuid: String,
    pub(crate) previous_seq: i64,
    pub(crate) previous_hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetentionAppliedV1 {
    pub version: u32,
    pub protocol: String,
    pub control_namespace_uuid: String,
    pub segment_index: i64,
    pub first_seq: i64,
    pub last_seq: i64,
    pub last_hash: String,
    pub floor_at_last: String,
    pub archive_id: String,
    pub manifest_sha256: String,
    pub events_sha256: String,
    pub archived_events: i64,
    pub archived_bytes: u64,
    pub classes: Vec<ReclaimClassV1>,
    pub reclaimed_events: i64,
    pub reclaimed_bytes: u64,
    pub reclaim_sha256: String,
    pub reclaimed_by_class: BTreeMap<ReclaimClassV1, ClassUsageV1>,
    pub retained_structural: i64,
    pub retained_pins: i64,
    /// Reclaimable events kept live because a signed checkpoint names them.
    pub kept_positions: Vec<i64>,
    /// Commitment to every row the segment keeps live (`chain::RetainedDigest`).
    pub retained_sha256: String,
    pub snapshot_sha256: String,
    pub policy_sha256: String,
    pub previous_manifest_sha256: Option<String>,
    /// The genesis marker for the first segment, else the previous applied event.
    pub previous_retention_seq: i64,
    pub previous_retention_hash: String,
    /// Chain head immediately before this event.
    pub previous_seq: i64,
    pub previous_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SegmentRow {
    pub(crate) index: i64,
    pub(crate) event_seq: i64,
    pub(crate) archive_id: String,
    pub(crate) manifest_sha256: String,
    pub(crate) first_seq: i64,
    pub(crate) last_seq: i64,
    pub(crate) last_hash: String,
    pub(crate) floor_at_last: String,
    pub(crate) archived_events: i64,
    pub(crate) reclaimed_events: i64,
    pub(crate) reclaimed_bytes: i64,
    pub(crate) retained_sha256: String,
    pub(crate) snapshot_json: String,
    pub(crate) snapshot_sha256: String,
}

impl SegmentRow {
    pub(crate) fn bound(&self) -> SegmentBound {
        SegmentBound {
            index: self.index,
            first_seq: self.first_seq,
            last_seq: self.last_seq,
            last_hash: self.last_hash.clone(),
            archived_events: self.archived_events,
            reclaimed_events: self.reclaimed_events,
            retained_sha256: self.retained_sha256.clone(),
        }
    }
}

fn table_present(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [TABLE],
        |row| row.get(0),
    )?)
}

/// The marker exists in the chain.
pub(crate) fn capability_present(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
        [GENESIS_KIND],
        |row| row.get(0),
    )?)
}

pub(crate) fn segments(db: &Connection) -> anyhow::Result<Vec<SegmentRow>> {
    if !table_present(db)? {
        return Ok(Vec::new());
    }
    let mut statement = db.prepare("SELECT segment_index,event_seq,archive_id,manifest_sha256,first_seq,last_seq,last_hash,floor_at_last,archived_events,reclaimed_events,reclaimed_bytes,retained_sha256,snapshot_json,snapshot_sha256 FROM audit_retention_segments ORDER BY segment_index")?;
    let rows = statement
        .query_map([], |row| {
            Ok(SegmentRow {
                index: row.get(0)?,
                event_seq: row.get(1)?,
                archive_id: row.get(2)?,
                manifest_sha256: row.get(3)?,
                first_seq: row.get(4)?,
                last_seq: row.get(5)?,
                last_hash: row.get(6)?,
                floor_at_last: row.get(7)?,
                archived_events: row.get(8)?,
                reclaimed_events: row.get(9)?,
                reclaimed_bytes: row.get(10)?,
                retained_sha256: row.get(11)?,
                snapshot_json: row.get(12)?,
                snapshot_sha256: row.get(13)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Last segment boundary, or the zero origin.
pub(crate) fn base(db: &Connection) -> anyhow::Result<(i64, String)> {
    if !table_present(db)? {
        return Ok((0, ZERO_HASH.to_owned()));
    }
    Ok(db
        .query_row(
            "SELECT last_seq,last_hash FROM audit_retention_segments ORDER BY segment_index DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, ZERO_HASH.to_owned())))
}

/// Events physically stored: the chain head less every reclaimed event.
pub(crate) fn stored_events(db: &Connection, head: i64) -> anyhow::Result<i64> {
    if !table_present(db)? {
        return Ok(head);
    }
    let reclaimed: i64 = db.query_row(
        "SELECT COALESCE(SUM(reclaimed_events),0) FROM audit_retention_segments",
        [],
        |row| row.get(0),
    )?;
    head.checked_sub(reclaimed)
        .filter(|stored| *stored >= 0)
        .ok_or_else(|| anyhow::anyhow!("audit retention accounting exceeds the chain head"))
}

/// Highest receive time through the last segment boundary.
pub(crate) fn latest_floor(db: &Connection) -> anyhow::Result<Option<String>> {
    if !table_present(db)? {
        return Ok(None);
    }
    Ok(db
        .query_row(
            "SELECT floor_at_last FROM audit_retention_segments ORDER BY segment_index DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

pub(crate) fn latest_snapshot(
    db: &Connection,
) -> anyhow::Result<Option<snapshot::RetentionSnapshotV1>> {
    if !table_present(db)? {
        return Ok(None);
    }
    let json: Option<String> = db
        .query_row(
            "SELECT snapshot_json FROM audit_retention_segments ORDER BY segment_index DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    json.as_deref().map(snapshot::decode).transpose()
}

pub(crate) fn insert_segment(db: &Connection, row: &SegmentRow) -> anyhow::Result<()> {
    let inserted = db.execute(
        "INSERT INTO audit_retention_segments(segment_index,event_seq,archive_id,manifest_sha256,first_seq,last_seq,last_hash,floor_at_last,archived_events,reclaimed_events,reclaimed_bytes,retained_sha256,snapshot_json,snapshot_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            row.index,
            row.event_seq,
            row.archive_id,
            row.manifest_sha256,
            row.first_seq,
            row.last_seq,
            row.last_hash,
            row.floor_at_last,
            row.archived_events,
            row.reclaimed_events,
            row.reclaimed_bytes,
            row.retained_sha256,
            row.snapshot_json,
            row.snapshot_sha256
        ],
    )?;
    anyhow::ensure!(inserted == 1, "audit retention segment was not recorded");
    Ok(())
}

fn namespace(db: &Connection) -> anyhow::Result<String> {
    Ok(db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) fn genesis_marker(db: &Connection) -> anyhow::Result<RetentionGenesisV1> {
    let (previous_seq, previous_hash) = audit_head(db)?;
    Ok(RetentionGenesisV1 {
        version: 1,
        protocol: PROTOCOL.into(),
        control_namespace_uuid: namespace(db)?,
        previous_seq,
        previous_hash,
    })
}

/// Replay of the capability marker and every applied segment against the
/// ledger table. Called by `control::validate` on every validated snapshot.
pub(crate) fn validate(db: &Connection, version: i64) -> anyhow::Result<()> {
    let mut statement = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.audit_retention.*' ORDER BY seq",
    )?;
    let events = statement
        .query_map([], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let rows = segments(db)?;
    if events.is_empty() {
        invalid(rows.is_empty() && !table_present(db)?)?;
        return Ok(());
    }
    invalid(format_admits(version) && table_present(db)?)?;
    let namespace = namespace(db)?;
    let mut iter = events.into_iter();
    let genesis = iter.next().expect("non-empty");
    invalid(
        genesis.event.kind == GENESIS_KIND
            && genesis.event.source == "gateway"
            && genesis.event.provenance == "observed",
    )?;
    let marker: RetentionGenesisV1 = serde_json::from_value(genesis.event.data.clone())?;
    invalid(
        marker.version == 1
            && marker.protocol == PROTOCOL
            && marker.control_namespace_uuid == namespace
            && marker.previous_seq == genesis.seq - 1
            && marker.previous_hash == genesis.prev_hash,
    )?;
    let mut previous_retention = (genesis.seq, genesis.hash.clone());
    let mut previous_manifest: Option<String> = None;
    let mut next_first = 1_i64;
    let mut expected_rows = Vec::new();
    for (position, stored) in iter.enumerate() {
        invalid(
            stored.event.kind == APPLIED_KIND
                && stored.event.source == "gateway"
                && stored.event.provenance == "observed",
        )?;
        let applied: RetentionAppliedV1 = serde_json::from_value(stored.event.data.clone())?;
        let index = i64::try_from(position)? + 1;
        let archived = applied
            .last_seq
            .checked_sub(applied.first_seq)
            .and_then(|span| span.checked_add(1));
        let by_class_events: i64 = applied.reclaimed_by_class.values().map(|c| c.events).sum();
        let by_class_bytes: u64 = applied.reclaimed_by_class.values().map(|c| c.bytes).sum();
        invalid(
            applied.version == 1
                && applied.protocol == PROTOCOL
                && applied.control_namespace_uuid == namespace
                && applied.segment_index == index
                && applied.first_seq == next_first
                && applied.last_seq < stored.seq
                && archived == Some(applied.archived_events)
                && applied.reclaimed_events >= 1
                && applied.reclaimed_events < applied.archived_events
                && applied.retained_structural + applied.retained_pins + applied.reclaimed_events
                    == applied.archived_events
                && by_class_events == applied.reclaimed_events
                && by_class_bytes == applied.reclaimed_bytes
                && applied
                    .reclaimed_by_class
                    .keys()
                    .all(|class| applied.classes.contains(class))
                && applied.kept_positions.len() <= super::MAX_KEPT_POSITIONS
                && applied
                    .kept_positions
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
                && applied
                    .kept_positions
                    .iter()
                    .all(|seq| (applied.first_seq..=applied.last_seq).contains(seq))
                && applied.previous_manifest_sha256 == previous_manifest
                && (
                    applied.previous_retention_seq,
                    applied.previous_retention_hash.clone(),
                ) == previous_retention
                && applied.previous_seq == stored.seq - 1
                && applied.previous_hash == stored.prev_hash
                && uuid::Uuid::parse_str(&applied.archive_id).is_ok(),
        )?;
        expected_rows.push((stored.seq, applied.clone()));
        previous_retention = (stored.seq, stored.hash.clone());
        previous_manifest = Some(applied.manifest_sha256.clone());
        next_first = applied.last_seq + 1;
    }
    invalid(rows.len() == expected_rows.len())?;
    for (row, (event_seq, applied)) in rows.iter().zip(&expected_rows) {
        invalid(
            row.event_seq == *event_seq
                && row.index == applied.segment_index
                && row.archive_id == applied.archive_id
                && row.manifest_sha256 == applied.manifest_sha256
                && row.first_seq == applied.first_seq
                && row.last_seq == applied.last_seq
                && row.last_hash == applied.last_hash
                && row.floor_at_last == applied.floor_at_last
                && row.archived_events == applied.archived_events
                && row.reclaimed_events == applied.reclaimed_events
                && u64::try_from(row.reclaimed_bytes).ok() == Some(applied.reclaimed_bytes)
                && row.retained_sha256 == applied.retained_sha256
                && row.snapshot_sha256 == applied.snapshot_sha256
                && snapshot::sha256(&row.snapshot_json) == row.snapshot_sha256,
        )?;
        let decoded = snapshot::decode(&row.snapshot_json)?;
        invalid(decoded.through_seq == row.last_seq)?;
    }
    Ok(())
}
