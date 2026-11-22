//! Retention operations of the SQLite tenant Store.
//!
//! `install`, `plan` and `apply` run offline (gateway and brokers stopped, an
//! operator attestation) inside one checked IMMEDIATE transaction each: no
//! other writer can interleave between the plan, the archive snapshot and the
//! reclaim. `apply` recomputes its plan inside that transaction and refuses
//! one that differs from the plan the operator reviewed. `export` and
//! `verify` only read.
use super::{
    archive::{
        ArchivePurposeV1, ArchiveWriter, ArchivedEventV1, ClassUsageV1, ManifestHeader,
        OpenedArchive, ReclaimSummaryV1, WrittenArchive, read_archive, read_manifest,
    },
    chain::{RetainedDigest, ZERO_HASH},
    classify::{ReclaimClassV1, ReclaimSelector, Selection},
    failure::{RetentionFailure, code},
    ledger::{self, RetentionAppliedV1, RetentionApplyRecordV1, SegmentRow},
    policy::AuditRetentionPolicyV1,
    snapshot::{self, SnapshotFold},
};
use crate::{
    audit::{Store, audit_head, capacity, control, validate_schema},
    broker_registry::{format_time, parse_time},
    model::Event,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// What a segment would archive and reclaim. Its digest binds `apply`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetentionPlanV1 {
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub head_seq: i64,
    pub head_hash: String,
    pub segment_index: i64,
    pub first_seq: i64,
    pub last_seq: i64,
    pub last_hash: String,
    pub floor_at_last: String,
    pub archived_events: i64,
    pub archived_bytes: u64,
    pub classes: Vec<ReclaimClassV1>,
    pub reclaimed_events: i64,
    pub reclaimed_bytes: u64,
    pub reclaimed_by_class: BTreeMap<ReclaimClassV1, ClassUsageV1>,
    pub retained_structural: i64,
    pub retained_pins: i64,
    /// Reclaimable events kept live because a signed checkpoint names them.
    pub kept_positions: Vec<i64>,
    /// Commitment to every row the segment keeps live.
    pub retained_sha256: String,
    pub reclaim_sha256: String,
    pub policy_sha256: String,
}

