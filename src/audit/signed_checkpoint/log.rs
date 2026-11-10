//! The checkpoint log: immutable, content-addressed record files in a private
//! directory of the control, outside the audited SQLite database.
//!
//! A record is created exclusively as `cp-<log index>-<statement sha256>.json`
//! and is never renamed, rewritten or removed. A file with a record name whose
//! content is not the strict, canonical record named by its hash is an
//! *incomplete record*: it is reported, kept, and ignored for the chain. This
//! is how an interrupted write is survived, and it gives a forger nothing more
//! than deleting the file would.
//!
//! One kind of incomplete record is never tolerated: a file that holds a
//! complete, canonical record other than the one its name announces. An
//! exclusive write that was interrupted leaves a prefix of the intended bytes;
//! it cannot leave another well-formed record. Such a file was altered or
//! replaced after it was written: every command that reads the records
//! (`verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create`
//! and `rotate-key`) refuses a directory that holds one.
//!
//! A complete record is trusted for what attaches it to a trusted record, not
//! for being in the directory. The records under a trusted one (the reference
//! of a trust anchor for a verification, the head of the log for a writer)
//! must be the chain that it commits to through `previous_statement_sha256`:
//! [`RecordSet::chain_below`] refuses a second record at one index, another
//! record than the one named by the record above it, and a record that stands
//! below an index that holds none.

use super::{
    MAX_LOG_RECORDS, MAX_RECORD_BYTES,
    format::{SignedGenesis, SignedRecord, parse_record_file_name},
    report::{Code, Failure},
};
use crate::control_fs::{Directory, PrivateEntryIdentity, PrivateFile, persistent::Entry};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Entries tolerated beside the records before enumeration is refused.
const ENTRY_MARGIN: u32 = 64;
const LOCK_FILE: &str = ".writer.lock";

/// Record names listed in one refusal before the list is cut.
const LISTED: usize = 32;

/// A record file that cannot be part of the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncompleteRecord {
    pub name: String,
    pub reason: String,
    /// True when the file holds a complete, canonical record other than the
    /// one its name announces. An interrupted write cannot produce this: the
    /// file was altered or replaced.
    pub holds_another_record: bool,
}

/// Refuse a directory in which a record file holds another complete record
/// than the one it is named after.
pub(crate) fn refuse_renamed_records(incomplete: &[IncompleteRecord]) -> Result<(), Failure> {
    let renamed: Vec<&str> = incomplete
        .iter()
        .filter(|record| record.holds_another_record)
        .map(|record| record.name.as_str())
        .collect();
    if renamed.is_empty() {
        return Ok(());
    }
    Err(Failure::new(
        Code::RecordNameMismatch,
        "a record file holds a complete record other than the one its name announces",
    )
    .with("count", renamed.len())
    .with(
        "records",
        renamed.into_iter().take(LISTED).collect::<Vec<_>>(),
    ))
}

/// Records available to a verification, by log index.
#[derive(Debug, Default)]
pub(crate) struct RecordSet {
    pub records: BTreeMap<u64, Vec<SignedRecord>>,
    pub incomplete: Vec<IncompleteRecord>,
    pub unexpected: Vec<String>,
}

impl RecordSet {
    pub(crate) fn head_index(&self) -> Option<u64> {
        self.records.keys().next_back().copied()
    }

