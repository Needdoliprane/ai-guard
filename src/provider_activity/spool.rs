//! Broker-local durable spool of masked provider activity events.
//!
//! One append-only, hash-chained file per session in a private control
//! directory. An append returns only after `sync_data`; that is the durability
//! point acknowledged to a gated hook (PreToolUse, UserPromptSubmit). Delivery
//! progress is recorded by markers in the same file; losing a marker only
//! causes an idempotent re-delivery. The spool holds masked events only and is
//! never read by the worker.
use crate::{control_fs::Directory, model::Event};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
};
use uuid::Uuid;

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Hard bound of one spooled line, above the largest bounded event.
const MAX_SPOOL_LINE: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Line {
    Event {
        v: u32,
        seq: u64,
        prev: String,
        hash: String,
        event: Event,
    },
    Delivered {
        v: u32,
        through: u64,
    },
}

/// One masked event with its spool position.
#[derive(Clone, Debug)]
pub struct SpooledEvent {
    pub seq: u64,
    pub bytes: usize,
    pub event: Event,
}

/// What a reopen found. A torn tail is an unacknowledged partial append.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpoolRecovery {
    pub events: u64,
    pub delivered_through: u64,
    pub torn_tail_bytes: u64,
}

pub struct ActivitySpool {
    session: Uuid,
    file: File,
    events: Vec<SpooledEvent>,
    head: String,
    delivered_through: u64,
    bytes: u64,
    failed: bool,
}

impl std::fmt::Debug for ActivitySpool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivitySpool")
            .field("events", &self.events.len())
            .field("delivered_through", &self.delivered_through)
            .field("bytes", &self.bytes)
            .field("failed", &self.failed)
            .finish()
    }
}

pub fn spool_name(session: Uuid) -> OsString {
    OsString::from(format!("provider-activity-{session}.spool"))
}

