//! Diagnosis of the pieces involved in signed checkpoints.
//!
//! A diagnostic explains state; it verifies no signature and makes no trust
//! decision. A section that cannot be read carries its own structured error,
//! and nothing unreadable is replaced by an empty or zero value.

use super::{
    DIAGNOSTIC_FORMAT,
    anchor::{AnchorKey, AnchorReference, load_anchor},
    evidence::{AuditEvidence, read_audit_evidence},
    format::SignedRecord,
    keys,
    log::{CheckpointLog, IncompleteRecord, TolerantLoad},
    report::{Code, ErrorEntry, Failure, HeadSummary},
    verify::head_summary,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::Path;

const NOTICE: &str = "Diagnostic only: no signature is verified and no trust decision is made. Use verify with an independently held trust anchor.";
/// Log indexes listed per finding before the list is cut.
const LISTED: usize = 32;

pub struct DiagnoseRequest<'a> {
    /// Audit database and the tenant it is expected to be bound to.
    pub data: Option<(&'a Path, &'a str)>,
    pub checkpoint_dir: Option<&'a Path>,
    pub key_dir: Option<&'a Path>,
    pub anchor: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatabaseDiagnostic {
    pub readable: bool,
    pub tenant_id: Option<String>,
    pub namespace_uuid: Option<String>,
    pub audit_schema_version: Option<u32>,
    pub event_count: Option<u64>,
    pub head_hash: Option<String>,
    pub error: Option<ErrorEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogDiagnostic {
    pub readable: bool,
    pub tenant_id: Option<String>,
    pub namespace_uuid: Option<String>,
    pub complete_records: Option<u64>,
    pub first_log_index: Option<u64>,
    /// The complete record with the highest log index, unverified.
    pub head: Option<HeadSummary>,
    /// Key expected to sign the next record.
    pub next_signer_key_id: Option<String>,
    pub incomplete_records: Vec<IncompleteRecord>,
    pub unexpected_entries: Vec<String>,
    /// Entries that do not satisfy the private storage policy; never opened.
    pub rejected_entries: Option<u32>,
    pub enumeration_complete: Option<bool>,
    pub error: Option<ErrorEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyDiagnostic {
    pub readable: bool,
    pub key_id: Option<String>,
    pub error: Option<ErrorEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnchorDiagnostic {
    pub readable: bool,
    pub revision: Option<u64>,
    /// SHA-256 of this revision file. The next revision names it in
    /// `previous_anchor_sha256`.
    pub sha256: Option<String>,
    pub previous_anchor_sha256: Option<String>,
    pub tenant_id: Option<String>,
    pub namespace_uuid: Option<String>,
    pub keys: Vec<AnchorKey>,
    pub reference: Option<AnchorReference>,
    pub error: Option<ErrorEntry>,
}

/// An observation that deserves the operator's attention. Not a verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub message: String,
    pub detail: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnosticReport {
    pub format: &'static str,
    pub notice: &'static str,
    pub database: Option<DatabaseDiagnostic>,
    pub checkpoint_log: Option<LogDiagnostic>,
    pub signing_key: Option<KeyDiagnostic>,
    pub trust_anchor: Option<AnchorDiagnostic>,
    pub findings: Vec<Finding>,
}

fn finding(code: &'static str, message: &str) -> Finding {
    Finding {
        code,
        message: message.to_owned(),
        detail: Map::new(),
    }
}

impl Finding {
    fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.detail.insert(key.into(), value.into());
        self
    }
}

fn listed(indexes: &[u64]) -> Vec<u64> {
    indexes.iter().take(LISTED).copied().collect()
}

struct LogFacts {
    head: SignedRecord,
    lowest: u64,
}

fn diagnose_log(path: &Path, findings: &mut Vec<Finding>) -> (LogDiagnostic, Option<LogFacts>) {
    let unreadable = |failure: Failure| LogDiagnostic {
        readable: false,
        tenant_id: None,
        namespace_uuid: None,
        complete_records: None,
        first_log_index: None,
        head: None,
        next_signer_key_id: None,
        incomplete_records: Vec::new(),
        unexpected_entries: Vec::new(),
        rejected_entries: None,
        enumeration_complete: None,
        error: Some(failure.error),
    };
    let loaded = CheckpointLog::open_reader(path).and_then(|log| log.load_tolerant());
    let TolerantLoad {
        set,
        rejected_entries,
        enumeration_complete,
    } = match loaded {
        Ok(loaded) => loaded,
        Err(failure) => return (unreadable(failure), None),
    };
    if !set.incomplete.is_empty() {
        findings.push(
            finding(
                "incomplete_records_present",
                "Record files that are not complete records were found. They are ignored for the chain and never removed by this tool.",
            )
            .with("count", set.incomplete.len()),
        );
    }
    let renamed = set
        .incomplete
        .iter()
        .filter(|record| record.holds_another_record)
        .count();
    if renamed > 0 {
        findings.push(
            finding(
                "record_name_mismatch",
                "Record files hold a complete record other than the one their name announces. An interrupted write cannot produce this: verify, anchor advance, anchor approve-rotation, export, create and rotate-key refuse the directory.",
            )
            .with("count", renamed),
        );
    }
    if !set.unexpected.is_empty() {
        findings.push(
            finding(
                "unexpected_entries_present",
                "Entries that are not checkpoint records were found in the checkpoint directory.",
            )
            .with("count", set.unexpected.len()),
        );
    }
    if rejected_entries > 0 || !enumeration_complete {
        findings.push(
            finding(
                "rejected_entries_present",
                "Some entries do not satisfy the private storage policy or could not be enumerated. create and verify refuse this directory until they are dealt with.",
            )
            .with("rejected_entries", rejected_entries)
            .with("enumeration_complete", enumeration_complete),
        );
    }
    let lowest = set.records.keys().next().copied();
    let head_index = set.head_index();
    // Walk the indexes that hold a record, never the interval between them:
    // one record at a far index must not cost time or memory in proportion to
    // the distance. Every list is bounded by the number of records read.
    let mut forked = Vec::new();
    let mut broken = Vec::new();
    let mut missing_count = 0_u64;
    let mut missing_ranges = Vec::new();
    // The previous index, and its record when it holds exactly one.
    let mut previous: Option<(u64, Option<&SignedRecord>)> = None;
    for (&log_index, records) in &set.records {
        let mut linked_to = None;
        if let Some((before, record)) = previous {
            if log_index - before > 1 {
                missing_count = missing_count.saturating_add(log_index - before - 1);
                if missing_ranges.len() < LISTED {
                    missing_ranges.push(serde_json::json!({
                        "first_log_index": before + 1,
                        "last_log_index": log_index - 1,
                    }));
                }
            } else {
                linked_to = record;
            }
        }
        previous = Some(match records.as_slice() {
            [record] => {
                if linked_to.is_some_and(|before| {
                    record.statement().previous_statement_sha256() != before.statement_sha256()
                }) {
                    broken.push(log_index);
                }
                (log_index, Some(record))
            }
            _ => {
                forked.push(log_index);
                (log_index, None)
            }
        });
    }
    for (code, message, indexes) in [
        (
            "log_forked",
            "Several complete records claim the same log position.",
            &forked,
        ),
        (
            "log_link_broken",
            "Records do not link to the record that precedes them.",
            &broken,
        ),
    ] {
        if !indexes.is_empty() {
            findings.push(
                finding(code, message)
                    .with("count", indexes.len())
                    .with("log_indexes", listed(indexes)),
            );
        }
    }
    if missing_count > 0 {
        findings.push(
            finding(
                "log_gap",
                "Log positions between the first and the last record hold no complete record.",
            )
            .with("count", missing_count)
            .with("missing_ranges", missing_ranges),
        );
    }
    let head = head_index
        .map(|index| set.at(index))
        .and_then(|records| match records {
            [record] => Some(record.clone()),
            _ => None,
        });
    let diagnostic = LogDiagnostic {
        readable: true,
        tenant_id: head
            .as_ref()
            .map(|record| record.statement().tenant_id().to_owned()),
        namespace_uuid: head
            .as_ref()
            .map(|record| record.statement().namespace_uuid().to_owned()),
        complete_records: Some(
            set.records
                .values()
                .map(|records| records.len() as u64)
                .sum(),
        ),
        first_log_index: lowest,
        head: head.as_ref().map(head_summary),
        next_signer_key_id: head
            .as_ref()
            .map(|record| record.statement().next_signer().to_owned()),
        incomplete_records: set.incomplete,
        unexpected_entries: set.unexpected,
        rejected_entries: Some(rejected_entries),
        enumeration_complete: Some(enumeration_complete),
        error: None,
    };
    let facts = head
        .zip(lowest)
        .map(|(head, lowest)| LogFacts { head, lowest });
    (diagnostic, facts)
}

fn diagnose_database(
    path: &Path,
    tenant_id: &str,
    log: Option<&LogFacts>,
    findings: &mut Vec<Finding>,
) -> (DatabaseDiagnostic, Option<AuditEvidence>) {
    let position = log.map(|log| log.head.statement().signed_position());
    let positions: Vec<u64> = position
        .iter()
        .map(|position| position.event_count)
        .collect();
    let evidence = match read_audit_evidence(path, tenant_id, &positions) {
        Ok(evidence) => evidence,
        Err(failure) => {
            return (
                DatabaseDiagnostic {
                    readable: false,
                    tenant_id: None,
                    namespace_uuid: None,
                    audit_schema_version: None,
                    event_count: None,
                    head_hash: None,
                    error: Some(failure.error),
                },
                None,
            );
        }
    };
    if let (Some(log), Some(position)) = (log, position) {
        let statement = log.head.statement();
        if statement.tenant_id() != evidence.tenant_id
            || statement.namespace_uuid() != evidence.namespace_uuid
        {
            findings.push(
                finding(
                    "scope_differs",
                    "The checkpoint log and the audit database belong to different tenants or control namespaces.",
                )
                .with("between", vec!["checkpoint_log", "database"]),
            );
        } else if position.event_count > evidence.event_count {
            findings.push(
                finding(
                    "database_shorter_than_last_checkpoint",
                    "The audit database holds fewer events than the last record of the log signed.",
                )
                .with("last_signed_event_count", position.event_count)
                .with("database_event_count", evidence.event_count),
            );
        } else if evidence.hashes.get(&position.event_count) != Some(&position.head_hash) {
            findings.push(
                finding(
                    "database_differs_at_last_checkpoint",
                    "The audit database does not hold the chain hash that the last record of the log signed.",
                )
                .with("event_count", position.event_count),
            );
        } else {
            findings.push(
                finding(
                    "unsigned_tail",
                    "Events appended after the last signed position are not covered by any signature until the next checkpoint.",
                )
                .with(
                    "events_after_last_checkpoint",
                    evidence.event_count - position.event_count,
                )
                .with("last_signed_event_count", position.event_count)
                .with("database_event_count", evidence.event_count),
            );
        }
    }
    (
        DatabaseDiagnostic {
            readable: true,
            tenant_id: Some(evidence.tenant_id.clone()),
            namespace_uuid: Some(evidence.namespace_uuid.clone()),
            audit_schema_version: Some(evidence.audit_schema_version),
            event_count: Some(evidence.event_count),
            head_hash: Some(evidence.head_hash.clone()),
            error: None,
        },
        Some(evidence),
    )
}

fn diagnose_key(path: &Path, log: Option<&LogFacts>, findings: &mut Vec<Finding>) -> KeyDiagnostic {
    match keys::load_signing_key(path) {
        Ok(key) => {
            if let Some(log) = log {
                let expected = log.head.statement().next_signer();
                if expected != key.key_id() {
                    findings.push(
                        finding(
                            "key_is_not_next_signer",
                            "The signing key is not the key expected to sign the next record of the log. Use the expected key, or record a rotation with rotate-key.",
                        )
                        .with("key_id", key.key_id())
                        .with("expected_key_id", expected),
                    );
                }
            }
            KeyDiagnostic {
                readable: true,
                key_id: Some(key.key_id().to_owned()),
                error: None,
            }
        }
        Err(failure) => KeyDiagnostic {
            readable: false,
            key_id: None,
            error: Some(failure.error),
        },
    }
}

fn diagnose_anchor(
    path: &Path,
    log: Option<&LogFacts>,
    database: Option<&AuditEvidence>,
    findings: &mut Vec<Finding>,
) -> AnchorDiagnostic {
    let loaded = match load_anchor(path) {
        Ok(loaded) => loaded,
        Err(failure) => {
            return AnchorDiagnostic {
                readable: false,
                revision: None,
                sha256: None,
                previous_anchor_sha256: None,
                tenant_id: None,
                namespace_uuid: None,
                keys: Vec::new(),
                reference: None,
                error: Some(failure.error),
            };
        }
    };
    let anchor = loaded.anchor;
    if let Some(reference) = &anchor.reference
        && !reference.retained_checkpoints.is_empty()
    {
        findings.push(
            finding(
                "anchor_retains_uncompared_positions",
                "The trust anchor retains earlier positions that its reference moved past without a database comparison. They are compared whenever a database is given; anchor advance with --data confirms the continuity.",
            )
            .with("count", reference.retained_checkpoints.len()),
        );
    }
    if let Some(database) = database
        && (database.tenant_id != anchor.tenant_id
            || database.namespace_uuid != anchor.namespace_uuid)
    {
        findings.push(
            finding(
                "scope_differs",
                "The trust anchor and the audit database belong to different tenants or control namespaces.",
            )
            .with("between", vec!["trust_anchor", "database"]),
        );
    }
    if let Some(log) = log {
        let statement = log.head.statement();
        if statement.tenant_id() != anchor.tenant_id
            || statement.namespace_uuid() != anchor.namespace_uuid
        {
            findings.push(
                finding(
                    "scope_differs",
                    "The trust anchor and the checkpoint log belong to different tenants or control namespaces.",
                )
                .with("between", vec!["trust_anchor", "checkpoint_log"]),
            );
        }
        let head_index = log.head.log_index();
        match &anchor.reference {
            None => findings.push(finding(
                "anchor_has_no_reference",
                "The trust anchor carries no accepted reference: a rollback to an earlier signed state cannot be detected. Record one with anchor advance.",
            )),
            Some(reference) if reference.log_index > head_index => findings.push(
                finding(
                    "anchor_reference_ahead_of_log",
                    "The checkpoint log ends before the record accepted by the trust anchor. verify will refuse it as a rollback.",
                )
                .with("reference_log_index", reference.log_index)
                .with("log_head_index", head_index),
            ),
            Some(reference) if reference.log_index < head_index => findings.push(
                finding(
                    "records_after_anchor_reference",
                    "Records follow the anchor reference. They are protected against rollback only once anchor advance accepts them.",
                )
                .with("reference_log_index", reference.log_index)
                .with("log_head_index", head_index)
                .with("first_log_index_in_directory", log.lowest),
            ),
            Some(_) => {}
        }
        let expected = statement.next_signer();
        if anchor
            .keys
            .iter()
            .find(|key| key.valid_through_log_index.is_none())
            .is_none_or(|key| key.key_id != expected)
        {
            findings.push(
                finding(
                    "next_signer_not_current_anchor_key",
                    "The key expected to sign the next record is not the current key of the trust anchor. A key rotation may be waiting for anchor approve-rotation.",
                )
                .with("expected_key_id", expected),
            );
        }
    }
    AnchorDiagnostic {
        readable: true,
        revision: Some(anchor.revision),
        sha256: Some(loaded.sha256),
        previous_anchor_sha256: anchor.previous_anchor_sha256,
        tenant_id: Some(anchor.tenant_id),
        namespace_uuid: Some(anchor.namespace_uuid),
        keys: anchor.keys,
        reference: anchor.reference,
        error: None,
    }
}

/// Describe the requested pieces and how they relate. Reading failures are
/// reported per section; the command itself fails only on invalid arguments.
pub fn diagnose(request: &DiagnoseRequest<'_>) -> Result<DiagnosticReport, Failure> {
    if request.data.is_none()
        && request.checkpoint_dir.is_none()
        && request.key_dir.is_none()
        && request.anchor.is_none()
    {
        return Err(Failure::new(
            Code::InvalidArgument,
            "name at least one of the database, the checkpoint directory, the key directory or the trust anchor",
        ));
    }
    let mut findings = Vec::new();
    let (checkpoint_log, log_facts) = match request.checkpoint_dir {
        Some(path) => {
            let (diagnostic, facts) = diagnose_log(path, &mut findings);
            (Some(diagnostic), facts)
        }
        None => (None, None),
    };
    let (database, evidence) = match request.data {
        Some((path, tenant_id)) => {
            let (diagnostic, evidence) =
                diagnose_database(path, tenant_id, log_facts.as_ref(), &mut findings);
            (Some(diagnostic), evidence)
        }
        None => (None, None),
    };
    let signing_key = request
        .key_dir
        .map(|path| diagnose_key(path, log_facts.as_ref(), &mut findings));
    let trust_anchor = request
        .anchor
        .map(|path| diagnose_anchor(path, log_facts.as_ref(), evidence.as_ref(), &mut findings));
    Ok(DiagnosticReport {
        format: DIAGNOSTIC_FORMAT,
        notice: NOTICE,
        database,
        checkpoint_log,
        signing_key,
        trust_anchor,
        findings,
    })
}