impl RetentionPlanV1 {
    pub fn sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:audit-retention-plan:v1\0");
        digest.update(serde_json::to_vec(self).expect("serializable plan"));
        hex::encode(digest.finalize())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditUsageV1 {
    pub stored_events: i64,
    pub stored_bytes: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct PlannedRetentionV1 {
    pub plan: RetentionPlanV1,
    pub plan_sha256: String,
    /// Highest sequence the policy admits now; informative, not bound.
    pub eligible_through_seq: i64,
    pub horizon: String,
    pub usage_before: AuditUsageV1,
    pub usage_after_reclaim: AuditUsageV1,
    pub max_events: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct ArchiveSummaryV1 {
    pub path: String,
    pub archive_id: String,
    pub manifest_sha256: String,
    pub purpose: ArchivePurposeV1,
    pub segment_index: i64,
    pub first_seq: i64,
    pub last_seq: i64,
    pub last_hash: String,
    pub event_count: i64,
    pub events_sha256: String,
}

impl ArchiveSummaryV1 {
    fn of(path: &Path, manifest: &super::archive::ArchiveManifestV1, sha: &str) -> Self {
        Self {
            path: path.display().to_string(),
            archive_id: manifest.archive_id.clone(),
            manifest_sha256: sha.to_owned(),
            purpose: manifest.purpose,
            segment_index: manifest.segment_index,
            first_seq: manifest.first_seq,
            last_seq: manifest.last_seq,
            last_hash: manifest.last_hash.clone(),
            event_count: manifest.event_count,
            events_sha256: manifest.events_sha256.clone(),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct RetentionReceiptV1 {
    pub plan_sha256: String,
    pub archive: ArchiveSummaryV1,
    pub retention_event_seq: i64,
    pub retention_event_hash: String,
    pub reclaimed_events: i64,
    pub reclaimed_bytes: u64,
    pub usage_before: AuditUsageV1,
    pub usage_after: AuditUsageV1,
}

#[derive(Serialize, Clone, Debug)]
pub struct RetentionInstallReportV1 {
    pub backup_path: String,
    pub backup_head_seq: u64,
    pub backup_head_hash: String,
    pub genesis_seq: i64,
    pub genesis_hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetentionSegmentSummaryV1 {
    pub segment_index: i64,
    pub archive_id: String,
    pub manifest_sha256: String,
    pub first_seq: i64,
    pub last_seq: i64,
    pub last_hash: String,
    pub reclaimed_events: i64,
    pub reclaimed_bytes: i64,
    pub retention_event_seq: i64,
}

/// What the live store records of its archived prefix.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetentionLedgerV1 {
    pub capability: bool,
    pub segments: i64,
    pub archived_through_seq: i64,
    pub reclaimed_events: i64,
    pub reclaimed_bytes: i64,
    pub last: Option<RetentionSegmentSummaryV1>,
    /// Every segment, oldest first; bounded by the number of retention runs.
    pub segment_list: Vec<RetentionSegmentSummaryV1>,
}

pub const VERIFICATION_FORMAT: &str = "ai-guard.audit.retention-verification.v1";

/// Scope that a verified verdict does not cover.
pub const VERIFICATION_LIMITS: [&str; 3] = [
    "reclaimed human read audits are verified as chained bytes, not re-validated against the identity projection at their position",
    "the live hash chain is unkeyed: a coherent rewrite of the live store and its archives by the storage owner is only detected against signed checkpoints",
    "an archive that is not passed to verify is not examined",
];

#[derive(Serialize, Clone, Debug)]
pub struct VerificationFailureV1 {
    pub code: &'static str,
    pub message: String,
    pub detail: serde_json::Map<String, Value>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RetentionVerificationV1 {
    pub format: &'static str,
    pub verdict: &'static str,
    pub tenant_id: String,
    pub live_head_seq: i64,
    pub live_head_hash: String,
    pub stored_events: i64,
    pub segments: i64,
    pub segments_verified: i64,
    pub archived_through_seq: i64,
    pub reclaimed_events: i64,
    pub exports_verified: i64,
    pub warnings: Vec<Value>,
    pub failure: Option<VerificationFailureV1>,
    pub limits: Vec<&'static str>,
}

/// One explicit, deterministic apply request.
pub struct RetentionApplyRequest<'a> {
    pub policy: &'a AuditRetentionPolicyV1,
    /// Evaluation time of the policy age floor. The CLI passes the clock.
    pub now: DateTime<Utc>,
    pub through_seq: i64,
    pub expect_plan_sha256: &'a str,
    pub archive_dir: &'a Path,
    /// Signed checkpoint positions to keep live (empty when the operator
    /// attests that the tenant has no signed checkpoints).
    pub kept_positions: &'a BTreeSet<i64>,
}

fn failure(code: &'static str, message: &str) -> anyhow::Error {
    RetentionFailure::new(code, message).into()
}

fn reclaim_sha256(seqs: &[i64]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:audit-retention-reclaim:v1\0");
    for seq in seqs {
        digest.update(seq.to_be_bytes());
    }
    hex::encode(digest.finalize())
}

fn quota_bytes(db: &Connection) -> anyhow::Result<u64> {
    let used: i64 = db.query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row.get(0))?;
    Ok(u64::try_from(used)?)
}

fn control_genesis(db: &Connection) -> anyhow::Result<(String, i64, String)> {
    db.query_row(
        "SELECT namespace_uuid,genesis_seq,genesis_hash FROM control_meta WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()?
    .ok_or_else(|| failure(code::FORMAT_UNSUPPORTED, "the store has no control genesis"))
}

fn row_archived(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArchivedEventV1> {
    Ok(ArchivedEventV1 {
        seq: row.get(0)?,
        id: row.get(1)?,
        received_at: row.get(2)?,
        prev_hash: row.get(3)?,
        hash: row.get(4)?,
        payload: row.get(5)?,
    })
}

const RANGE_SQL: &str = "SELECT seq,id,received_at,prev_hash,hash,payload FROM events WHERE seq BETWEEN ?1 AND ?2 ORDER BY seq";

struct Work {
    planned: PlannedRetentionV1,
    reclaim: Vec<i64>,
}

fn plan_in(
    store: &Store,
    db: &Connection,
    policy: &AuditRetentionPolicyV1,
    now: DateTime<Utc>,
    through: Option<i64>,
    kept: &BTreeSet<i64>,
) -> anyhow::Result<Work> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !ledger::format_admits(version) {
        return Err(RetentionFailure::new(
            code::FORMAT_UNSUPPORTED,
            "audit retention requires a format 13, 14 or 15 store",
        )
        .with("user_version", version)
        .into());
    }
    if !ledger::capability_present(db)? {
        return Err(failure(
            code::CAPABILITY_MISSING,
            "audit retention is not installed on this store; run audit-retention install",
        ));
    }
    let rows = ledger::segments(db)?;
    let base = rows.last().map_or(0, |row| row.last_seq);
    let first = base + 1;
    let segment_index = i64::try_from(rows.len())? + 1;
    let (head, head_hash) = audit_head(db)?;
    let min_age = chrono::Duration::seconds(i64::try_from(policy.min_age_seconds)?);
    let horizon = format_time(now - min_age);
    let keep = head.saturating_sub(i64::try_from(policy.keep_recent_events)?);
    let segment_end = first
        .saturating_add(i64::try_from(policy.max_segment_events)?)
        .saturating_sub(1);
    let aged: Option<i64> = db.query_row(
        "SELECT MAX(event_seq) FROM audit_time_floors WHERE event_seq>=?1 AND max_received_at<=?2",
        params![first, horizon],
        |row| row.get(0),
    )?;
    let eligible = keep.min(segment_end).min(aged.unwrap_or(0));
    let nothing = |eligible: i64| {
        RetentionFailure::new(
            code::NOTHING_TO_RECLAIM,
            "no event of the policy range can be reclaimed yet",
        )
        .with("first_seq", first)
        .with("eligible_through_seq", eligible.max(base))
        .with("horizon", horizon.clone())
    };
    if eligible < first {
        return Err(nothing(eligible).into());
    }
    let last = match through {
        Some(seq) if seq < first || seq > eligible => {
            return Err(RetentionFailure::new(
                code::RANGE_INVALID,
                "the requested segment end is outside the range the policy admits",
            )
            .with("first_seq", first)
            .with("eligible_through_seq", eligible)
            .with("through_seq", seq)
            .into());
        }
        Some(seq) => seq,
        None => eligible,
    };
    let classes = policy.class_set();
    let mut selector = ReclaimSelector::default();
    {
        let mut statement = db.prepare("SELECT payload FROM events WHERE seq<?1 ORDER BY seq")?;
        let mut retained = statement.query([first])?;
        while let Some(row) = retained.next()? {
            let event: Event = serde_json::from_str(&row.get::<_, String>(0)?)?;
            selector.observe_retained(&event);
        }
    }
    let mut reclaim = Vec::new();
    let mut by_class: BTreeMap<ReclaimClassV1, ClassUsageV1> = BTreeMap::new();
    let (mut structural, mut pins, mut archived, mut archived_bytes, mut reclaimed_bytes) =
        (0_i64, 0_i64, 0_i64, 0_u64, 0_u64);
    let mut last_hash = None;
    let mut kept_positions = Vec::new();
    let mut retained_digest = RetainedDigest::new();
    {
        let mut statement = db.prepare(RANGE_SQL)?;
        let mut range = statement.query(params![first, last])?;
        while let Some(row) = range.next()? {
            let archived_row = row_archived(row)?;
            anyhow::ensure!(
                archived_row.seq == first + archived,
                RetentionFailure::new(
                    code::AUDIT_CHAIN_INVALID,
                    "the segment range is not contiguous in the live store"
                )
            );
            let event: Event = serde_json::from_str(&archived_row.payload)?;
            let bytes = archived_row.payload.len() as u64;
            archived += 1;
            archived_bytes += bytes;
            match selector.select(&event, archived_row.seq == last, &classes) {
                Selection::Reclaim(_) if kept.contains(&archived_row.seq) => {
                    pins += 1;
                    kept_positions.push(archived_row.seq);
                    retained_digest.add(archived_row.seq, &archived_row.hash);
                }
                Selection::Reclaim(class) => {
                    reclaim.push(archived_row.seq);
                    reclaimed_bytes += bytes;
                    let usage = by_class.entry(class).or_default();
                    usage.events += 1;
                    usage.bytes += bytes;
                }
                Selection::Structural => {
                    structural += 1;
                    retained_digest.add(archived_row.seq, &archived_row.hash);
                }
                Selection::Boundary | Selection::FirstSighting => {
                    pins += 1;
                    retained_digest.add(archived_row.seq, &archived_row.hash);
                }
            }
            last_hash = Some(archived_row.hash);
        }
    }
    anyhow::ensure!(
        archived == last - first + 1,
        RetentionFailure::new(
            code::AUDIT_CHAIN_INVALID,
            "the segment range is not contiguous in the live store"
        )
    );
    if reclaim.is_empty() {
        return Err(nothing(last).into());
    }
    if kept_positions.len() > super::MAX_KEPT_POSITIONS {
        return Err(RetentionFailure::new(
            code::RANGE_INVALID,
            "the segment holds more signed checkpoint positions than one segment may keep; plan a shorter segment",
        )
        .with("kept_positions", kept_positions.len())
        .into());
    }
    let floor_at_last: String = db.query_row(
        "SELECT max_received_at FROM audit_time_floors WHERE event_seq=?1",
        [last],
        |row| row.get(0),
    )?;
    let (namespace, _, _) = control_genesis(db)?;
    let stored = ledger::stored_events(db, head)?;
    let bytes = quota_bytes(db)?;
    let plan = RetentionPlanV1 {
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        head_seq: head,
        head_hash,
        segment_index,
        first_seq: first,
        last_seq: last,
        last_hash: last_hash.unwrap_or_else(|| ZERO_HASH.to_owned()),
        floor_at_last,
        archived_events: archived,
        archived_bytes,
        classes: policy.classes.clone(),
        reclaimed_events: i64::try_from(reclaim.len())?,
        reclaimed_bytes,
        reclaimed_by_class: by_class,
        retained_structural: structural,
        retained_pins: pins,
        kept_positions,
        retained_sha256: retained_digest.finish(),
        reclaim_sha256: reclaim_sha256(&reclaim),
        policy_sha256: policy.fingerprint(),
    };
    let plan_sha256 = plan.sha256();
    let usage_after_reclaim = AuditUsageV1 {
        stored_events: stored - plan.reclaimed_events,
        stored_bytes: bytes - reclaimed_bytes,
    };
    Ok(Work {
        planned: PlannedRetentionV1 {
            plan_sha256,
            eligible_through_seq: eligible,
            horizon,
            usage_before: AuditUsageV1 {
                stored_events: stored,
                stored_bytes: bytes,
            },
            usage_after_reclaim,
            max_events: store.max_events,
            plan,
        },
        reclaim,
    })
}

fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// A directory already tied to the exact operation in the checked store can
/// confirm a previous apply. An unrecorded or older archive remains occupied.
fn confirm_apply(
    db: &Connection,
    request: &RetentionApplyRequest<'_>,
) -> anyhow::Result<Option<RetentionReceiptV1>> {
    let Ok(archive) = read_manifest(request.archive_dir) else {
        return Ok(None);
    };
    if archive.manifest.purpose != ArchivePurposeV1::Retention || !ledger::capability_present(db)? {
        return Ok(None);
    }
    let committed: Option<(i64, String, String)> = db
        .query_row(
            "SELECT e.seq,e.hash,e.payload FROM audit_retention_segments s JOIN events e ON e.seq=s.event_seq WHERE s.manifest_sha256=?1",
            [&archive.manifest_sha256],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((event_seq, event_hash, payload)) = committed else {
        return Ok(None);
    };
    let event: Event = serde_json::from_str(&payload)?;
    let record: RetentionAppliedV1 = serde_json::from_value(event.data)?;
    let Some(apply) = record.apply else {
        return Ok(None);
    };
    anyhow::ensure!(
        apply.plan_sha256 == request.expect_plan_sha256
            && record.last_seq == request.through_seq
            && record.policy_sha256 == request.policy.fingerprint(),
        RetentionFailure::new(
            code::PLAN_MISMATCH,
            "the archive is already recorded for a different plan",
        )
    );
    let verified = read_archive(&archive.path, |_, _| Ok(()))?;
    anyhow::ensure!(
        verified.manifest_sha256 == archive.manifest_sha256,
        RetentionFailure::new(
            code::ARCHIVE_UNVERIFIED,
            "the recorded archive changed during verification"
        )
    );
    let payload_bytes = u64::try_from(payload.len())?;
    let usage_after = AuditUsageV1 {
        stored_events: apply
            .usage_before
            .stored_events
            .checked_sub(record.reclaimed_events)
            .and_then(|remaining| remaining.checked_add(1))
            .context("the confirmed retention event count is invalid")?,
        stored_bytes: apply
            .usage_before
            .stored_bytes
            .checked_sub(record.reclaimed_bytes)
            .and_then(|remaining| remaining.checked_add(payload_bytes))
            .context("the confirmed retention byte count is invalid")?,
    };
    Ok(Some(RetentionReceiptV1 {
        plan_sha256: apply.plan_sha256,
        archive: ArchiveSummaryV1::of(&archive.path, &archive.manifest, &archive.manifest_sha256),
        retention_event_seq: event_seq,
        retention_event_hash: event_hash,
        reclaimed_events: record.reclaimed_events,
        reclaimed_bytes: record.reclaimed_bytes,
        usage_before: apply.usage_before,
        usage_after,
    }))
}

impl Store {
    /// Install the retention capability once, after a verified new backup.
    pub fn install_audit_retention(
        &self,
        backup_path: &Path,
    ) -> anyhow::Result<RetentionInstallReportV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let (target, expected, _) =
            self.backup_verified(&mut db, backup_path, |preflight, version| {
                if !ledger::format_admits(version) {
                    return Err(RetentionFailure::new(
                        code::FORMAT_UNSUPPORTED,
                        "audit retention requires a format 13, 14 or 15 store",
                    )
                    .with("user_version", version)
                    .into());
                }
                if ledger::capability_present(preflight)? {
                    return Err(failure(
                        code::CAPABILITY_PRESENT,
                        "audit retention is already installed on this store",
                    ));
                }
                Ok(())
            })?;
        let tx = self.begin_checked(&mut db)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let current = Store::check_integrity(&tx)?;
        crate::control::conflict(
            ledger::format_admits(version)
                && current.valid
                && current.event_count == expected.event_count
                && current.head_hash == expected.head_hash
                && !ledger::capability_present(&tx)?,
        )?;
        for (_, _, sql) in ledger::SCHEMA {
            tx.execute_batch(sql)?;
        }
        let marker = ledger::genesis_marker(&tx)?;
        let mut event = Event::new(ledger::GENESIS_KIND);
        event.data = serde_json::to_value(&marker)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            event.data == serde_json::to_value(&marker)?,
            "the retention marker must not contain masked values"
        );
        let stored = self.append_with_capacity_class(
            &tx,
            event,
            payload,
            capacity::Class::RetentionMaintenance,
        )?;
        validate_schema(&tx, false)?;
        Store::validate_metadata(&tx, &self.tenant_id)?;
        anyhow::ensure!(
            Store::check_integrity(&tx)?.valid,
            "post-install audit integrity failure"
        );
        control::validate(&tx)?;
        self.commit_checked(tx)?;
        Ok(RetentionInstallReportV1 {
            backup_path: target.display().to_string(),
            backup_head_seq: expected.event_count,
            backup_head_hash: expected.head_hash,
            genesis_seq: stored.seq,
            genesis_hash: stored.hash,
        })
    }

    /// Compute, without effect, the next segment the policy admits.
    pub fn audit_retention_plan(
        &self,
        policy: &AuditRetentionPolicyV1,
        now: DateTime<Utc>,
        through_seq: Option<i64>,
        kept_positions: &BTreeSet<i64>,
    ) -> anyhow::Result<PlannedRetentionV1> {
        policy.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let work = plan_in(self, &tx, policy, now, through_seq, kept_positions)?;
        self.commit_checked(tx)?;
        Ok(work.planned)
    }

    /// Archive the planned segment into a new directory, verify it, then
    /// reclaim its leaf events and record the segment, in one transaction.
    pub fn apply_audit_retention(
        &self,
        request: &RetentionApplyRequest<'_>,
    ) -> anyhow::Result<RetentionReceiptV1> {
        request.policy.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        if let Some(receipt) = confirm_apply(&tx, request)? {
            return Ok(receipt);
        }
        let Work { planned, reclaim } = plan_in(
            self,
            &tx,
            request.policy,
            request.now,
            Some(request.through_seq),
            request.kept_positions,
        )?;
        if planned.plan_sha256 != request.expect_plan_sha256 {
            return Err(RetentionFailure::new(
                code::PLAN_MISMATCH,
                "the store no longer matches the reviewed plan; plan again",
            )
            .with("plan_sha256", planned.plan_sha256.clone())
            .into());
        }
        let plan = planned.plan.clone();
        let rows = ledger::segments(&tx)?;
        let previous = rows.last().cloned();
        let mut fold = match &previous {
            Some(row) => SnapshotFold::resume(snapshot::decode(&row.snapshot_json)?),
            None => SnapshotFold::empty(),
        };
        let (_, genesis_seq, genesis_hash) = control_genesis(&tx)?;
        let mut writer = ArchiveWriter::create(request.archive_dir, &self.backing)?;
        let archive_path = writer.path().to_path_buf();
        let written = (|| -> anyhow::Result<(WrittenArchive, String, String)> {
            {
                let mut statement = tx.prepare(RANGE_SQL)?;
                let mut range = statement.query(params![plan.first_seq, plan.last_seq])?;
                while let Some(row) = range.next()? {
                    let archived = row_archived(row)?;
                    let event: Event = serde_json::from_str(&archived.payload)?;
                    writer.push(&archived)?;
                    fold.observe(archived.seq, &event);
                }
            }
            let snapshot = fold.finish();
            let (snapshot_json, snapshot_sha256) = snapshot::encode(&snapshot)?;
            let reclaim_summary = ReclaimSummaryV1 {
                classes: plan.classes.clone(),
                reclaimed_events: plan.reclaimed_events,
                reclaimed_bytes: plan.reclaimed_bytes,
                reclaim_sha256: plan.reclaim_sha256.clone(),
                by_class: plan.reclaimed_by_class.clone(),
                retained_structural: plan.retained_structural,
                retained_pins: plan.retained_pins,
                kept_positions: plan.kept_positions.clone(),
                retained_sha256: plan.retained_sha256.clone(),
            };
            let header = ManifestHeader {
                archive_id: uuid::Uuid::new_v4().to_string(),
                purpose: ArchivePurposeV1::Retention,
                tenant_id: self.tenant_id.clone(),
                control_namespace_uuid: plan.control_namespace_uuid.clone(),
                control_genesis_seq: genesis_seq,
                control_genesis_hash: genesis_hash.clone(),
                backend: "sqlite",
                segment_index: plan.segment_index,
                floor_at_last: Some(plan.floor_at_last.clone()),
                previous_manifest_sha256: previous.as_ref().map(|row| row.manifest_sha256.clone()),
                reclaim: Some(reclaim_summary),
                snapshot_sha256: Some(snapshot_sha256.clone()),
                policy_sha256: Some(plan.policy_sha256.clone()),
                created_at: format_time(Utc::now()),
            };
            Ok((writer.finish(header)?, snapshot_json, snapshot_sha256))
        })();
        let (written, snapshot_json, snapshot_sha256) =
            written.map_err(|error| match super::failure::refusal(&error) {
                Some(refusal) => refusal
                    .clone()
                    .with("incomplete_archive", archive_path.display().to_string())
                    .into(),
                None => error.context(format!(
                    "nothing was reclaimed; the incomplete archive {} is kept for diagnosis",
                    archive_path.display()
                )),
            })?;
        let orphan = |error: anyhow::Error| -> anyhow::Error {
            match super::failure::refusal(&error) {
                Some(refusal) => refusal
                    .clone()
                    .with("orphan_archive", written.path.display().to_string())
                    .into(),
                None => error.context(format!(
                    "nothing was reclaimed; the archive {} is complete but not part of the chain",
                    written.path.display()
                )),
            }
        };
        let mut readback = 0_i64;
        let reopened = read_archive(&written.path, |_, _| {
            readback += 1;
            Ok(())
        })
        .map_err(|fault| {
            orphan(
                RetentionFailure::new(
                    code::ARCHIVE_UNVERIFIED,
                    "the written archive does not verify",
                )
                .with("cause", fault.to_string())
                .into(),
            )
        })?;
        if reopened.manifest_sha256 != written.manifest_sha256
            || readback != plan.archived_events
            || reopened.manifest.last_hash != plan.last_hash
        {
            return Err(orphan(failure(
                code::ARCHIVE_UNVERIFIED,
                "the written archive differs from the planned segment",
            )));
        }
        let applied = (|| -> anyhow::Result<RetentionReceiptV1> {
            for seq in &reclaim {
                let floors =
                    tx.execute("DELETE FROM audit_time_floors WHERE event_seq=?1", [seq])?;
                let removed = match tx.execute("DELETE FROM events WHERE seq=?1", [seq]) {
                    Ok(removed) => removed,
                    Err(error) if is_constraint(&error) => {
                        return Err(RetentionFailure::new(
                            code::RECLAIM_REFERENCED_EVENT,
                            "a projection references an event selected for reclaim",
                        )
                        .with("seq", *seq)
                        .into());
                    }
                    Err(error) => return Err(error.into()),
                };
                anyhow::ensure!(
                    floors == 1 && removed == 1,
                    "reclaim did not remove exactly one planned event"
                );
            }
            let updated = tx.execute(
                "UPDATE quota SET bytes=bytes-?1 WHERE id=1",
                [i64::try_from(plan.reclaimed_bytes)?],
            )?;
            anyhow::ensure!(updated == 1, "audit byte quota was not updated");
            let event_seq = plan.head_seq + 1;
            let previous_retention: (i64, String) = match &previous {
                Some(row) => (
                    row.event_seq,
                    tx.query_row(
                        "SELECT hash FROM events WHERE seq=?1",
                        [row.event_seq],
                        |r| r.get(0),
                    )?,
                ),
                None => tx.query_row(
                    "SELECT seq,hash FROM events WHERE kind=?1",
                    [ledger::GENESIS_KIND],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?,
            };
            let record = RetentionAppliedV1 {
                version: 1,
                protocol: ledger::PROTOCOL.into(),
                control_namespace_uuid: plan.control_namespace_uuid.clone(),
                segment_index: plan.segment_index,
                first_seq: plan.first_seq,
                last_seq: plan.last_seq,
                last_hash: plan.last_hash.clone(),
                floor_at_last: plan.floor_at_last.clone(),
                archive_id: written.manifest.archive_id.clone(),
                manifest_sha256: written.manifest_sha256.clone(),
                events_sha256: written.manifest.events_sha256.clone(),
                archived_events: plan.archived_events,
                archived_bytes: plan.archived_bytes,
                classes: plan.classes.clone(),
                reclaimed_events: plan.reclaimed_events,
                reclaimed_bytes: plan.reclaimed_bytes,
                reclaim_sha256: plan.reclaim_sha256.clone(),
                reclaimed_by_class: plan.reclaimed_by_class.clone(),
                retained_structural: plan.retained_structural,
                retained_pins: plan.retained_pins,
                kept_positions: plan.kept_positions.clone(),
                retained_sha256: plan.retained_sha256.clone(),
                snapshot_sha256: snapshot_sha256.clone(),
                policy_sha256: plan.policy_sha256.clone(),
                previous_manifest_sha256: previous.as_ref().map(|row| row.manifest_sha256.clone()),
                previous_retention_seq: previous_retention.0,
                previous_retention_hash: previous_retention.1,
                previous_seq: plan.head_seq,
                previous_hash: plan.head_hash.clone(),
                apply: Some(RetentionApplyRecordV1 {
                    plan_sha256: planned.plan_sha256.clone(),
                    usage_before: planned.usage_before.clone(),
                }),
            };
            ledger::insert_segment(
                &tx,
                &SegmentRow {
                    index: plan.segment_index,
                    event_seq,
                    archive_id: record.archive_id.clone(),
                    manifest_sha256: record.manifest_sha256.clone(),
                    first_seq: plan.first_seq,
                    last_seq: plan.last_seq,
                    last_hash: plan.last_hash.clone(),
                    floor_at_last: plan.floor_at_last.clone(),
                    archived_events: plan.archived_events,
                    reclaimed_events: plan.reclaimed_events,
                    reclaimed_bytes: i64::try_from(plan.reclaimed_bytes)?,
                    retained_sha256: plan.retained_sha256.clone(),
                    snapshot_json: snapshot_json.clone(),
                    snapshot_sha256: snapshot_sha256.clone(),
                },
            )?;
            let data = serde_json::to_value(&record)?;
            let mut event = Event::new(ledger::APPLIED_KIND);
            event.data = data.clone();
            let (event, payload) = self.prepare_event(event)?;
            anyhow::ensure!(
                event.data == data,
                "the retention record must not contain masked values"
            );
            let stored = self.append_with_capacity_class(
                &tx,
                event,
                payload,
                capacity::Class::RetentionMaintenance,
            )?;
            anyhow::ensure!(
                stored.seq == event_seq,
                "the retention record did not follow the planned head"
            );
            validate_schema(&tx, false)?;
            Store::validate_metadata(&tx, &self.tenant_id)?;
            anyhow::ensure!(
                Store::check_integrity(&tx)?.valid,
                RetentionFailure::new(
                    code::AUDIT_CHAIN_INVALID,
                    "the reclaimed store does not verify; nothing was changed"
                )
            );
            control::validate(&tx)?;
            let after = AuditUsageV1 {
                stored_events: ledger::stored_events(&tx, stored.seq)?,
                stored_bytes: quota_bytes(&tx)?,
            };
            Ok(RetentionReceiptV1 {
                plan_sha256: planned.plan_sha256.clone(),
                archive: ArchiveSummaryV1::of(
                    &written.path,
                    &written.manifest,
                    &written.manifest_sha256,
                ),
                retention_event_seq: stored.seq,
                retention_event_hash: stored.hash,
                reclaimed_events: plan.reclaimed_events,
                reclaimed_bytes: plan.reclaimed_bytes,
                usage_before: planned.usage_before.clone(),
                usage_after: after,
            })
        })();
        let receipt = applied.map_err(&orphan)?;
        self.commit_checked(tx).map_err(|error| {
            error.context(
                RetentionFailure::new(
                    code::COMMIT_UNCONFIRMED,
                    "the archive is complete, but the store could not confirm the commit; keep the archive and retry the same apply to check the receipt",
                )
                .with("archive", written.path.display().to_string())
                .with("manifest_sha256", written.manifest_sha256.clone()),
            )
        })?;
        // Budgets restart at the new base: the next admission rescans from it.
        *self
            .supervision_lease
            .capacity
            .lock()
            .map_err(|_| anyhow::anyhow!("audit capacity cache lock poisoned"))? =
            capacity::Cache::default();
        Ok(receipt)
    }

    /// Write a verifiable export of a contiguous live range into a new directory.
    pub fn export_audit_archive(
        &self,
        archive_dir: &Path,
        first_seq: Option<i64>,
        last_seq: Option<i64>,
    ) -> anyhow::Result<ArchiveSummaryV1> {
        let integrity = self.integrity()?;
        if !integrity.valid {
            return Err(failure(
                code::AUDIT_CHAIN_INVALID,
                "the live audit chain does not verify",
            ));
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        self.backing.verify(&tx)?;
        let (namespace, genesis_seq, genesis_hash) = control_genesis(&tx)?;
        let (base, _) = ledger::base(&tx)?;
        let (head, _) = audit_head(&tx)?;
        let first = first_seq.unwrap_or(base + 1);
        let last = last_seq.unwrap_or(i64::try_from(integrity.event_count)?);
        if first <= base || first < 1 || last < first || last > head {
            return Err(RetentionFailure::new(
                code::RANGE_INVALID,
                "an export covers a contiguous live range after the archived prefix",
            )
            .with("archived_through_seq", base)
            .with("head_seq", head)
            .into());
        }
        let floors: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='audit_time_floors')",
            [],
            |row| row.get(0),
        )?;
        let floor_at_last: Option<String> = if floors {
            tx.query_row(
                "SELECT max_received_at FROM audit_time_floors WHERE event_seq=?1",
                [last],
                |row| row.get(0),
            )
            .optional()?
        } else {
            None
        };
        let mut writer = ArchiveWriter::create(archive_dir, &self.backing)?;
        {
            let mut statement = tx.prepare(RANGE_SQL)?;
            let mut range = statement.query(params![first, last])?;
            while let Some(row) = range.next()? {
                writer.push(&row_archived(row)?)?;
            }
        }
        let written = writer.finish(ManifestHeader {
            archive_id: uuid::Uuid::new_v4().to_string(),
            purpose: ArchivePurposeV1::Export,
            tenant_id: self.tenant_id.clone(),
            control_namespace_uuid: namespace,
            control_genesis_seq: genesis_seq,
            control_genesis_hash: genesis_hash,
            backend: "sqlite",
            segment_index: 0,
            floor_at_last,
            previous_manifest_sha256: None,
            reclaim: None,
            snapshot_sha256: None,
            policy_sha256: None,
            created_at: format_time(Utc::now()),
        })?;
        drop(tx);
        let reopened = read_archive(&written.path, |_, _| Ok(()))?;
        if reopened.manifest_sha256 != written.manifest_sha256
            || written.manifest.event_count != last - first + 1
        {
            return Err(failure(
                code::ARCHIVE_UNVERIFIED,
                "the written export differs from the live range",
            ));
        }
        Ok(ArchiveSummaryV1::of(
            &written.path,
            &written.manifest,
            &written.manifest_sha256,
        ))
    }

    /// Ledger summary for integrity reports and the dashboard.
    pub fn audit_retention_ledger(&self) -> anyhow::Result<RetentionLedgerV1> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.verify_diagnostic_backing(&db)?;
        ledger_of(&db)
    }

    /// Verify the archived prefix end to end against the live store.
    pub fn verify_audit_archives(
        &self,
        archives: &[PathBuf],
    ) -> anyhow::Result<RetentionVerificationV1> {
        let integrity = self.integrity()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        self.backing.verify(&tx)?;
        let (head, head_hash) = audit_head(&tx)?;
        let rows = ledger::segments(&tx)?;
        let ledger = ledger_of(&tx)?;
        let mut report = RetentionVerificationV1 {
            format: VERIFICATION_FORMAT,
            verdict: "refused",
            tenant_id: self.tenant_id.clone(),
            live_head_seq: head,
            live_head_hash: head_hash,
            stored_events: ledger::stored_events(&tx, head)?,
            segments: ledger.segments,
            segments_verified: 0,
            archived_through_seq: ledger.archived_through_seq,
            reclaimed_events: ledger.reclaimed_events,
            exports_verified: 0,
            warnings: Vec::new(),
            failure: None,
            limits: VERIFICATION_LIMITS.to_vec(),
        };
        let outcome = if integrity.valid {
            verify_in(self, &tx, &rows, archives, &mut report)
        } else {
            Err(RetentionFailure::new(
                code::AUDIT_CHAIN_INVALID,
                "the live audit chain does not verify",
            )
            .with("integrity_error", integrity.error.clone()))
        };
        match outcome {
            Ok(()) => report.verdict = "verified",
            Err(refusal) => {
                report.failure = Some(VerificationFailureV1 {
                    code: refusal.code,
                    message: refusal.message,
                    detail: refusal.detail,
                });
            }
        }
        Ok(report)
    }
}

fn ledger_of(db: &Connection) -> anyhow::Result<RetentionLedgerV1> {
    let rows = ledger::segments(db)?;
    let segment_list: Vec<RetentionSegmentSummaryV1> = rows
        .iter()
        .map(|row| RetentionSegmentSummaryV1 {
            segment_index: row.index,
            archive_id: row.archive_id.clone(),
            manifest_sha256: row.manifest_sha256.clone(),
            first_seq: row.first_seq,
            last_seq: row.last_seq,
            last_hash: row.last_hash.clone(),
            reclaimed_events: row.reclaimed_events,
            reclaimed_bytes: row.reclaimed_bytes,
            retention_event_seq: row.event_seq,
        })
        .collect();
    Ok(RetentionLedgerV1 {
        capability: ledger::capability_present(db)?,
        segments: i64::try_from(rows.len())?,
        archived_through_seq: rows.last().map_or(0, |row| row.last_seq),
        reclaimed_events: rows.iter().map(|row| row.reclaimed_events).sum(),
        reclaimed_bytes: rows.iter().map(|row| row.reclaimed_bytes).sum(),
        last: segment_list.last().cloned(),
        segment_list,
    })
}

fn sql_refusal(error: impl std::fmt::Display) -> RetentionFailure {
    RetentionFailure::new(
        code::ARCHIVE_UNAVAILABLE,
        "the live store could not be read",
    )
    .with("cause", error.to_string())
}

fn same_row(live: &ArchivedEventV1, archived: &ArchivedEventV1) -> bool {
    live == archived
}

/// Replays the archives in order and recomputes what each segment had the
/// right to reclaim, its accounting, its floor and the legacy view snapshot.
/// The live comparison, when a store is at hand, is the caller's.
struct ArchiveReplay {
    selector: ReclaimSelector,
    fold: SnapshotFold,
    floor: Option<DateTime<Utc>>,
    previous_hash: String,
    previous_manifest: Option<String>,
    reclaimed_ids: Vec<String>,
}

/// Per-segment state of `ArchiveReplay`.
struct SegmentReplay {
    index: i64,
    last_seq: i64,
    classes: BTreeSet<ReclaimClassV1>,
    kept: BTreeSet<i64>,
    kept_seen: Vec<i64>,
    retained: RetainedDigest,
    reclaimed: Vec<i64>,
    by_class: BTreeMap<ReclaimClassV1, ClassUsageV1>,
    structural: i64,
    pins: i64,
}

/// What the ledger says of one segment, from the live store or the gateway.
struct SegmentClaim<'a> {
    index: i64,
    archive_id: &'a str,
    first_seq: i64,
    last_seq: i64,
    last_hash: &'a str,
    reclaimed_events: i64,
    floor_at_last: Option<&'a str>,
    snapshot_sha256: Option<&'a str>,
}

impl ArchiveReplay {
    fn new() -> Self {
        Self {
            selector: ReclaimSelector::default(),
            fold: SnapshotFold::empty(),
            floor: None,
            previous_hash: ZERO_HASH.to_owned(),
            previous_manifest: None,
            reclaimed_ids: Vec::new(),
        }
    }

    fn segment(&self, index: i64, last_seq: i64, summary: &ReclaimSummaryV1) -> SegmentReplay {
        SegmentReplay {
            index,
            last_seq,
            classes: summary.classes.iter().copied().collect(),
            kept: summary.kept_positions.iter().copied().collect(),
            kept_seen: Vec::new(),
            retained: RetainedDigest::new(),
            reclaimed: Vec::new(),
            by_class: BTreeMap::new(),
            structural: 0,
            pins: 0,
        }
    }

    /// The selection of one archived event: `Reclaim` means the segment had
    /// the right to remove it from the live store, anything else that it must
    /// still be there.
    fn observe(
        &mut self,
        segment: &mut SegmentReplay,
        archived: &ArchivedEventV1,
        event: &Event,
    ) -> Result<Selection, RetentionFailure> {
        let boundary = archived.seq == segment.last_seq;
        let selection = match self.selector.select(event, boundary, &segment.classes) {
            // A signed checkpoint position kept live counts as a pin.
            Selection::Reclaim(_) if segment.kept.contains(&archived.seq) => {
                segment.kept_seen.push(archived.seq);
                Selection::Boundary
            }
            other => other,
        };
        match selection {
            Selection::Reclaim(class) => {
                segment.reclaimed.push(archived.seq);
                let usage = segment.by_class.entry(class).or_default();
                usage.events += 1;
                usage.bytes += archived.payload.len() as u64;
                self.reclaimed_ids.push(archived.id.clone());
            }
            Selection::Structural => {
                segment.structural += 1;
                segment.retained.add(archived.seq, &archived.hash);
            }
            Selection::Boundary | Selection::FirstSighting => {
                segment.pins += 1;
                segment.retained.add(archived.seq, &archived.hash);
            }
        }
        self.fold.observe(archived.seq, event);
        let received = parse_time(&archived.received_at)
            .and_then(crate::audit::workload::projection::canonical_control_time)
            .map_err(|_| {
                RetentionFailure::new(
                    code::ARCHIVE_CHAIN_INVALID,
                    "an archived receive time is invalid",
                )
                .with("segment_index", segment.index)
                .with("seq", archived.seq)
            })?;
        self.floor = Some(
            self.floor
                .map_or(received, |previous| previous.max(received)),
        );
        Ok(selection)
    }

    /// Compare one replayed segment with its manifest and its ledger claim.
    fn close(
        &mut self,
        segment: SegmentReplay,
        opened: &OpenedArchive,
        claim: &SegmentClaim<'_>,
    ) -> Result<(), RetentionFailure> {
        let manifest = &opened.manifest;
        let summary = ReclaimSummaryV1 {
            classes: segment.classes.iter().copied().collect(),
            reclaimed_events: i64::try_from(segment.reclaimed.len()).unwrap_or(i64::MAX),
            reclaimed_bytes: segment.by_class.values().map(|usage| usage.bytes).sum(),
            reclaim_sha256: reclaim_sha256(&segment.reclaimed),
            by_class: segment.by_class,
            retained_structural: segment.structural,
            retained_pins: segment.pins,
            kept_positions: segment.kept_seen,
            retained_sha256: segment.retained.finish(),
        };
        let floor = self.floor.map(format_time);
        let consistent = manifest.purpose == ArchivePurposeV1::Retention
            && manifest.archive_id == claim.archive_id
            && manifest.segment_index == claim.index
            && manifest.first_seq == claim.first_seq
            && manifest.last_seq == claim.last_seq
            && manifest.last_hash == claim.last_hash
            && manifest.start_prev_hash == self.previous_hash
            && manifest.previous_manifest_sha256 == self.previous_manifest
            && manifest.floor_at_last == floor
            && claim
                .floor_at_last
                .is_none_or(|claimed| floor.as_deref() == Some(claimed))
            && claim
                .snapshot_sha256
                .is_none_or(|claimed| manifest.snapshot_sha256.as_deref() == Some(claimed))
            && manifest.reclaim.as_ref() == Some(&summary)
            && summary.reclaimed_events == claim.reclaimed_events;
        if !consistent {
            return Err(RetentionFailure::new(
                code::ARCHIVE_MANIFEST_MISMATCH,
                "the archive or its reclaim accounting differs from the live ledger",
            )
            .with("segment_index", claim.index));
        }
        let (_, recomputed) = snapshot::encode(&self.fold.snapshot())?;
        if manifest.snapshot_sha256.as_deref() != Some(recomputed.as_str()) {
            return Err(RetentionFailure::new(
                code::SNAPSHOT_MISMATCH,
                "the persisted legacy view snapshot differs from the archived history",
            )
            .with("segment_index", claim.index));
        }
        self.previous_hash = manifest.last_hash.clone();
        self.previous_manifest = Some(opened.manifest_sha256.clone());
        Ok(())
    }
}

fn missing_archive(index: i64, archive_id: &str, manifest_sha256: &str) -> RetentionFailure {
    RetentionFailure::new(
        code::ARCHIVE_MISSING,
        "the archive of a retention segment was not provided",
    )
    .with("segment_index", index)
    .with("archive_id", archive_id.to_owned())
    .with("manifest_sha256", manifest_sha256.to_owned())
}

/// Verify the retention archives of a tenant against the ledger reported by
/// its gateway (`GET /api/v1/integrity`), without the live store: the
/// gateway's own integrity check covers the live chain, its kept rows and
/// their per-segment digests; this covers every archived event.
pub fn verify_archive_chain(
    tenant_id: &str,
    ledger: &RetentionLedgerV1,
    archives: &[PathBuf],
) -> RetentionVerificationV1 {
    let mut report = RetentionVerificationV1 {
        format: VERIFICATION_FORMAT,
        verdict: "refused",
        tenant_id: tenant_id.to_owned(),
        live_head_seq: 0,
        live_head_hash: String::new(),
        stored_events: 0,
        segments: ledger.segments,
        segments_verified: 0,
        archived_through_seq: ledger.archived_through_seq,
        reclaimed_events: ledger.reclaimed_events,
        exports_verified: 0,
        warnings: Vec::new(),
        failure: None,
        limits: VERIFICATION_LIMITS.to_vec(),
    };
    let outcome = (|| -> Result<(), RetentionFailure> {
        let mut by_manifest: BTreeMap<String, OpenedArchive> = BTreeMap::new();
        for path in archives {
            let opened = read_manifest(path)?;
            if opened.manifest.tenant_id != tenant_id {
                return Err(RetentionFailure::new(
                    code::ARCHIVE_MANIFEST_MISMATCH,
                    "the archive belongs to another tenant",
                )
                .with("archive", opened.path.display().to_string()));
            }
            if opened.manifest.purpose == ArchivePurposeV1::Retention {
                by_manifest.insert(opened.manifest_sha256.clone(), opened);
            }
        }
        if i64::try_from(ledger.segment_list.len()).ok() != Some(ledger.segments) {
            return Err(RetentionFailure::new(
                code::AUDIT_CHAIN_INVALID,
                "the gateway ledger does not list every segment",
            ));
        }
        let mut replay = ArchiveReplay::new();
        for claim in &ledger.segment_list {
            let opened = by_manifest.remove(&claim.manifest_sha256).ok_or_else(|| {
                missing_archive(
                    claim.segment_index,
                    &claim.archive_id,
                    &claim.manifest_sha256,
                )
            })?;
            let summary = opened.manifest.reclaim.clone().ok_or_else(|| {
                RetentionFailure::new(
                    code::ARCHIVE_MANIFEST_INVALID,
                    "a retention archive has no reclaim summary",
                )
            })?;
            let mut segment = replay.segment(claim.segment_index, claim.last_seq, &summary);
            let path = opened.path.clone();
            read_archive(&path, |archived, event| {
                replay.observe(&mut segment, archived, event).map(|_| ())
            })?;
            replay.close(
                segment,
                &opened,
                &SegmentClaim {
                    index: claim.segment_index,
                    archive_id: &claim.archive_id,
                    first_seq: claim.first_seq,
                    last_seq: claim.last_seq,
                    last_hash: &claim.last_hash,
                    reclaimed_events: claim.reclaimed_events,
                    floor_at_last: None,
                    snapshot_sha256: None,
                },
            )?;
            report.segments_verified += 1;
        }
        for orphan in by_manifest.values() {
            report.warnings.push(json!({
                "code": "orphan_archive",
                "archive": orphan.path.display().to_string(),
                "archive_id": orphan.manifest.archive_id,
                "message": "a retention archive that the gateway ledger does not reference; nothing it holds was reclaimed"
            }));
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => report.verdict = "verified",
        Err(refusal) => {
            report.failure = Some(VerificationFailureV1 {
                code: refusal.code,
                message: refusal.message,
                detail: refusal.detail,
            })
        }
    }
    report
}

fn verify_in(
    store: &Store,
    db: &Connection,
    rows: &[SegmentRow],
    archives: &[PathBuf],
    report: &mut RetentionVerificationV1,
) -> Result<(), RetentionFailure> {
    let (namespace, genesis_seq, genesis_hash) =
        control_genesis(db).map_err(|error| sql_refusal(error))?;
    let mut retention: BTreeMap<String, OpenedArchive> = BTreeMap::new();
    let mut exports = Vec::new();
    for path in archives {
        let opened = read_manifest(path)?;
        let manifest = &opened.manifest;
        if manifest.tenant_id != store.tenant_id
            || manifest.control_namespace_uuid != namespace
            || manifest.control_genesis_seq != genesis_seq
            || manifest.control_genesis_hash != genesis_hash
        {
            return Err(RetentionFailure::new(
                code::ARCHIVE_MANIFEST_MISMATCH,
                "the archive belongs to another tenant or control namespace",
            )
            .with("archive", opened.path.display().to_string()));
        }
        match manifest.purpose {
            ArchivePurposeV1::Retention => {
                retention.insert(opened.manifest_sha256.clone(), opened);
            }
            ArchivePurposeV1::Export => exports.push(opened),
        }
    }
    let applied: Vec<RetentionAppliedV1> = {
        let mut statement = db
            .prepare("SELECT payload FROM events WHERE kind=?1 ORDER BY seq")
            .map_err(sql_refusal)?;
        let payloads = statement
            .query_map([ledger::APPLIED_KIND], |row| row.get::<_, String>(0))
            .map_err(sql_refusal)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_refusal)?;
        payloads
            .iter()
            .map(|payload| {
                let event: Event = serde_json::from_str(payload).map_err(sql_refusal)?;
                serde_json::from_value(event.data).map_err(sql_refusal)
            })
            .collect::<Result<_, _>>()?
    };
    if applied.len() != rows.len() {
        return Err(RetentionFailure::new(
            code::AUDIT_CHAIN_INVALID,
            "the retention ledger differs from its chain events",
        ));
    }
    let mut replay = ArchiveReplay::new();
    for (row, record) in rows.iter().zip(&applied) {
        let opened = retention
            .remove(&row.manifest_sha256)
            .ok_or_else(|| missing_archive(row.index, &row.archive_id, &row.manifest_sha256))?;
        if opened.manifest.events_sha256 != record.events_sha256
            || opened.manifest.reclaim.as_ref().is_none_or(|summary| {
                summary.reclaim_sha256 != record.reclaim_sha256
                    || summary.by_class != record.reclaimed_by_class
                    || summary.kept_positions != record.kept_positions
                    || summary.retained_sha256 != record.retained_sha256
                    || summary.classes != record.classes
            })
        {
            return Err(RetentionFailure::new(
                code::ARCHIVE_MANIFEST_MISMATCH,
                "the archive or its reclaim accounting differs from the live ledger",
            )
            .with("segment_index", row.index));
        }
        let summary = opened.manifest.reclaim.clone().expect("checked above");
        let mut segment = replay.segment(row.index, row.last_seq, &summary);
        let mut statement = db.prepare(RANGE_SQL).map_err(sql_refusal)?;
        let mut live = statement
            .query_map(params![row.first_seq, row.last_seq], row_archived)
            .map_err(sql_refusal)?
            .peekable();
        let path = opened.path.clone();
        read_archive(&path, |archived, event| {
            let selection = replay.observe(&mut segment, archived, event)?;
            let present = match live.peek() {
                Some(Ok(candidate)) if candidate.seq == archived.seq => true,
                Some(Err(_)) => return Err(sql_refusal("a live row is unreadable")),
                _ => false,
            };
            let at = |code: &'static str, message: &str| {
                RetentionFailure::new(code, message)
                    .with("segment_index", row.index)
                    .with("seq", archived.seq)
            };
            match (present, selection) {
                (true, Selection::Reclaim(_)) => Err(at(
                    code::RETAINED_EVENT_DIVERGES,
                    "an event that the segment reclaims is still in the live store",
                )),
                (true, _) => {
                    let candidate = live
                        .next()
                        .expect("peeked")
                        .map_err(|error| sql_refusal(error))?;
                    if same_row(&candidate, archived) {
                        Ok(())
                    } else {
                        Err(at(
                            code::RETAINED_EVENT_DIVERGES,
                            "a retained live event differs from its archived bytes",
                        ))
                    }
                }
                (false, Selection::Reclaim(_)) => Ok(()),
                (false, _) => Err(at(
                    code::UNACCOUNTED_LOSS,
                    "an event that no segment may reclaim is missing from the live store",
                )),
            }
        })?;
        replay.close(
            segment,
            &opened,
            &SegmentClaim {
                index: row.index,
                archive_id: &row.archive_id,
                first_seq: row.first_seq,
                last_seq: row.last_seq,
                last_hash: &row.last_hash,
                reclaimed_events: row.reclaimed_events,
                floor_at_last: Some(&row.floor_at_last),
                snapshot_sha256: Some(&row.snapshot_sha256),
            },
        )?;
        report.segments_verified += 1;
    }
    let reclaimed_ids = std::mem::take(&mut replay.reclaimed_ids);
    for id in &reclaimed_ids {
        let duplicated: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(sql_refusal)?;
        if duplicated {
            return Err(RetentionFailure::new(
                code::DUPLICATE_EVENT_ID,
                "a live event reuses the identifier of a reclaimed event",
            )
            .with("event_id", id.clone()));
        }
    }
    let base = rows.last().map_or(0, |row| row.last_seq);
    for export in exports {
        let manifest = &export.manifest;
        if manifest.first_seq <= base || manifest.last_seq > report.live_head_seq {
            return Err(RetentionFailure::new(
                code::EXPORT_DIVERGES,
                "the export covers events that the live store no longer holds contiguously",
            )
            .with("archive", export.path.display().to_string()));
        }
        let mut statement = db.prepare(RANGE_SQL).map_err(sql_refusal)?;
        let mut live = statement
            .query_map(params![manifest.first_seq, manifest.last_seq], row_archived)
            .map_err(sql_refusal)?;
        let path = export.path.clone();
        read_archive(&path, |archived, _| match live.next() {
            Some(Ok(candidate)) if same_row(&candidate, archived) => Ok(()),
            _ => Err(RetentionFailure::new(
                code::EXPORT_DIVERGES,
                "an exported event differs from the live store",
            )
            .with("seq", archived.seq)),
        })?;
        report.exports_verified += 1;
    }
    for orphan in retention.values() {
        report.warnings.push(json!({
            "code": "orphan_archive",
            "archive": orphan.path.display().to_string(),
            "archive_id": orphan.manifest.archive_id,
            "message": "a retention archive that the live ledger does not reference; nothing it holds was reclaimed"
        }));
    }
    Ok(())
}
