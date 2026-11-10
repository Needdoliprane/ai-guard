//! The proof bundle: a public, self-contained export of contiguous records.
//!
//! A bundle carries no key, public or private. Its header fields are a
//! convenience derived from the entries and are not authenticated: a verifier
//! reads the entries, checks every one against the trust anchor, and refuses a
//! bundle whose header disagrees with them.

use super::{
    MAX_BUNDLE_BYTES, MAX_BUNDLE_ENTRIES, PROOF_FORMAT, files,
    format::{SignedRecord, is_canonical_time, now, sha256_hex},
    log::{CheckpointLog, IncompleteRecord, RecordSet, forked, refuse_renamed_records},
    report::{Code, Failure},
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::path::Path;

const NOTICE: &str =
    "Public material. Contains no key. Trust comes only from an independently held trust anchor.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleIn<'a> {
    format: String,
    tenant_id: String,
    namespace_uuid: String,
    first_log_index: u64,
    last_log_index: u64,
    #[serde(borrow)]
    entries: Vec<&'a RawValue>,
    exported_at: String,
    #[allow(dead_code)]
    notice: String,
}

#[derive(Serialize)]
struct BundleOut<'a> {
    format: &'a str,
    tenant_id: &'a str,
    namespace_uuid: &'a str,
    first_log_index: u64,
    last_log_index: u64,
    entries: Vec<Box<RawValue>>,
    exported_at: String,
    notice: &'a str,
}

pub struct ExportRequest<'a> {
    pub checkpoint_dir: &'a Path,
    /// New file receiving the bundle.
    pub out: &'a Path,
    /// First log index to export; the lowest available by default.
    pub from_log_index: Option<u64>,
    /// Last log index to export; the head of the log by default.
    pub through_log_index: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportOutcome {
    pub bundle_file: String,
    pub sha256: String,
    pub bytes: u64,
    pub tenant_id: String,
    pub namespace_uuid: String,
    pub first_log_index: u64,
    pub last_log_index: u64,
    pub entries: u64,
    pub incomplete_records: Vec<IncompleteRecord>,
    pub notice: &'static str,
}

fn malformed(message: &str) -> Failure {
    Failure::new(Code::BundleMalformed, message)
}

/// The single complete record at `log_index` of a directory.
fn exportable(set: &RecordSet, log_index: u64) -> Result<&SignedRecord, Failure> {
    match set.at(log_index) {
        [record] => Ok(record),
        [] => Err(Failure::new(
            Code::LogDiscontinuous,
            "no complete checkpoint record is available at a log index of the requested range",
        )
        .with("missing_log_index", log_index)),
        records => Err(forked(log_index, records)),
    }
}