    pub(crate) fn at(&self, log_index: u64) -> &[SignedRecord] {
        self.records
            .get(&log_index)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn insert(&mut self, record: SignedRecord) {
        self.records
            .entry(record.log_index())
            .or_default()
            .push(record);
    }

    /// Authenticate by transitivity the complete records at and below a
    /// trusted record, named by its log index and statement hash: each one
    /// must be the single record at its index and carry the hash that the
    /// record above it names in `previous_statement_sha256`. No signature is
    /// needed for that: the trusted hash commits to the whole chain under it.
    ///
    /// The walk stops at the first index that holds no complete record. The
    /// set may start there: a continuation directory, a partial export and a
    /// partial copy hold nothing below their first record. A complete record
    /// that stands below that index is attached to the trusted record by no
    /// chain, and is a break.
    ///
    /// Returns the lowest log index that was attached, or `None` when the set
    /// does not hold the trusted record itself.
    pub(crate) fn chain_below(
        &self,
        log_index: u64,
        statement_sha256: &str,
    ) -> Result<Option<u64>, ChainBreak<'_>> {
        let mut expected = statement_sha256.to_owned();
        let mut index = log_index;
        let mut lowest = None;
        loop {
            match self.at(index) {
                [] => break,
                [record] if record.statement_sha256() == expected => {
                    lowest = Some(index);
                    if index == 1 {
                        return Ok(lowest);
                    }
                    expected = record.statement().previous_statement_sha256().to_owned();
                    index -= 1;
                }
                [record] => {
                    return Err(ChainBreak::AnotherRecord {
                        log_index: index,
                        expected_statement_sha256: expected,
                        actual_statement_sha256: record.statement_sha256().to_owned(),
                    });
                }
                records => {
                    return Err(ChainBreak::Forked {
                        log_index: index,
                        records,
                    });
                }
            }
        }
        match self.records.range(..index).next_back() {
            None => Ok(lowest),
            Some((&unattached_log_index, _)) => Err(ChainBreak::Unattached {
                missing_log_index: index,
                unattached_log_index,
            }),
        }
    }

    /// The control genesis named by the most recent checkpoint of a chain
    /// that [`Self::chain_below`] attached from `lowest` to `highest`. `None`
    /// when that part of the chain holds rotations only: a rotation names no
    /// genesis.
    pub(crate) fn last_signed_genesis(&self, lowest: u64, highest: u64) -> Option<SignedGenesis> {
        (lowest..=highest)
            .rev()
            .find_map(|log_index| match self.at(log_index) {
                [record] => SignedGenesis::of(record),
                _ => None,
            })
    }
}

/// Why the complete records under a trusted record are not the chain that it
/// commits to.
#[derive(Debug)]
pub(crate) enum ChainBreak<'a> {
    /// Two complete records claim one log index.
    Forked {
        log_index: u64,
        records: &'a [SignedRecord],
    },
    /// The record at this index is not the one named by the record above it,
    /// or by the trusted hash itself.
    AnotherRecord {
        log_index: u64,
        expected_statement_sha256: String,
        actual_statement_sha256: String,
    },
    /// No complete record stands at `missing_log_index`, and complete records
    /// stand below it: `unattached_log_index` is the highest of them.
    Unattached {
        missing_log_index: u64,
        unattached_log_index: u64,
    },
}

impl ChainBreak<'_> {
    /// The refusal of a writer: the trusted record is the head of the log.
    fn below_head(self, head_index: u64) -> Failure {
        match self {
            Self::Forked { log_index, records } => forked(log_index, records),
            Self::AnotherRecord {
                log_index,
                expected_statement_sha256,
                actual_statement_sha256,
            } => Failure::new(
                Code::LogDiscontinuous,
                "a record below the head of the log is not the record that the head chains to; nothing was signed",
            )
            .with("log_index", log_index)
            .with("head_log_index", head_index)
            .with("expected_statement_sha256", expected_statement_sha256)
            .with("actual_statement_sha256", actual_statement_sha256),
            Self::Unattached {
                missing_log_index,
                unattached_log_index,
            } => Failure::new(
                Code::LogDiscontinuous,
                "complete records stand below a log index that holds none: no chain attaches them to the head of the log; nothing was signed",
            )
            .with("missing_log_index", missing_log_index)
            .with("unattached_log_index", unattached_log_index)
            .with("head_log_index", head_index),
        }
    }
}

/// The record a new record must link to.
pub(crate) struct LogHead {
    pub record: Option<SignedRecord>,
    /// Incomplete record files found at or above the head index.
    pub incomplete: Vec<IncompleteRecord>,
    /// Record-named files in the directory, complete or not.
    pub record_files: usize,
}

