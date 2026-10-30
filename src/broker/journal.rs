//! Durable private job data. This module never authorizes or starts a workload.
use super::intent::ExecutionIntentV1;
use crate::{
    control_fs::{Directory, PrivateFile},
    runner::CheckpointPayloadV1,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, Weak},
};
use uuid::Uuid;
mod types;
pub use types::*;
use types::{AcceptedData, BindingData};

macro_rules! stage {
    ($stage:ident) => {{
        #[cfg(test)]
        {
            tests::checkpoint(FaultStage::$stage)?;
        }
    }};
}

const SMALL: u64 = 8 * 1024;
const CHECKPOINT: u64 = 32 * 1024 * 1024;

pub(crate) fn canonical(value: &impl Serialize) -> JournalResult<Vec<u8>> {
    fn order(value: Value) -> Value {
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, order(v)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.into_iter().map(order).collect()),
            other => other,
        }
    }
    serde_json::to_vec(&order(
        serde_json::to_value(value).map_err(|_| JournalError::InvalidInput)?,
    ))
    .map_err(|_| JournalError::InvalidInput)
}
fn digest(domain: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    hex::encode(hash.finalize())
}
fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn valid_hash(hash: &str) -> JournalResult<()> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(JournalError::InvalidInput)
    }
}
fn fscheck<T>(result: anyhow::Result<T>) -> JournalResult<T> {
    result.map_err(|error| {
        if error.is::<crate::control_fs::persistent::PersistentLimit>() {
            JournalError::LimitExceeded
        } else {
            JournalError::UnsafeFilesystem
        }
    })
}
fn io<T>(result: anyhow::Result<T>) -> JournalResult<T> {
    result.map_err(|_| JournalError::Io)
}
fn revision_name(revision: u64) -> String {
    format!("rev-{revision:020}.json")
}

#[derive(Clone)]
pub struct Journal(Arc<Inner>);
struct Inner {
    scope: JournalScopeV1,
    scope_hash: String,
    directory: Directory,
    jobs: Directory,
    lock: PrivateFile,
    exclusions: FsExclusions,
    limits: JournalLimitsV1,
    control: ControlSecrets,
    writer: Mutex<Writer>,
}
#[derive(Default)]
struct Writer {
    tickets: HashMap<Uuid, Weak<()>>,
    floors: HashMap<Uuid, RevisionRefV1>,
}
pub struct ReservationTicket {
    owner: Arc<Inner>,
    lease: Arc<()>,
    operation: Uuid,
    submit_hash: String,
}

macro_rules! record {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Serialize, Deserialize)] #[serde(deny_unknown_fields)]
        struct $name { $($field: $ty),* }
    };
}
record!(Reservation {
    version: u32,
    scope_hash: String,
    operation_id: Uuid,
    submit_hash: String
});
record!(Bindings {
    version: u32,
    operation_id: Uuid,
    intent_hash: String,
    binding: BindingManifestV1
});
record!(Checkpoint { version:u32, operation_id:Uuid, intent_hash:String, revision:u64, previous:Option<RevisionRefV1>, payload:CheckpointPayloadV1 });
record!(Current {
    version: u32,
    operation_id: Uuid,
    intent_hash: String,
    head: RevisionRefV1
});
record!(Accepted {
    version: u32,
    scope_hash: String,
    operation_id: Uuid,
    submit_hash: String,
    intent_hash: String,
    bindings_hash: String,
    initial_checkpoint: RevisionRefV1
});

#[derive(Default)]
struct Usage {
    bytes: u64,
    entries: u32,
    jobs: u32,
    job_bytes: u64,
    job_entries: u32,
}
impl Usage {
    fn add(
        &mut self,
        bytes: u64,
        entries: u32,
        job: bool,
        limits: &JournalLimitsV1,
    ) -> JournalResult<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(JournalError::LimitExceeded)?;
        self.entries = self
            .entries
            .checked_add(entries)
            .ok_or(JournalError::LimitExceeded)?;
        if job {
            self.job_bytes = self
                .job_bytes
                .checked_add(bytes)
                .ok_or(JournalError::LimitExceeded)?;
            self.job_entries = self
                .job_entries
                .checked_add(entries)
                .ok_or(JournalError::LimitExceeded)?;
        }
        if self.bytes > limits.scope_bytes
            || self.entries > limits.scope_entries
            || self.jobs > limits.jobs
            || self.job_bytes > limits.job_bytes
            || self.job_entries > limits.job_entries
        {
            return Err(JournalError::LimitExceeded);
        }
        Ok(())
    }
}