/// Deterministic event id for a spool position, so a re-delivery after an
/// interruption is an idempotent replay of the same event at the gateway.
pub fn event_id(session: Uuid, seq: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ai-guard.provider-activity.event.v1\0");
    hash.update(session.as_bytes());
    hash.update(seq.to_be_bytes());
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 UUIDv8 layout: custom, name-derived bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn chain(prev: &str, seq: u64, event: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ai-guard.provider-activity.spool.v1\0");
    hash.update(prev.as_bytes());
    hash.update(seq.to_be_bytes());
    hash.update(event.as_bytes());
    hex::encode(hash.finalize())
}

fn lock_writer(directory: &Directory, name: &OsStr, file: &File) -> Result<()> {
    if directory.try_lock_private_entry(name, file)? {
        return Ok(());
    }
    Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        .context("provider activity spool ownership is unconfirmed")
}

impl ActivitySpool {
    /// Create a new spool; an existing one is never truncated or replaced.
    pub fn create(directory: &Directory, session: Uuid) -> Result<Self> {
        let name = spool_name(session);
        let file = directory
            .create_private_entry(&name)
            .context("provider activity spool cannot be created privately")?;
        lock_writer(directory, &name, &file)?;
        directory.sync()?;
        Ok(Self {
            session,
            file,
            events: Vec::new(),
            head: GENESIS.into(),
            delivered_through: 0,
            bytes: 0,
            failed: false,
        })
    }

    /// Reopen this session's spool when it exists, otherwise create it.
    pub fn open_or_create(
        directory: &Directory,
        session: Uuid,
    ) -> Result<(Self, Option<SpoolRecovery>)> {
        if directory.private_entry(&spool_name(session))?.is_some() {
            let (spool, recovery) = Self::reopen(directory, session)?;
            Ok((spool, Some(recovery)))
        } else {
            Ok((Self::create(directory, session)?, None))
        }
    }

    /// Reopen after an interruption, verifying every chained record. A final
    /// line without its delimiter is a torn, unacknowledged append and is cut;
    /// any other inconsistency refuses the spool without repairing it.
    pub fn reopen(directory: &Directory, session: Uuid) -> Result<(Self, SpoolRecovery)> {
        let name = spool_name(session);
        let mut file = directory
            .open_private_entry_rw(&name)?
            .context("provider activity spool is absent")?;
        lock_writer(directory, &name, &file)?;
        // A racing creator can leave a new entry before syncing its parent.
        directory.sync()?;
        let mut reader = BufReader::new(file.try_clone()?);
        let mut events = Vec::new();
        let mut head = GENESIS.to_owned();
        let mut delivered_through = 0;
        let mut offset = 0_u64;
        let mut torn_tail_bytes = 0;
        loop {
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(MAX_SPOOL_LINE as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                ensure!(
                    line.len() <= MAX_SPOOL_LINE,
                    "provider activity spool line exceeds its bound"
                );
                torn_tail_bytes = line.len() as u64;
                break;
            }
            let parsed: Line = serde_json::from_slice(&line[..line.len() - 1])
                .context("provider activity spool record is corrupt")?;
            match parsed {
                Line::Event {
                    v,
                    seq,
                    prev,
                    hash,
                    event,
                } => {
                    ensure!(
                        v == 1
                            && seq == events.len() as u64 + 1
                            && prev == head
                            && event.id == event_id(session, seq),
                        "provider activity spool chain is broken"
                    );
                    let body = serde_json::to_string(&event)?;
                    ensure!(
                        chain(&prev, seq, &body) == hash,
                        "provider activity spool hash mismatch"
                    );
                    head = hash;
                    events.push(SpooledEvent {
                        seq,
                        bytes: line.len(),
                        event,
                    });
                }
                Line::Delivered { v, through } => {
                    ensure!(
                        v == 1 && through >= delivered_through && through <= events.len() as u64,
                        "provider activity spool delivery marker is inconsistent"
                    );
                    delivered_through = through;
                }
            }
            offset += line.len() as u64;
        }
        if torn_tail_bytes > 0 {
            lock_writer(directory, &name, &file)?;
            file.set_len(offset)
                .context("torn provider activity tail cannot be cut")?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;
        let recovery = SpoolRecovery {
            events: events.len() as u64,
            delivered_through,
            torn_tail_bytes,
        };
        Ok((
            Self {
                session,
                file,
                bytes: events.iter().map(|e| e.bytes as u64).sum(),
                events,
                head,
                delivered_through,
                failed: false,
            },
            recovery,
        ))
    }

    /// Append events durably and return their sequence numbers. After any
    /// write or sync error the spool refuses every later append.
    pub fn append(&mut self, events: Vec<Event>) -> Result<Vec<u64>> {
        self.append_bounded(events, u64::MAX, u64::MAX)?
            .context("provider activity spool capacity is exhausted")
    }

    /// Check the exact encoded lines before writing. A batch that would exceed
    /// either limit leaves the file, chain head and sequence numbers intact.
    pub(super) fn append_bounded(
        &mut self,
        events: Vec<Event>,
        max_events: u64,
        max_bytes: u64,
    ) -> Result<Option<Vec<u64>>> {
        ensure!(!self.failed, "provider activity spool is unavailable");
        if events.len() as u64 > max_events.saturating_sub(self.len()) {
            return Ok(None);
        }
        let available_bytes = max_bytes.saturating_sub(self.bytes);
        let mut lines = Vec::new();
        let mut pending = Vec::with_capacity(events.len());
        let mut head = self.head.clone();
        let mut seq = self.events.len() as u64;
        for mut event in events {
            seq += 1;
            event.id = event_id(self.session, seq);
            let body = serde_json::to_string(&event)?;
            let hash = chain(&head, seq, &body);
            let line = serde_json::to_vec(&Line::Event {
                v: 1,
                seq,
                prev: head.clone(),
                hash: hash.clone(),
                event: event.clone(),
            })?;
            ensure!(
                line.len() < MAX_SPOOL_LINE,
                "provider activity event exceeds the spool line bound"
            );
            if (lines.len() + line.len() + 1) as u64 > available_bytes {
                return Ok(None);
            }
            head = hash;
            pending.push(SpooledEvent {
                seq,
                bytes: line.len() + 1,
                event,
            });
            lines.extend_from_slice(&line);
            lines.push(b'\n');
        }
        if lines.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if let Err(error) = self
            .file
            .write_all(&lines)
            .and_then(|()| self.file.sync_data())
        {
            self.failed = true;
            bail!("provider activity spool write failed: {}", error.kind());
        }
        self.head = head;
        self.bytes += lines.len() as u64;
        let seqs = pending.iter().map(|e| e.seq).collect();
        self.events.extend(pending);
        Ok(Some(seqs))
    }

    /// Record delivery progress. Durability is not required: a lost marker
    /// only replays already delivered, idempotent events.
    pub fn mark_delivered(&mut self, through: u64) -> Result<()> {
        ensure!(
            through <= self.events.len() as u64 && through >= self.delivered_through,
            "invalid provider activity delivery position"
        );
        if through == self.delivered_through || self.failed {
            return Ok(());
        }
        let mut line = serde_json::to_vec(&Line::Delivered { v: 1, through })?;
        line.push(b'\n');
        if let Err(error) = self.file.write_all(&line) {
            self.failed = true;
            bail!("provider activity spool write failed: {}", error.kind());
        }
        self.delivered_through = through;
        Ok(())
    }

    /// The next undelivered events, bounded by count and serialized bytes.
    /// The first pending event is always returned, so one large event can
    /// never stall delivery behind the byte bound.
    pub fn pending(&self, max_events: usize, max_bytes: usize) -> Vec<SpooledEvent> {
        let mut used = 0;
        self.events
            .iter()
            .skip(self.delivered_through as usize)
            .take(max_events)
            .enumerate()
            .take_while(|(index, event)| {
                used += event.bytes;
                *index == 0 || used <= max_bytes
            })
            .map(|(_, event)| event.clone())
            .collect()
    }

    /// Every spooled event in order, delivered or not (recovery inspection).
    pub fn events(&self) -> &[SpooledEvent] {
        &self.events
    }
    pub fn len(&self) -> u64 {
        self.events.len() as u64
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn delivered_through(&self) -> u64 {
        self.delivered_through
    }
    pub fn backlog(&self) -> (u64, u64) {
        let delivered = self.delivered_through as usize;
        (
            self.len() - self.delivered_through,
            self.events[delivered..]
                .iter()
                .map(|e| e.bytes as u64)
                .sum(),
        )
    }
    pub fn failed(&self) -> bool {
        self.failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::ErrorKind, os::unix::fs::PermissionsExt};

    #[test]
    fn budget_counts_encoded_lines_and_refuses_the_whole_batch_before_writing() {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = crate::control_fs::absolute_path(temporary.path()).unwrap();
        let directory = Directory::open_existing(&root).unwrap();
        let event = Event::new("activity.channel");
        let pair = vec![event.clone(), event];
        let mut reference = ActivitySpool::create(&directory, Uuid::new_v4()).unwrap();
        reference.append(pair.clone()).unwrap();
        let encoded_bytes = reference.bytes();

        let session = Uuid::new_v4();
        let path = root.join(spool_name(session));
        let mut spool = ActivitySpool::create(&directory, session).unwrap();
        assert_eq!(
            spool
                .append_bounded(pair.clone(), 2, encoded_bytes - 1)
                .unwrap(),
            None
        );
        assert!(spool.is_empty());
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);

        assert_eq!(
            spool.append_bounded(pair, 2, encoded_bytes).unwrap(),
            Some(vec![1, 2])
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), encoded_bytes);
    }

    #[test]
    fn retained_writer_blocks_reopen_before_torn_tail_recovery() {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = crate::control_fs::absolute_path(temporary.path()).unwrap();
        let directory = Directory::open_existing(&root).unwrap();
        let session = Uuid::new_v4();
        let path = temporary.path().join(spool_name(session));
        let mut spool = ActivitySpool::create(&directory, session).unwrap();
        spool.append(vec![Event::new("activity.channel")]).unwrap();
        let prefix = fs::read(&path).unwrap();
        let mut retained_writer = spool.file.try_clone().unwrap();
        drop(spool);

        let partial = b"{\"type\":\"event\"";
        retained_writer.write_all(partial).unwrap();
        retained_writer.sync_data().unwrap();
        let before = fs::read(&path).unwrap();

        let failure = ActivitySpool::open_or_create(&directory, session)
            .expect_err("reopening must wait for the retained writer");
        assert_eq!(
            failure
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(ErrorKind::WouldBlock)
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        drop(retained_writer);

        let (mut reopened, recovery) = ActivitySpool::reopen(&directory, session).unwrap();
        assert_eq!(recovery.events, 1);
        assert_eq!(recovery.torn_tail_bytes, partial.len() as u64);
        assert_eq!(fs::read(&path).unwrap(), prefix);
        reopened
            .append(vec![Event::new("activity.channel")])
            .unwrap();
        drop(reopened);

        let (reopened, recovery) = ActivitySpool::reopen(&directory, session).unwrap();
        assert_eq!(recovery.events, 2);
        assert_eq!(reopened.events()[0].event.id, event_id(session, 1));
        assert_eq!(reopened.events()[1].event.id, event_id(session, 2));
    }
}