/// The head of a log that holds at least one complete record and in which
/// every complete record below the head is the chain that the head commits
/// to, with what the reading found beside it.
pub(crate) struct ExtendableHead {
    pub record: SignedRecord,
    /// The control genesis named by the most recent checkpoint at or below
    /// the head. `None` when the directory holds rotations only down to its
    /// first record.
    pub signed_genesis: Option<SignedGenesis>,
    /// Incomplete record files found at or above the head index.
    pub incomplete: Vec<IncompleteRecord>,
    /// Record-named files in the directory, complete or not.
    pub record_files: usize,
}

struct RecordFile {
    name: String,
    log_index: u64,
    bytes: u64,
}

struct Listing {
    records: Vec<RecordFile>,
    unexpected: Vec<String>,
}

impl Listing {
    /// Sort directory entries into record files, the writer lock and the rest.
    fn of(entries: Vec<Entry>) -> Self {
        let mut listing = Self {
            records: Vec::new(),
            unexpected: Vec::new(),
        };
        for entry in entries {
            let record_index = parse_record_file_name(&entry.name).map(|(log_index, _)| log_index);
            match record_index {
                Some(log_index) if !entry.directory => listing.records.push(RecordFile {
                    name: entry.name,
                    log_index,
                    bytes: entry.bytes,
                }),
                _ if entry.name == LOCK_FILE && !entry.directory => {}
                _ => listing.unexpected.push(entry.name),
            }
        }
        listing
            .records
            .sort_by(|left, right| left.name.cmp(&right.name));
        listing.unexpected.sort();
        listing
    }
}

/// A best-effort reading of a directory, for diagnosis only. Entries that do
/// not satisfy the private storage policy are counted and never opened.
pub(crate) struct TolerantLoad {
    pub set: RecordSet,
    pub rejected_entries: u32,
    pub enumeration_complete: bool,
}

pub(crate) struct CheckpointLog {
    directory: Directory,
    path: PathBuf,
    exclusions: Vec<PrivateEntryIdentity>,
    /// Held for the lifetime of a writer; dropping the file releases the lock.
    _writer_lock: Option<PrivateFile>,
}

fn unavailable(error: &anyhow::Error) -> Failure {
    Failure::new(
        Code::CheckpointLogUnavailable,
        "the checkpoint directory does not satisfy the private storage policy",
    )
    .cause(error)
}

/// Identity of the audit database file and the directory that holds it, used
/// to keep SQLite files out of the record and key namespaces.
pub(crate) fn database_identity(data: &Path) -> Result<(PathBuf, PrivateEntryIdentity), Failure> {
    let path = super::files::absolute(data)?;
    let unreadable = |error: Option<&anyhow::Error>| {
        let failure = Failure::new(
            Code::AuditUnavailable,
            "the audit database is not an existing private file in a private directory",
        )
        .with("path", path.display().to_string());
        match error {
            Some(error) => failure.cause(error),
            None => failure,
        }
    };
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(unreadable(None));
    };
    let directory = Directory::open_existing(parent).map_err(|error| unreadable(Some(&error)))?;
    let identity = directory
        .private_entry(name)
        .map_err(|error| unreadable(Some(&error)))?
        .ok_or_else(|| unreadable(None))?;
    Ok((parent.to_owned(), identity))
}