impl Journal {
    pub fn open(
        root: &Directory,
        scope: JournalScopeV1,
        exclusions: &FsExclusions,
        limits: JournalLimitsV1,
        control_secrets: ControlSecrets,
    ) -> JournalResult<Self> {
        scope.validate()?;
        let scope_bytes = canonical(&scope)?;
        control_secrets.record(&scope_bytes)?;
        if scope_bytes.len() as u64 > SMALL
            || limits.scope_entries < 3
            || scope_bytes.len() as u64 > limits.scope_bytes
        {
            return Err(JournalError::LimitExceeded);
        }
        let scope_hash = digest("ai-guard:broker-journal-scope:v1", &scope_bytes);
        fscheck(root.durable_bootstrap())?;
        let namespace =
            fscheck(root.persistent_child("broker-journal-v1", true))?.ok_or(JournalError::Io)?;
        let directory =
            fscheck(namespace.persistent_child(&scope_hash, true))?.ok_or(JournalError::Io)?;
        let lock = match fscheck(directory.persistent_read(".writer.lock", 0, &exclusions.0))? {
            Some(file) => file,
            None => {
                stage!(WriterLockAbsent);
                let file = match directory.persistent_create(".writer.lock", &[]) {
                    Ok(file) => file,
                    Err(error)
                        if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                            error.kind() == std::io::ErrorKind::AlreadyExists
                        }) =>
                    {
                        // Another first opener won O_EXCL. Reinspect once with
                        // the same no-link/ACL/exclusion guards, then try flock.
                        fscheck(directory.persistent_read(".writer.lock", 0, &exclusions.0))?
                            .ok_or(JournalError::Io)?
                    }
                    Err(_) => return Err(JournalError::Io),
                };
                io(file.persistent_sync())?;
                io(directory.sync())?;
                file
            }
        };
        if !fscheck(lock.persistent_try_lock())? {
            return Err(JournalError::Busy);
        }
        match fscheck(directory.persistent_read("scope.json", SMALL, &exclusions.0))? {
            Some(file) => {
                let existing: JournalScopeV1 = parse(file.bytes())?;
                if existing.0 != scope.0 || file.bytes() != scope_bytes {
                    return Err(JournalError::Conflict);
                }
                io(file.persistent_sync())?;
            }
            None => {
                let entries = fscheck(directory.persistent_entries(limits.scope_entries))?;
                if entries.iter().any(|entry| entry.name != ".writer.lock") {
                    return Err(JournalError::Corrupt);
                }
                let file = io(directory.persistent_create("scope.json", &scope_bytes))?;
                io(file.persistent_sync())?;
            }
        }
        io(directory.sync())?;
        let jobs = fscheck(directory.persistent_child("jobs", true))?.ok_or(JournalError::Io)?;
        let journal = Self(Arc::new(Inner {
            scope,
            scope_hash,
            directory,
            jobs,
            lock,
            exclusions: exclusions.clone(),
            limits,
            control: control_secrets,
            writer: Mutex::new(Writer::default()),
        }));
        journal.usage(None)?;
        stage!(NamespaceDurable);
        Ok(journal)
    }

    pub fn reserve(&self, operation_id: Uuid, submit_bytes: &[u8]) -> JournalResult<ReserveResult> {
        if submit_bytes.len() > 256 * 1024 {
            return Err(JournalError::TooLarge);
        }
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        self.0.control.bytes(submit_bytes)?;
        let submit_hash = digest("ai-guard:broker-submit:v1", submit_bytes);
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let name = operation_id.to_string();
        if let Some(job) = fscheck(self.0.jobs.persistent_child(&name, false))? {
            if self.read_file(&job, "reservation.json", SMALL)?.is_none() {
                self.usage(Some(&name))?;
                return Ok(ReserveResult::Interrupted);
            }
            let reservation: Reservation = self.read(&job, "reservation.json", SMALL)?;
            self.check_reservation(&reservation, operation_id)?;
            if reservation.submit_hash != submit_hash {
                return Err(JournalError::Conflict);
            }
            if self.read_file(&job, "accepted.json", SMALL)?.is_some() {
                let accepted = self
                    .load_locked(operation_id, &mut writer)?
                    .ok_or(JournalError::Corrupt)?;
                stage!(BeforeAck);
                return Ok(ReserveResult::Accepted(accepted));
            }
            self.usage(Some(&name))?;
            if writer
                .tickets
                .get(&operation_id)
                .and_then(Weak::upgrade)
                .is_some()
            {
                return Ok(ReserveResult::InProgress);
            }
            return Ok(ReserveResult::Interrupted);
        }
        if writer.floors.contains_key(&operation_id) {
            return Err(JournalError::Conflict);
        }
        let reservation = Reservation {
            version: 1,
            scope_hash: self.0.scope_hash.clone(),
            operation_id,
            submit_hash: submit_hash.clone(),
        };
        let bytes = self.checked_record(&reservation, SMALL)?;
        let mut usage = self.usage(None)?;
        usage.jobs = usage
            .jobs
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        usage.add(bytes.len() as u64, 2, true, &self.0.limits)?;
        let job = fscheck(self.0.jobs.persistent_child(&name, true))?.ok_or(JournalError::Io)?;
        let file = io(job.persistent_create("reservation.json", &bytes))?;
        stage!(ReservationWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        stage!(ReservationDurable);
        let lease = Arc::new(());
        writer.tickets.insert(operation_id, Arc::downgrade(&lease));
        Ok(ReserveResult::New(ReservationTicket {
            owner: self.0.clone(),
            lease,
            operation: operation_id,
            submit_hash,
        }))
    }

    pub fn accept(
        &self,
        ticket: ReservationTicket,
        intent: &ExecutionIntentV1,
        bindings: BindingInputs<'_>,
        checkpoint0: &CheckpointPayloadV1,
    ) -> JournalResult<AcceptedJobV1> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        if !Arc::ptr_eq(&ticket.owner, &self.0)
            || ticket.operation != intent.operation_id()
            || writer
                .tickets
                .get(&ticket.operation)
                .and_then(Weak::upgrade)
                .is_none_or(|lease| !Arc::ptr_eq(&lease, &ticket.lease))
        {
            return Err(JournalError::Conflict);
        }
        intent.validate().map_err(|_| JournalError::InvalidInput)?;
        let initial = CheckpointPayloadV1::initial(intent)?;
        if initial.canonical_bytes()? != checkpoint0.canonical_bytes()? {
            return Err(JournalError::Conflict);
        }
        let intent_bytes = intent
            .canonical_bytes()
            .map_err(|_| JournalError::InvalidInput)?;
        self.0.control.record(&intent_bytes)?;
        let intent_hash = intent
            .intent_hash()
            .map_err(|_| JournalError::InvalidInput)?;
        let (binding, binding_bytes) = self.prepare_binding(intent, bindings)?;
        self.context(checkpoint0, intent, &binding)?;
        let manifest = Bindings {
            version: 1,
            operation_id: ticket.operation,
            intent_hash: intent_hash.clone(),
            binding,
        };
        let manifest_bytes = self.checked_record(&manifest, SMALL)?;
        let checkpoint = Checkpoint {
            version: 1,
            operation_id: ticket.operation,
            intent_hash: intent_hash.clone(),
            revision: 0,
            previous: None,
            payload: checkpoint0.clone(),
        };
        let checkpoint_bytes = self.checked_record(&checkpoint, CHECKPOINT)?;
        let head = RevisionRefV1 {
            revision: 0,
            hash: digest("ai-guard:broker-checkpoint:v1", &checkpoint_bytes),
        };
        let current = Current {
            version: 1,
            operation_id: ticket.operation,
            intent_hash: intent_hash.clone(),
            head: head.clone(),
        };
        let current_bytes = self.checked_record(&current, SMALL)?;
        let accepted = Accepted {
            version: 1,
            scope_hash: self.0.scope_hash.clone(),
            operation_id: ticket.operation,
            submit_hash: ticket.submit_hash.clone(),
            intent_hash,
            bindings_hash: digest("ai-guard:broker-bindings:v1", &manifest_bytes),
            initial_checkpoint: head,
        };
        let accepted_bytes = self.checked_record(&accepted, SMALL)?;
        let name = ticket.operation.to_string();
        let job = self.job(ticket.operation)?;
        let reservation: Reservation = self.read(&job, "reservation.json", SMALL)?;
        self.check_reservation(&reservation, ticket.operation)?;
        if reservation.submit_hash != ticket.submit_hash {
            return Err(JournalError::Conflict);
        }
        let entries = fscheck(job.persistent_entries(self.0.limits.job_entries))?;
        if entries.len() != 1 || entries[0].name != "reservation.json" {
            return Err(JournalError::Interrupted);
        }
        let added_bytes = intent_bytes.len() as u64
            + manifest_bytes.len() as u64
            + checkpoint_bytes.len() as u64
            + current_bytes.len() as u64
            + accepted_bytes.len() as u64
            + binding_bytes.as_ref().map_or(0, |b| b.len() as u64);
        self.budget(
            &name,
            added_bytes,
            5 + if binding_bytes.is_some() { 2 } else { 0 },
        )?;
        if let Some(bytes) = binding_bytes {
            let directory =
                fscheck(job.persistent_child("bindings", true))?.ok_or(JournalError::Io)?;
            let filename = manifest.binding.file_name().ok_or(JournalError::Corrupt)?;
            let file = io(directory.persistent_create(filename, &bytes))?;
            stage!(BindingWritten);
            io(file.persistent_sync())?;
            io(directory.sync())?;
            io(job.sync())?;
            stage!(BindingDurable);
        }
        self.write(&job, "intent.json", &intent_bytes)?;
        stage!(IntentDurable);
        self.write(&job, "bindings.json", &manifest_bytes)?;
        stage!(ManifestDurable);
        let revision = io(job.persistent_create(&revision_name(0), &checkpoint_bytes))?;
        stage!(RevisionWritten);
        io(revision.persistent_sync())?;
        io(job.sync())?;
        stage!(RevisionDurable);
        let file = io(job.persistent_create("current.json", &current_bytes))?;
        stage!(CurrentWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        stage!(CurrentDurable);
        let file = io(job.persistent_create("accepted.json", &accepted_bytes))?;
        stage!(AcceptedWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        stage!(AcceptedDurable);
        let result = self
            .load_locked(ticket.operation, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        stage!(BeforeAck);
        Ok(result)
    }

    pub fn load(&self, operation_id: Uuid) -> JournalResult<Option<AcceptedJobV1>> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        self.load_locked(operation_id, &mut writer)
    }

    pub fn advance(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: &CheckpointPayloadV1,
    ) -> JournalResult<CheckpointReceiptV1> {
        valid_hash(&expected.hash)?;
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let loaded = self
            .load_locked(operation_id, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        self.context(next, &loaded.0.intent, &loaded.0.binding)?;
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        let checkpoint = Checkpoint {
            version: 1,
            operation_id,
            intent_hash: loaded.0.intent_hash.clone(),
            revision: next_revision,
            previous: Some(expected.clone()),
            payload: next.clone(),
        };
        let bytes = self.checked_record(&checkpoint, CHECKPOINT)?;
        let head = RevisionRefV1 {
            revision: next_revision,
            hash: digest("ai-guard:broker-checkpoint:v1", &bytes),
        };
        if expected.revision < loaded.0.head.revision {
            let index = usize::try_from(expected.revision).map_err(|_| JournalError::Conflict)?;
            if loaded.0.chain.get(index) == Some(&expected)
                && loaded.0.chain.get(index + 1) == Some(&head)
            {
                stage!(BeforeAck);
                return Ok(CheckpointReceiptV1 { operation_id, head });
            }
            return Err(JournalError::Conflict);
        }
        if expected != loaded.0.head {
            return Err(JournalError::Conflict);
        }
        if next_revision >= u64::from(self.0.limits.revisions) {
            return Err(JournalError::LimitExceeded);
        }
        let job = self.job(operation_id)?;
        if self
            .read_file(&job, &revision_name(next_revision), CHECKPOINT)?
            .is_some()
            || self.read_file(&job, "current-next.json", SMALL)?.is_some()
        {
            return Err(JournalError::Interrupted);
        }
        let current = Current {
            version: 1,
            operation_id,
            intent_hash: loaded.0.intent_hash.clone(),
            head: head.clone(),
        };
        let pointer = self.checked_record(&current, SMALL)?;
        self.budget(
            &operation_id.to_string(),
            bytes.len() as u64 + pointer.len() as u64,
            2,
        )?;
        let file = io(job.persistent_create(&revision_name(next_revision), &bytes))?;
        stage!(RevisionWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        stage!(RevisionDurable);
        let temporary = io(job.persistent_create("current-next.json", &pointer))?;
        stage!(CurrentWritten);
        io(temporary.persistent_sync())?;
        let old = self
            .read_file(&job, "current.json", SMALL)?
            .ok_or(JournalError::Corrupt)?;
        let old_current: Current = parse(old.bytes())?;
        if old_current.head != expected {
            return Err(JournalError::Conflict);
        }
        io(job.persistent_publish(&temporary, &old, "current.json"))?;
        stage!(CurrentPublished);
        io(job.sync())?;
        stage!(CurrentDurable);
        let result = self
            .load_locked(operation_id, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        if result.0.head != head {
            return Err(JournalError::Corrupt);
        }
        stage!(BeforeAck);
        Ok(CheckpointReceiptV1 { operation_id, head })
    }
}

impl Journal {
    fn prepare_binding(
        &self,
        intent: &ExecutionIntentV1,
        inputs: BindingInputs<'_>,
    ) -> JournalResult<(BindingManifestV1, Option<Vec<u8>>)> {
        let options = intent.initial_options().run_options();
        match inputs {
            BindingInputs::None => {
                if options.secret_env_file.is_some() || intent.provider_credential().is_some() {
                    return Err(JournalError::InvalidInput);
                }
                Ok((BindingManifestV1(BindingData::None), None))
            }
            BindingInputs::Manual { source } => {
                if intent.provider_credential().is_some()
                    || options.secret_env_file.as_deref() != Some(source.path())
                {
                    return Err(JournalError::Conflict);
                }
                if source.bytes().len() > 64 * 1024 {
                    return Err(JournalError::TooLarge);
                }
                fscheck(source.persistent_revalidate())?;
                let material = crate::runner::prepare_manual_credentials(source.bytes())
                    .map_err(|_| JournalError::InvalidInput)?;
                self.check_material(&material)?;
                let stored_sha256 = sha(&material.bytes);
                let manifest = BindingManifestV1(BindingData::Manual {
                    binding_id: Uuid::new_v4(),
                    file_name: "manual-env-v1.bin".into(),
                    names: material.names,
                    source_sha256: source.digest().into(),
                    handoff_sha256: stored_sha256.clone(),
                    stored_sha256,
                    stored_bytes: material.bytes.len() as u64,
                });
                Ok((manifest, Some(material.bytes)))
            }
            BindingInputs::Provider { source } => {
                let reference = intent
                    .provider_credential()
                    .ok_or(JournalError::InvalidInput)?;
                if options.secret_env_file.is_some() || source.digest() != reference.digest() {
                    return Err(JournalError::Conflict);
                }
                if source.bytes().len() > 512 * 1024 {
                    return Err(JournalError::TooLarge);
                }
                fscheck(source.persistent_revalidate())?;
                self.0.control.record(source.bytes())?;
                let material = crate::runner::prepare_provider_credentials(
                    source.bytes(),
                    reference.binding_id(),
                )
                .map_err(|_| JournalError::InvalidInput)?;
                self.check_material(&material)?;
                self.check_provider_context(intent, &material.bytes)?;
                let manifest = BindingManifestV1(BindingData::Provider {
                    binding_id: reference.binding_id(),
                    file_name: reference.file_name().into(),
                    names: material.names,
                    source_sha256: source.digest().into(),
                    stored_sha256: source.digest().into(),
                    handoff_sha256: sha(&material.bytes),
                    stored_bytes: source.bytes().len() as u64,
                });
                Ok((manifest, Some(source.bytes().to_vec())))
            }
        }
    }
    fn check_material(
        &self,
        material: &crate::runner::PreparedCredentialMaterial,
    ) -> JournalResult<()> {
        self.0.control.bytes(&material.bytes)?;
        for secret in &material.secrets {
            self.0.control.bytes(secret.as_bytes())?;
        }
        for name in &material.names {
            self.0.control.bytes(name.as_bytes())?;
        }
        Ok(())
    }
    fn check_provider_context(
        &self,
        intent: &ExecutionIntentV1,
        env_bytes: &[u8],
    ) -> JournalResult<()> {
        let options = intent.initial_options().run_options();
        let strings = crate::provider::validate_frozen_credential(&options.argv, env_bytes)
            .map_err(|_| JournalError::InvalidInput)?;
        for value in strings {
            self.0.control.bytes(value.as_bytes())?;
        }
        Ok(())
    }
    fn load_binding(
        &self,
        job: &Directory,
        intent: &ExecutionIntentV1,
        binding: &BindingManifestV1,
    ) -> JournalResult<Option<PrivateFile>> {
        let options = intent.initial_options().run_options();
        let directory = fscheck(job.persistent_child("bindings", false))?;
        if binding.kind() == BindingKindV1::None {
            if options.secret_env_file.is_some() || intent.provider_credential().is_some() {
                return Err(JournalError::Corrupt);
            }
            if let Some(directory) = directory {
                if !fscheck(directory.persistent_entries(self.0.limits.job_entries))?.is_empty() {
                    return Err(JournalError::Corrupt);
                }
                io(directory.sync())?;
            }
            return Ok(None);
        }
        let id = binding.binding_id().ok_or(JournalError::Corrupt)?;
        if id.is_nil() || binding.stored_bytes() == 0 {
            return Err(JournalError::Corrupt);
        }
        for hash in [
            binding.source_sha(),
            binding.stored_sha(),
            binding.handoff_sha(),
        ] {
            valid_hash(hash.ok_or(JournalError::Corrupt)?).map_err(|_| JournalError::Corrupt)?;
        }
        let directory = directory.ok_or(JournalError::Interrupted)?;
        let name = binding.file_name().ok_or(JournalError::Corrupt)?;
        let entries = fscheck(directory.persistent_entries(self.0.limits.job_entries))?;
        if entries.len() != 1 || entries[0].name != name || entries[0].directory {
            return Err(JournalError::Corrupt);
        }
        let max = match binding.kind() {
            BindingKindV1::Manual => 64 * 1024,
            BindingKindV1::Provider => 512 * 1024,
            BindingKindV1::None => return Err(JournalError::Corrupt),
        };
        let file = self
            .read_file(&directory, name, max)?
            .ok_or(JournalError::Interrupted)?;
        if file.digest() != binding.stored_sha().ok_or(JournalError::Corrupt)?
            || file.bytes().len() as u64 != binding.stored_bytes()
        {
            return Err(JournalError::Corrupt);
        }
        let material = match binding.kind() {
            BindingKindV1::Manual => {
                if intent.provider_credential().is_some()
                    || options.secret_env_file.is_none()
                    || name != "manual-env-v1.bin"
                {
                    return Err(JournalError::Corrupt);
                }
                let material = crate::runner::prepare_manual_credentials(file.bytes())
                    .map_err(|_| JournalError::Corrupt)?;
                if material.bytes != file.bytes() || binding.handoff_sha() != binding.stored_sha() {
                    return Err(JournalError::Corrupt);
                }
                material
            }
            BindingKindV1::Provider => {
                let reference = intent.provider_credential().ok_or(JournalError::Corrupt)?;
                if options.secret_env_file.is_some()
                    || id != reference.binding_id()
                    || name != reference.file_name()
                    || binding.stored_sha() != Some(reference.digest())
                    || binding.source_sha() != Some(reference.digest())
                {
                    return Err(JournalError::Corrupt);
                }
                self.0.control.record(file.bytes())?;
                crate::runner::prepare_provider_credentials(file.bytes(), id)
                    .map_err(|_| JournalError::Corrupt)?
            }
            BindingKindV1::None => return Err(JournalError::Corrupt),
        };
        self.check_material(&material)?;
        if binding.kind() == BindingKindV1::Provider {
            self.check_provider_context(intent, &material.bytes)?;
        }
        if binding.names() != material.names
            || binding.handoff_sha() != Some(sha(&material.bytes).as_str())
        {
            return Err(JournalError::Corrupt);
        }
        io(file.persistent_sync())?;
        io(directory.sync())?;
        Ok(Some(file))
    }
    fn load_locked(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<AcceptedJobV1>> {
        let name = operation_id.to_string();
        let Some(job) = fscheck(self.0.jobs.persistent_child(&name, false))? else {
            return if writer.floors.contains_key(&operation_id) {
                Err(JournalError::Conflict)
            } else {
                Ok(None)
            };
        };
        self.usage(Some(&name))?;
        let reservation: Reservation = self.read(&job, "reservation.json", SMALL)?;
        self.check_reservation(&reservation, operation_id)?;
        let accepted: Accepted = self.read(&job, "accepted.json", SMALL)?;
        if accepted.version != 1
            || accepted.operation_id != operation_id
            || accepted.scope_hash != self.0.scope_hash
            || accepted.submit_hash != reservation.submit_hash
        {
            return Err(JournalError::Corrupt);
        }
        let intent_file = self
            .read_file(&job, "intent.json", 256 * 1024)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(intent_file.bytes())?;
        let intent = ExecutionIntentV1::from_bytes(intent_file.bytes())
            .map_err(|_| JournalError::Corrupt)?;
        let intent_hash = intent.intent_hash().map_err(|_| JournalError::Corrupt)?;
        if intent.operation_id() != operation_id
            || accepted.intent_hash != intent_hash
            || intent
                .canonical_bytes()
                .map_err(|_| JournalError::Corrupt)?
                != intent_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        io(intent_file.persistent_sync())?;
        let binding_file = self
            .read_file(&job, "bindings.json", SMALL)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(binding_file.bytes())?;
        let bindings: Bindings = parse(binding_file.bytes())?;
        if bindings.version != 1
            || bindings.operation_id != operation_id
            || bindings.intent_hash != intent_hash
            || accepted.bindings_hash != digest("ai-guard:broker-bindings:v1", binding_file.bytes())
            || canonical(&bindings)? != binding_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        io(binding_file.persistent_sync())?;
        let source = self.load_binding(&job, &intent, &bindings.binding)?;
        let current: Current = self.read(&job, "current.json", SMALL)?;
        if current.version != 1
            || current.operation_id != operation_id
            || current.intent_hash != intent_hash
            || current.head.revision >= u64::from(self.0.limits.revisions)
        {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&current.head.hash).map_err(|_| JournalError::Corrupt)?;
        let mut chain = Vec::new();
        let mut previous = None;
        let mut payload = None;
        for revision in 0..=current.head.revision {
            let file = self
                .read_file(&job, &revision_name(revision), CHECKPOINT)?
                .ok_or(JournalError::Interrupted)?;
            self.0.control.record(file.bytes())?;
            let checkpoint: Checkpoint = parse(file.bytes())?;
            let raw: Value = parse(file.bytes())?;
            CheckpointPayloadV1::from_bytes(&canonical(
                raw.get("payload").ok_or(JournalError::Corrupt)?,
            )?)?;
            if checkpoint.version != 1
                || checkpoint.operation_id != operation_id
                || checkpoint.intent_hash != intent_hash
                || checkpoint.revision != revision
                || checkpoint.previous != previous
                || canonical(&checkpoint)? != file.bytes()
            {
                return Err(JournalError::Corrupt);
            }
            self.context(&checkpoint.payload, &intent, &bindings.binding)?;
            let head = RevisionRefV1 {
                revision,
                hash: digest("ai-guard:broker-checkpoint:v1", file.bytes()),
            };
            if revision == 0
                && (accepted.initial_checkpoint != head
                    || CheckpointPayloadV1::initial(&intent)?.canonical_bytes()?
                        != checkpoint.payload.canonical_bytes()?)
            {
                return Err(JournalError::Corrupt);
            }
            io(file.persistent_sync())?;
            previous = Some(head.clone());
            chain.push(head);
            payload = Some(checkpoint.payload);
        }
        if previous.as_ref() != Some(&current.head) {
            return Err(JournalError::Corrupt);
        }
        if let Some(floor) = writer.floors.get(&operation_id) {
            let index = usize::try_from(floor.revision).map_err(|_| JournalError::Corrupt)?;
            if chain.get(index) != Some(floor) {
                return Err(JournalError::Conflict);
            }
        }
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        io(self.0.directory.sync())?;
        writer.floors.insert(operation_id, current.head.clone());
        Ok(Some(AcceptedJobV1(Box::new(AcceptedData {
            operation_id,
            intent,
            intent_hash,
            submit_hash: accepted.submit_hash,
            binding: bindings.binding,
            source,
            head: current.head,
            checkpoint: payload.ok_or(JournalError::Corrupt)?,
            chain,
        }))))
    }
    fn check_writer(&self) -> JournalResult<()> {
        fscheck(self.0.lock.persistent_revalidate())?;
        fscheck(self.0.directory.revalidate())?;
        let scope = self
            .read_file(&self.0.directory, "scope.json", SMALL)?
            .ok_or(JournalError::Corrupt)?;
        if scope.bytes() != canonical(&self.0.scope)? {
            return Err(JournalError::Conflict);
        }
        self.0.control.record(scope.bytes())
    }
    fn checked_record(&self, value: &impl Serialize, max: u64) -> JournalResult<Vec<u8>> {
        let bytes = canonical(value)?;
        if bytes.len() as u64 > max {
            return Err(JournalError::TooLarge);
        }
        self.0.control.record(&bytes)?;
        Ok(bytes)
    }
    fn write(&self, directory: &Directory, name: &str, bytes: &[u8]) -> JournalResult<()> {
        let file = io(directory.persistent_create(name, bytes))?;
        io(file.persistent_sync())?;
        io(directory.sync())
    }
    fn read_file(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<Option<PrivateFile>> {
        fscheck(directory.persistent_read(name, max, &self.0.exclusions.0))
    }
    fn read<T: DeserializeOwned>(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<T> {
        let file = self
            .read_file(directory, name, max)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(file.bytes())?;
        let value = parse(file.bytes())?;
        io(file.persistent_sync())?;
        Ok(value)
    }
    fn job(&self, operation: Uuid) -> JournalResult<Directory> {
        fscheck(self.0.jobs.persistent_child(&operation.to_string(), false))?
            .ok_or(JournalError::Interrupted)
    }
    fn check_reservation(&self, reservation: &Reservation, operation: Uuid) -> JournalResult<()> {
        if reservation.version != 1
            || reservation.scope_hash != self.0.scope_hash
            || reservation.operation_id != operation
        {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&reservation.submit_hash).map_err(|_| JournalError::Corrupt)
    }
    fn context(
        &self,
        checkpoint: &CheckpointPayloadV1,
        intent: &ExecutionIntentV1,
        binding: &BindingManifestV1,
    ) -> JournalResult<()> {
        let bytes = checkpoint.canonical_bytes()?;
        self.0.control.record(&bytes)?;
        let options = intent.initial_options().run_options();
        let expected = reqwest::Url::parse(&self.0.scope.0.gateway_origin)
            .map_err(|_| JournalError::InvalidInput)?;
        let configured =
            reqwest::Url::parse(&options.gateway).map_err(|_| JournalError::InvalidInput)?;
        if expected.origin() != configured.origin() {
            return Err(JournalError::Conflict);
        }
        checkpoint.validate_journal_context(
            intent,
            &self.0.scope.0.tenant_id,
            &self.0.scope.0.gateway_origin,
            binding.binding_id(),
            binding.names(),
            binding.handoff_sha(),
        )
    }
    fn budget(&self, name: &str, bytes: u64, entries: u32) -> JournalResult<()> {
        self.usage(Some(name))?
            .add(bytes, entries, true, &self.0.limits)
    }
    fn usage(&self, selected: Option<&str>) -> JournalResult<Usage> {
        let mut usage = Usage::default();
        for entry in fscheck(
            self.0
                .directory
                .persistent_entries(self.0.limits.scope_entries),
        )? {
            if !matches!(entry.name.as_str(), ".writer.lock" | "scope.json" | "jobs")
                || entry.directory != (entry.name == "jobs")
            {
                return Err(JournalError::Corrupt);
            }
            usage.add(entry.bytes, 1, false, &self.0.limits)?;
        }
        for entry in fscheck(self.0.jobs.persistent_entries(self.0.limits.scope_entries))? {
            let id = Uuid::parse_str(&entry.name).map_err(|_| JournalError::Corrupt)?;
            if !entry.directory || id.is_nil() || id.to_string() != entry.name {
                return Err(JournalError::Corrupt);
            }
            usage.jobs += 1;
            let active = selected == Some(entry.name.as_str());
            usage.add(0, 1, active, &self.0.limits)?;
            let directory = fscheck(self.0.jobs.persistent_child(&entry.name, false))?
                .ok_or(JournalError::Corrupt)?;
            let mut bytes = 0u64;
            let mut count = 1u32;
            let mut revisions = 0u32;
            for file in fscheck(directory.persistent_entries(self.0.limits.job_entries))? {
                if file.directory {
                    if file.name != "bindings" {
                        return Err(JournalError::Corrupt);
                    }
                    let bindings = fscheck(directory.persistent_child("bindings", false))?
                        .ok_or(JournalError::Corrupt)?;
                    for binding in fscheck(bindings.persistent_entries(self.0.limits.job_entries))?
                    {
                        if binding.directory
                            || !(binding.name == "manual-env-v1.bin"
                                || binding.name.strip_suffix(".json").is_some_and(|id| {
                                    Uuid::parse_str(id)
                                        .is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == id)
                                }))
                        {
                            return Err(JournalError::Corrupt);
                        }
                        bytes = bytes
                            .checked_add(binding.bytes)
                            .ok_or(JournalError::LimitExceeded)?;
                        count += 1;
                        usage.add(binding.bytes, 1, active, &self.0.limits)?;
                    }
                } else if let Some(number) = file
                    .name
                    .strip_prefix("rev-")
                    .and_then(|name| name.strip_suffix(".json"))
                {
                    if number.len() != 20 || !number.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(JournalError::Corrupt);
                    }
                    revisions += 1;
                } else if !matches!(
                    file.name.as_str(),
                    "reservation.json"
                        | "intent.json"
                        | "bindings.json"
                        | "current.json"
                        | "current-next.json"
                        | "accepted.json"
                ) {
                    return Err(JournalError::Corrupt);
                }
                bytes = bytes
                    .checked_add(file.bytes)
                    .ok_or(JournalError::LimitExceeded)?;
                count += 1;
                usage.add(file.bytes, 1, active, &self.0.limits)?;
            }
            if bytes > self.0.limits.job_bytes
                || count > self.0.limits.job_entries
                || revisions > self.0.limits.revisions
            {
                return Err(JournalError::LimitExceeded);
            }
        }
        Ok(usage)
    }
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> JournalResult<T> {
    serde_json::from_slice(bytes).map_err(|_| JournalError::Corrupt)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FaultStage {
    WriterLockAbsent,
    NamespaceDurable,
    ReservationWritten,
    ReservationDurable,
    BindingWritten,
    BindingDurable,
    IntentDurable,
    ManifestDurable,
    RevisionWritten,
    RevisionDurable,
    CurrentWritten,
    CurrentPublished,
    CurrentDurable,
    AcceptedWritten,
    AcceptedDurable,
    BeforeAck,
}
#[cfg(test)]
mod tests;