/// Export contiguous records of a checkpoint directory into a new public file.
/// Nothing is verified against a trust anchor here: the bundle is evidence to
/// be verified, not a verdict.
pub fn export_bundle(request: &ExportRequest<'_>) -> Result<ExportOutcome, Failure> {
    files::require_new_output(request.out)?;
    let set = CheckpointLog::open_reader(request.checkpoint_dir)?.load()?;
    refuse_renamed_records(&set.incomplete)?;
    let (Some(lowest), Some(head)) = (set.records.keys().next().copied(), set.head_index()) else {
        return Err(Failure::new(
            Code::CheckpointLogEmpty,
            "the checkpoint directory holds no complete record to export",
        )
        .action("Create a checkpoint first."));
    };
    let first = request.from_log_index.unwrap_or(lowest);
    let last = request.through_log_index.unwrap_or(head);
    if first < lowest || last > head || first > last {
        return Err(Failure::new(
            Code::InvalidArgument,
            "the requested log index range is not available in the checkpoint directory",
        )
        .with("available_first_log_index", lowest)
        .with("available_last_log_index", head));
    }
    // The reader refuses a bundle beyond this bound: never write one.
    let requested = last - first + 1;
    if requested > u64::from(MAX_BUNDLE_ENTRIES) {
        return Err(Failure::new(
            Code::InvalidArgument,
            "the requested range holds more records than one bundle may carry",
        )
        .with("requested_entries", requested)
        .with("limit", MAX_BUNDLE_ENTRIES)
        .action(
            "Export a shorter range with --from-log-index and --through-log-index. Bundles of consecutive ranges verify in order, advancing the anchor reference after each.",
        ));
    }
    let mut entries = Vec::new();
    let mut previous: Option<&SignedRecord> = None;
    for log_index in first..=last {
        let record = exportable(&set, log_index)?;
        if let Some(previous) = previous {
            let statement = record.statement();
            if statement.tenant_id() != previous.statement().tenant_id()
                || statement.namespace_uuid() != previous.statement().namespace_uuid()
            {
                return Err(Failure::new(
                    Code::ScopeMismatch,
                    "the checkpoint directory mixes records of several tenants or control namespaces",
                )
                .with("log_index", log_index));
            }
            if statement.previous_statement_sha256() != previous.statement_sha256() {
                return Err(Failure::new(
                    Code::LogDiscontinuous,
                    "a checkpoint record does not link to the record that precedes it",
                )
                .with("log_index", log_index));
            }
        }
        let text = String::from_utf8(record.to_bytes()?)
            .map_err(|_| malformed("a record is not UTF-8"))?;
        entries.push(RawValue::from_string(text).map_err(|_| malformed("a record is not JSON"))?);
        previous = Some(record);
    }
    let Some(scope) = previous else {
        return Err(malformed("the requested range holds no record"));
    };
    let tenant_id = scope.statement().tenant_id().to_owned();
    let namespace_uuid = scope.statement().namespace_uuid().to_owned();
    let count = entries.len() as u64;
    let mut bytes = serde_json::to_vec(&BundleOut {
        format: PROOF_FORMAT,
        tenant_id: &tenant_id,
        namespace_uuid: &namespace_uuid,
        first_log_index: first,
        last_log_index: last,
        entries,
        exported_at: now(),
        notice: NOTICE,
    })
    .map_err(|_| malformed("the bundle cannot be serialized"))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(Failure::new(
            Code::InvalidArgument,
            "the requested range exceeds the bundle size bound; export a smaller range",
        ));
    }
    let path = files::write_new(request.out, &bytes, MAX_BUNDLE_BYTES)?;
    Ok(ExportOutcome {
        bundle_file: path.display().to_string(),
        sha256: sha256_hex(&bytes),
        bytes: bytes.len() as u64,
        tenant_id,
        namespace_uuid,
        first_log_index: first,
        last_log_index: last,
        entries: count,
        incomplete_records: set.incomplete,
        notice: NOTICE,
    })
}

/// Read a bundle into the records it carries. Every entry must be a strict,
/// canonical record and the header must agree with the entries.
pub(crate) fn read_bundle(path: &Path) -> Result<RecordSet, Failure> {
    let bytes = files::read_input(path, MAX_BUNDLE_BYTES, Code::InputUnavailable, "bundle")?;
    crate::json::decode_unique(&bytes, 8).map_err(|error| {
        malformed("the bundle is not strict JSON").with("reason", error.to_string())
    })?;
    let bundle: BundleIn<'_> = serde_json::from_slice(&bytes).map_err(|error| {
        malformed("the bundle does not match its contract").with("reason", error.to_string())
    })?;
    if bundle.format != PROOF_FORMAT || !is_canonical_time(&bundle.exported_at) {
        return Err(malformed(
            "the bundle format or export time is not supported",
        ));
    }
    if bundle.entries.is_empty() || bundle.entries.len() > MAX_BUNDLE_ENTRIES as usize {
        return Err(malformed(
            "the bundle must carry between 1 and the maximum number of entries",
        ));
    }
    let mut set = RecordSet::default();
    for (position, entry) in bundle.entries.iter().enumerate() {
        let record = SignedRecord::parse(entry.get().as_bytes())
            .map_err(|failure| failure.with("bundle_entry", position))?;
        let statement = record.statement();
        let expected = bundle.first_log_index.checked_add(position as u64);
        if Some(record.log_index()) != expected
            || statement.tenant_id() != bundle.tenant_id
            || statement.namespace_uuid() != bundle.namespace_uuid
        {
            return Err(malformed(
                "the bundle header disagrees with its entries, or the entries are not contiguous",
            )
            .with("bundle_entry", position));
        }
        set.insert(record);
    }
    if set.head_index() != Some(bundle.last_log_index) {
        return Err(malformed("the bundle header disagrees with its last entry"));
    }
    Ok(set)
}