impl CheckpointLog {
    /// Inspect a continuation destination before a no-op, without creating
    /// either the directory or its writer lock. Apply the same first-record
    /// policy as a writer. A missing destination remains missing; any other
    /// metadata or storage-policy failure is an error, never an empty log.
    pub(crate) fn starting_head_if_present(
        path: &Path,
        exclusions: Vec<PrivateEntryIdentity>,
        startable: impl Fn(&LogHead) -> Result<(), Failure>,
    ) -> Result<Option<LogHead>, Failure> {
        let path = super::files::absolute(path)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unavailable(&error.into())),
        }
        let log = Self {
            directory: Directory::open_existing(&path).map_err(|error| unavailable(&error))?,
            path,
            exclusions,
            _writer_lock: None,
        };
        let head = log.head()?;
        startable(&head)?;
        Ok(Some(head))
    }

    /// Open a log to write its first record: create a missing private
    /// directory, take the exclusive writer lock and read what the directory
    /// holds.
    ///
    /// `startable` refuses a directory in which a first record may not be
    /// written. A directory that already exists is first read without the
    /// lock, so that such a refusal creates nothing in it, not even the lock
    /// file. It is then read again under the lock: another writer may have
    /// appended in between, and only that second reading is used.
    pub(crate) fn open_starting_writer(
        path: &Path,
        exclusions: Vec<PrivateEntryIdentity>,
        startable: impl Fn(&LogHead) -> Result<(), Failure>,
    ) -> Result<(Self, LogHead), Failure> {
        let path = super::files::absolute(path)?;
        let created = match std::fs::symlink_metadata(&path) {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(unavailable(&error.into())),
        };
        let directory = Directory::open_or_create(&path).map_err(|error| unavailable(&error))?;
        if created {
            // `open_or_create` does not synchronize the ancestors it created.
            directory.durable_bootstrap().map_err(|error| {
                super::files::io_failure(
                    "the new checkpoint directory could not be made durable",
                    &error,
                )
            })?;
        }
        let mut log = Self {
            directory,
            path,
            exclusions,
            _writer_lock: None,
        };
        if !created {
            startable(&log.head()?)?;
        }
        log._writer_lock = Some(Self::writer_lock(&log.directory, &log.exclusions)?);
        let head = log.head()?;
        startable(&head)?;
        Ok((log, head))
    }

    /// Open an existing log for appending, take its exclusive writer lock and
    /// read the record that a new record must link to.
    ///
    /// A missing directory is an error, and so are a directory that holds no
    /// complete record (`empty` builds that failure), one in which two
    /// complete records stand at the head and one in which a record file at
    /// or above the head holds another record than the one it is named after.
    /// The head is first read without the lock, so that such a refusal
    /// creates nothing, not even the lock file.
    ///
    /// Every record is then read under the lock, and only that second reading
    /// is used: another writer may have appended in between, and a writer
    /// extends the log only when every complete record of the directory is
    /// the chain that the head commits to. That reading costs one file per
    /// record, bounded by `MAX_LOG_RECORDS` and the rotation reserve; a
    /// refusal it decides leaves the lock file in a directory that had none.
    pub(crate) fn open_existing_writer(
        path: &Path,
        exclusions: Vec<PrivateEntryIdentity>,
        empty: impl Fn() -> Failure,
    ) -> Result<(Self, ExtendableHead), Failure> {
        let path = super::files::absolute(path)?;
        let directory = Directory::open_existing(&path).map_err(|error| unavailable(&error))?;
        let mut log = Self {
            directory,
            path,
            exclusions,
            _writer_lock: None,
        };
        let unlocked = log.head()?;
        refuse_renamed_records(&unlocked.incomplete)?;
        if unlocked.record.is_none() {
            return Err(empty());
        }
        log._writer_lock = Some(Self::writer_lock(&log.directory, &log.exclusions)?);
        let head = log.extendable_head()?.ok_or_else(empty)?;
        Ok((log, head))
    }

    /// The head of the log when it holds a complete record, after every
    /// record of the directory was read. Refusals: a record file that holds
    /// another complete record than the one it is named after, two complete
    /// records at one index, a record that the head does not chain to, and a
    /// record that stands below an index that holds none.
    fn extendable_head(&self) -> Result<Option<ExtendableHead>, Failure> {
        let listing = self.listing()?;
        let record_files = listing.records.len();
        let set = self.classify(listing)?;
        refuse_renamed_records(&set.incomplete)?;
        let Some(head_index) = set.head_index() else {
            return Ok(None);
        };
        let record = match set.at(head_index) {
            [record] => record.clone(),
            records => return Err(forked(head_index, records)),
        };
        let signed_genesis = set
            .chain_below(head_index, record.statement_sha256())
            .map_err(|chain_break| chain_break.below_head(head_index))?
            .and_then(|lowest| set.last_signed_genesis(lowest, head_index));
        let incomplete = set
            .incomplete
            .into_iter()
            .filter(|incomplete| {
                parse_record_file_name(&incomplete.name)
                    .is_some_and(|(log_index, _)| log_index >= head_index)
            })
            .collect();
        Ok(Some(ExtendableHead {
            record,
            signed_genesis,
            incomplete,
            record_files,
        }))
    }

    fn writer_lock(
        directory: &Directory,
        exclusions: &[PrivateEntryIdentity],
    ) -> Result<PrivateFile, Failure> {
        let read = || {
            directory
                .persistent_read(LOCK_FILE, 0, exclusions)
                .map_err(|error| unavailable(&error))
        };
        let file = match read()? {
            Some(file) => file,
            None => match directory.persistent_create(LOCK_FILE, b"") {
                Ok(file) => file,
                // Another writer created the lock file concurrently.
                Err(error) if super::files::already_exists(&error) => read()?.ok_or_else(|| {
                    Failure::new(
                        Code::CheckpointLogUnavailable,
                        "the writer lock of the checkpoint log disappeared",
                    )
                })?,
                Err(error) => {
                    return Err(super::files::io_failure(
                        "the writer lock of the checkpoint log could not be created",
                        &error,
                    ));
                }
            },
        };
        match file.persistent_try_lock() {
            Ok(true) => Ok(file),
            Ok(false) => Err(Failure::new(
                Code::CheckpointLogBusy,
                "another writer holds the checkpoint log",
            )),
            Err(error) => Err(unavailable(&error)),
        }
    }

    /// Open an existing log for reading. Nothing is created and no lock is
    /// taken: a record is immutable once it is written. A reading that meets
    /// a record while a writer is still writing it either reports that file
    /// as incomplete or, when the file changes between the two checks of its
    /// bounded read, fails closed with `checkpoint_log_unavailable`: an
    /// operational, transient error and never a verdict. The readings that a
    /// writer makes before it takes the lock have the same window.
    pub(crate) fn open_reader(path: &Path) -> Result<Self, Failure> {
        let path = super::files::absolute(path)?;
        let directory = Directory::open_existing(&path).map_err(|error| unavailable(&error))?;
        Ok(Self {
            directory,
            path,
            exclusions: Vec::new(),
            _writer_lock: None,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Strict enumeration: any entry that is not a private file or directory
    /// with an admissible name makes the whole log unavailable.
    fn listing(&self) -> Result<Listing, Failure> {
        self.directory
            .persistent_entries(MAX_LOG_RECORDS + ENTRY_MARGIN)
            .map(Listing::of)
            .map_err(|error| unavailable(&error))
    }

    fn classify(&self, listing: Listing) -> Result<RecordSet, Failure> {
        let mut set = RecordSet {
            unexpected: listing.unexpected,
            ..RecordSet::default()
        };
        for file in &listing.records {
            match self.read_record(file)? {
                Ok(record) => set.insert(record),
                Err(incomplete) => set.incomplete.push(incomplete),
            }
        }
        Ok(set)
    }

    /// Every admissible record of the directory, without failing on an entry
    /// that the strict enumeration refuses. Never used for a trust decision.
    pub(crate) fn load_tolerant(&self) -> Result<TolerantLoad, Failure> {
        let inventory = self
            .directory
            .persistent_inventory(MAX_LOG_RECORDS + ENTRY_MARGIN)
            .map_err(|error| unavailable(&error))?;
        Ok(TolerantLoad {
            rejected_entries: inventory.rejected,
            enumeration_complete: !inventory.incomplete,
            set: self.classify(Listing::of(inventory.entries))?,
        })
    }

    /// Read one record file. `Err` is a storage failure; `Ok(Err(_))` is a
    /// file that is not the complete record its name announces.
    fn read_record(
        &self,
        file: &RecordFile,
    ) -> Result<Result<SignedRecord, IncompleteRecord>, Failure> {
        let incomplete = |reason: String, holds_another_record: bool| IncompleteRecord {
            name: file.name.clone(),
            reason,
            holds_another_record,
        };
        if file.bytes > MAX_RECORD_BYTES {
            return Ok(Err(incomplete(
                "the file exceeds the record size bound".to_owned(),
                false,
            )));
        }
        let Some(content) = self
            .directory
            .persistent_read(&file.name, MAX_RECORD_BYTES, &self.exclusions)
            .map_err(|error| unavailable(&error))?
        else {
            return Err(Failure::new(
                Code::CheckpointLogUnavailable,
                "a checkpoint record disappeared while the log was read",
            )
            .with("record", file.name.clone()));
        };
        Ok(match SignedRecord::parse(content.bytes()) {
            Ok(record) if record.file_name() == file.name => Ok(record),
            Ok(_) => Err(incomplete(
                "the content is a complete record, but not the one named by the file".to_owned(),
                true,
            )),
            Err(failure) => Err(incomplete(
                format!("{}: {}", failure.error.code, failure.error.message),
                false,
            )),
        })
    }

    /// Every record of the directory, classified.
    pub(crate) fn load(&self) -> Result<RecordSet, Failure> {
        let listing = self.listing()?;
        self.classify(listing)
    }

    /// The complete record with the highest log index. Two complete records at
    /// that index are a fork.
    pub(crate) fn head(&self) -> Result<LogHead, Failure> {
        let listing = self.listing()?;
        let mut head = LogHead {
            record: None,
            incomplete: Vec::new(),
            record_files: listing.records.len(),
        };
        let mut indexes: Vec<u64> = listing.records.iter().map(|file| file.log_index).collect();
        indexes.sort_unstable();
        indexes.dedup();
        for index in indexes.into_iter().rev() {
            let mut complete = Vec::new();
            for file in listing
                .records
                .iter()
                .filter(|file| file.log_index == index)
            {
                match self.read_record(file)? {
                    Ok(record) => complete.push(record),
                    Err(incomplete) => head.incomplete.push(incomplete),
                }
            }
            if complete.len() > 1 {
                return Err(forked(index, &complete));
            }
            if let Some(record) = complete.pop() {
                head.record = Some(record);
                break;
            }
        }
        head.incomplete
            .sort_by(|left, right| left.name.cmp(&right.name));
        Ok(head)
    }

    /// Create one record exclusively, make it durable and read it back.
    pub(crate) fn append(&self, record: &SignedRecord) -> Result<PathBuf, Failure> {
        let name = record.file_name();
        let bytes = record.to_bytes()?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(Failure::new(
                Code::RecordMalformed,
                "the new record exceeds its size bound",
            ));
        }
        let file = self
            .directory
            .persistent_create(&name, &bytes)
            .map_err(|error| {
                if super::files::already_exists(&error) {
                    Failure::new(
                        Code::CheckpointRecordExists,
                        "a file with the name of the new record already exists",
                    )
                    .with("record", name.clone())
                } else {
                    super::files::io_failure("the checkpoint record could not be created", &error)
                        .with("record", name.clone())
                }
            })?;
        file.persistent_sync()
            .and_then(|()| self.directory.sync())
            .map_err(|error| {
                super::files::io_failure("the checkpoint record could not be made durable", &error)
                    .with("record", name.clone())
            })?;
        drop(file);
        let written = self.read_record(&RecordFile {
            name: name.clone(),
            log_index: record.log_index(),
            bytes: bytes.len() as u64,
        })?;
        if written.as_ref().ok() != Some(record) {
            return Err(Failure::new(
                Code::IoFailure,
                "the checkpoint record read back differs from the signed record",
            )
            .with("record", name));
        }
        Ok(self.path.join(&name))
    }
}

pub(crate) fn forked(log_index: u64, records: &[SignedRecord]) -> Failure {
    Failure::new(
        Code::LogForked,
        "two complete records claim the same log position",
    )
    .with("log_index", log_index)
    .with(
        "statement_sha256",
        records
            .iter()
            .map(|record| record.statement_sha256().to_owned())
            .collect::<Vec<_>>(),
    )
}
