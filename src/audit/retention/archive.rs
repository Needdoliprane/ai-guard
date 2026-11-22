//! Portable archive of a contiguous range of a tenant audit chain.
//!
//! A new private directory (mode 0700, created exclusively) holds
//! `events.jsonl`, one `ArchivedEventV1` per line in sequence order with the
//! exact hashed payload string, and `manifest.json`. The manifest binds the
//! range, its digests and, for a retention segment, the reclaim accounting.
//! The same format serves SQLite and MySQL; an `export` archive is the same
//! file set without any reclaim. Nothing here removes or rewrites a file.
use super::{
    chain::ZERO_HASH,
    classify::ReclaimClassV1,
    failure::{RetentionFailure, code},
};
use crate::{
    audit::{event_codec::digest, storage::StoreStorage},
    control_fs::{Directory, absolute_path},
    model::Event,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub const ARCHIVE_FORMAT: &str = "ai-guard.audit.archive.v1";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const MANIFEST_FILE: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// A stored payload is at most 128 KiB; JSON string escaping can expand it.
const MAX_LINE_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchivedEventV1 {
    pub seq: i64,
    pub id: String,
    pub received_at: String,
    pub prev_hash: String,
    pub hash: String,
    /// Exact stored payload, the string that the chain hashes.
    pub payload: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchivePurposeV1 {
    Retention,
    Export,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClassUsageV1 {
    pub events: i64,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReclaimSummaryV1 {
    pub classes: Vec<ReclaimClassV1>,
    pub reclaimed_events: i64,
    pub reclaimed_bytes: u64,
    /// SHA-256 of the domain-separated, ordered list of reclaimed sequences.
    pub reclaim_sha256: String,
    pub by_class: BTreeMap<ReclaimClassV1, ClassUsageV1>,
    pub retained_structural: i64,
    pub retained_pins: i64,
    /// Reclaimable events kept live because a signed checkpoint names them.
    pub kept_positions: Vec<i64>,
    /// Commitment to every row the segment keeps live.
    pub retained_sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProducerV1 {
    pub component: String,
    pub version: String,
}

impl ProducerV1 {
    pub(crate) fn current() -> Self {
        Self {
            component: "agentsandbox".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifestV1 {
    pub format: String,
    pub archive_id: String,
    pub purpose: ArchivePurposeV1,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub control_genesis_seq: i64,
    pub control_genesis_hash: String,
    pub backend: String,
    /// 1-based segment index of a retention archive, 0 for an export.
    pub segment_index: i64,
    pub first_seq: i64,
    pub start_prev_hash: String,
    pub last_seq: i64,
    pub last_hash: String,
    /// Highest receive time through `last_seq`, as the live time floor holds it.
    pub floor_at_last: Option<String>,
    pub event_count: i64,
    pub payload_bytes: u64,
    pub events_file: String,
    pub events_sha256: String,
    pub events_bytes: u64,
    pub previous_manifest_sha256: Option<String>,
    pub reclaim: Option<ReclaimSummaryV1>,
    pub snapshot_sha256: Option<String>,
    pub policy_sha256: Option<String>,
    /// Declared by the producer; not a trusted timestamp.
    pub created_at: String,
    pub producer: ProducerV1,
}

/// Fields of a manifest that the writer does not derive from the rows.
pub(crate) struct ManifestHeader {
    pub(crate) archive_id: String,
    pub(crate) purpose: ArchivePurposeV1,
    pub(crate) tenant_id: String,
    pub(crate) control_namespace_uuid: String,
    pub(crate) control_genesis_seq: i64,
    pub(crate) control_genesis_hash: String,
    pub(crate) backend: &'static str,
    pub(crate) segment_index: i64,
    pub(crate) floor_at_last: Option<String>,
    pub(crate) previous_manifest_sha256: Option<String>,
    pub(crate) reclaim: Option<ReclaimSummaryV1>,
    pub(crate) snapshot_sha256: Option<String>,
    pub(crate) policy_sha256: Option<String>,
    pub(crate) created_at: String,
}

pub(crate) struct WrittenArchive {
    pub(crate) path: PathBuf,
    pub(crate) manifest: ArchiveManifestV1,
    pub(crate) manifest_sha256: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_failure(message: &str, error: impl std::fmt::Display) -> RetentionFailure {
    RetentionFailure::new(code::ARCHIVE_WRITE_FAILED, message).with("cause", error.to_string())
}

pub(crate) struct ArchiveWriter {
    directory: Directory,
    path: PathBuf,
    file: BufWriter<File>,
    hasher: Sha256,
    bytes: u64,
    count: i64,
    payload_bytes: u64,
    first: Option<(i64, String)>,
    last: Option<(i64, String)>,
}

impl ArchiveWriter {
    /// Create the archive directory exclusively under an existing private
    /// parent. An existing path, even an empty directory, is a refusal.
    pub(super) fn create(path: &Path, storage: &StoreStorage) -> Result<Self, RetentionFailure> {
        let unavailable = |message: &str, error: &dyn std::fmt::Display| {
            RetentionFailure::new(code::DESTINATION_UNAVAILABLE, message)
                .with("path", path.display().to_string())
                .with("cause", error.to_string())
        };
        let path = absolute_path(path)
            .map_err(|error| unavailable("the archive path is not usable", &error))?;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(RetentionFailure::new(
                code::DESTINATION_UNAVAILABLE,
                "the archive path has no parent or name",
            ));
        };
        let name = name
            .to_str()
            .ok_or_else(|| {
                RetentionFailure::new(
                    code::DESTINATION_UNAVAILABLE,
                    "the archive name is not UTF-8",
                )
            })?
            .to_owned();
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(RetentionFailure::new(
                    code::DESTINATION_NOT_NEW,
                    "the archive destination already exists; choose a new path",
                )
                .with("path", path.display().to_string()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(unavailable("the archive path cannot be inspected", &error)),
        }
        let parent = Directory::open_existing(parent).map_err(|error| {
            unavailable(
                "the archive parent must be an existing private directory",
                &error,
            )
        })?;
        if storage
            .reserves_directory_name(&parent, &name)
            .map_err(|error| {
                unavailable(
                    "the archive parent could not be compared with SQLite storage",
                    &error,
                )
            })?
        {
            return Err(RetentionFailure::new(
                code::DESTINATION_RESERVED,
                "choose an ASCII name outside the SQLite namespace or another private parent",
            ));
        }
        let directory = parent.exclusive_child(&name).map_err(|error| {
            if std::fs::symlink_metadata(&path).is_ok() {
                RetentionFailure::new(
                    code::DESTINATION_NOT_NEW,
                    "the archive destination already exists; choose a new path",
                )
                .with("path", path.display().to_string())
            } else {
                unavailable("the archive directory could not be created", &error)
            }
        })?;
        let file = directory
            .create_private_entry(OsStr::new(EVENTS_FILE))
            .map_err(|error| {
                write_failure("the archive events file could not be created", error)
            })?;
        Ok(Self {
            directory,
            path,
            file: BufWriter::new(file),
            hasher: Sha256::new(),
            bytes: 0,
            count: 0,
            payload_bytes: 0,
            first: None,
            last: None,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Append the next row; the range must stay a contiguous, valid chain.
    pub(crate) fn push(&mut self, row: &ArchivedEventV1) -> Result<(), RetentionFailure> {
        let linked = match &self.last {
            Some((seq, hash)) => seq.checked_add(1) == Some(row.seq) && &row.prev_hash == hash,
            None => row.seq >= 1,
        };
        if !linked || row.hash != digest(row.seq, &row.received_at, &row.prev_hash, &row.payload) {
            return Err(RetentionFailure::new(
                code::AUDIT_CHAIN_INVALID,
                "the archived range is not a contiguous valid chain",
            )
            .with("seq", row.seq));
        }
        let mut line = serde_json::to_vec(row)
            .map_err(|error| write_failure("an archived row could not be encoded", error))?;
        line.push(b'\n');
        self.file
            .write_all(&line)
            .map_err(|error| write_failure("an archived row could not be written", error))?;
        self.hasher.update(&line);
        self.bytes += line.len() as u64;
        self.count += 1;
        self.payload_bytes += row.payload.len() as u64;
        if self.first.is_none() {
            self.first = Some((row.seq, row.prev_hash.clone()));
        }
        self.last = Some((row.seq, row.hash.clone()));
        Ok(())
    }

    /// Make the events durable, then write the manifest exclusively.
    pub(crate) fn finish(self, header: ManifestHeader) -> Result<WrittenArchive, RetentionFailure> {
        let Self {
            directory,
            path,
            file,
            hasher,
            bytes,
            count,
            payload_bytes,
            first,
            last,
        } = self;
        let (Some((first_seq, start_prev_hash)), Some((last_seq, last_hash))) = (first, last)
        else {
            return Err(RetentionFailure::new(
                code::RANGE_INVALID,
                "an archive holds at least one event",
            ));
        };
        let file = file
            .into_inner()
            .map_err(|error| write_failure("the archive events could not be flushed", error))?;
        file.sync_all().map_err(|error| {
            write_failure("the archive events could not be synchronized", error)
        })?;
        drop(file);
        let manifest = ArchiveManifestV1 {
            format: ARCHIVE_FORMAT.into(),
            archive_id: header.archive_id,
            purpose: header.purpose,
            tenant_id: header.tenant_id,
            control_namespace_uuid: header.control_namespace_uuid,
            control_genesis_seq: header.control_genesis_seq,
            control_genesis_hash: header.control_genesis_hash,
            backend: header.backend.into(),
            segment_index: header.segment_index,
            first_seq,
            start_prev_hash,
            last_seq,
            last_hash,
            floor_at_last: header.floor_at_last,
            event_count: count,
            payload_bytes,
            events_file: EVENTS_FILE.into(),
            events_sha256: hex::encode(hasher.finalize()),
            events_bytes: bytes,
            previous_manifest_sha256: header.previous_manifest_sha256,
            reclaim: header.reclaim,
            snapshot_sha256: header.snapshot_sha256,
            policy_sha256: header.policy_sha256,
            created_at: header.created_at,
            producer: ProducerV1::current(),
        };
        let mut encoded = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| write_failure("the archive manifest could not be encoded", error))?;
        encoded.push(b'\n');
        let mut manifest_file = directory
            .create_private_entry(OsStr::new(MANIFEST_FILE))
            .map_err(|error| write_failure("the archive manifest could not be created", error))?;
        manifest_file
            .write_all(&encoded)
            .and_then(|()| manifest_file.sync_all())
            .map_err(|error| write_failure("the archive manifest could not be written", error))?;
        drop(manifest_file);
        directory.sync().map_err(|error| {
            write_failure("the archive directory could not be synchronized", error)
        })?;
        Ok(WrittenArchive {
            path,
            manifest,
            manifest_sha256: sha256_hex(&encoded),
        })
    }
}

pub(crate) struct OpenedArchive {
    pub(crate) path: PathBuf,
    pub(crate) manifest: ArchiveManifestV1,
    pub(crate) manifest_sha256: String,
}

fn manifest_invalid(path: &Path, message: &str) -> RetentionFailure {
    RetentionFailure::new(code::ARCHIVE_MANIFEST_INVALID, message)
        .with("archive", path.display().to_string())
}

/// Read and check only the manifest of an archive.
pub(crate) fn read_manifest(path: &Path) -> Result<OpenedArchive, RetentionFailure> {
    let unavailable = |error: anyhow::Error| {
        RetentionFailure::new(
            code::ARCHIVE_UNAVAILABLE,
            "the archive is not a readable private directory",
        )
        .with("archive", path.display().to_string())
        .with("cause", error.to_string())
    };
    let path = absolute_path(path).map_err(unavailable)?;
    let directory = Directory::open_existing(&path).map_err(unavailable)?;
    let held = directory
        .read_private_file(MANIFEST_FILE, MAX_MANIFEST_BYTES)
        .map_err(unavailable)?;
    let manifest: ArchiveManifestV1 = serde_json::from_slice(held.bytes())
        .map_err(|_| manifest_invalid(&path, "the archive manifest is not a version 1 manifest"))?;
    let digests = [
        manifest.start_prev_hash.as_str(),
        manifest.last_hash.as_str(),
        manifest.events_sha256.as_str(),
        manifest.control_genesis_hash.as_str(),
    ];
    if manifest.format != ARCHIVE_FORMAT
        || manifest.events_file != EVENTS_FILE
        || uuid::Uuid::parse_str(&manifest.archive_id).is_err()
        || manifest.first_seq < 1
        || manifest.last_seq < manifest.first_seq
        || manifest
            .last_seq
            .checked_sub(manifest.first_seq)
            .and_then(|span| span.checked_add(1))
            != Some(manifest.event_count)
        || !digests.iter().all(|value| valid_hex(value))
        || (manifest.purpose == ArchivePurposeV1::Retention)
            != (manifest.segment_index >= 1
                && manifest.reclaim.is_some()
                && manifest.snapshot_sha256.is_some())
        || (manifest.purpose == ArchivePurposeV1::Export && manifest.segment_index != 0)
    {
        return Err(manifest_invalid(
            &path,
            "the archive manifest is inconsistent",
        ));
    }
    Ok(OpenedArchive {
        path,
        manifest,
        manifest_sha256: held.digest().to_owned(),
    })
}

pub(crate) fn valid_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Read a whole archive: the manifest, then every row in order, checking the
/// chain, the digests and the counts. `visit` sees each verified row with its
/// decoded event and may refuse it. Nothing is written.
pub(crate) fn read_archive(
    path: &Path,
    mut visit: impl FnMut(&ArchivedEventV1, &Event) -> Result<(), RetentionFailure>,
) -> Result<OpenedArchive, RetentionFailure> {
    let opened = read_manifest(path)?;
    let path = opened.path.clone();
    let manifest = &opened.manifest;
    let unavailable = |error: &dyn std::fmt::Display| {
        RetentionFailure::new(
            code::ARCHIVE_UNAVAILABLE,
            "the archive events are unreadable",
        )
        .with("archive", path.display().to_string())
        .with("cause", error.to_string())
    };
    let chain_invalid = |seq: i64| {
        RetentionFailure::new(
            code::ARCHIVE_CHAIN_INVALID,
            "the archived events do not form the chain the manifest describes",
        )
        .with("archive", path.display().to_string())
        .with("seq", seq)
    };
    let directory = Directory::open_existing(&path).map_err(|error| unavailable(&error))?;
    let (file, before) = directory
        .open_private_stream(EVENTS_FILE)
        .map_err(|error| unavailable(&error))?;
    if before.len() != manifest.events_bytes {
        return Err(RetentionFailure::new(
            code::ARCHIVE_DIGEST_MISMATCH,
            "the archive events size differs from its manifest",
        )
        .with("archive", path.display().to_string()));
    }
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut previous: Option<(i64, String)> = None;
    let mut count = 0_i64;
    let mut payload_bytes = 0_u64;
    let mut total = 0_u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = Read::by_ref(&mut reader)
            .take(MAX_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| unavailable(&error))?;
        if read == 0 {
            break;
        }
        let expected = previous
            .as_ref()
            .map_or(manifest.first_seq, |(seq, _)| seq.saturating_add(1));
        if read as u64 > MAX_LINE_BYTES || line.last() != Some(&b'\n') {
            return Err(chain_invalid(expected));
        }
        hasher.update(&line);
        total += read as u64;
        let row: ArchivedEventV1 =
            serde_json::from_slice(&line[..line.len() - 1]).map_err(|_| chain_invalid(expected))?;
        let prev = previous
            .as_ref()
            .map_or(manifest.start_prev_hash.as_str(), |(_, hash)| hash.as_str());
        if row.seq != expected
            || row.prev_hash != prev
            || row.hash != digest(row.seq, &row.received_at, &row.prev_hash, &row.payload)
            || (row.seq == 1 && row.prev_hash != ZERO_HASH)
        {
            return Err(chain_invalid(expected));
        }
        let event: Event =
            serde_json::from_str(&row.payload).map_err(|_| chain_invalid(row.seq))?;
        if event.id != row.id {
            return Err(chain_invalid(row.seq));
        }
        visit(&row, &event)?;
        count += 1;
        payload_bytes += row.payload.len() as u64;
        previous = Some((row.seq, row.hash.clone()));
    }
    let after = reader
        .get_ref()
        .metadata()
        .map_err(|error| unavailable(&error))?;
    let last = previous.unwrap_or((0, ZERO_HASH.to_owned()));
    if count != manifest.event_count
        || last != (manifest.last_seq, manifest.last_hash.clone())
        || payload_bytes != manifest.payload_bytes
    {
        return Err(chain_invalid(last.0.saturating_add(1)));
    }
    if total != manifest.events_bytes
        || hex::encode(hasher.finalize()) != manifest.events_sha256
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.ino() != before.ino()
    {
        return Err(RetentionFailure::new(
            code::ARCHIVE_DIGEST_MISMATCH,
            "the archive events differ from their manifest digest",
        )
        .with("archive", path.display().to_string()));
    }
    Ok(opened)
}
