//! Immutable `relay_v1` business records, separate from audit and checkpoints.
//! No pruning, repair, implicit replay or proof is persisted here.
use super::*;
use crate::broker::journal::{JournalScopeV1, canonical};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const NAMESPACE: &str = "relay_v1";
const MAX_OPERATIONS: usize = 1024;
const MAX_TENANT_BYTES: usize = 64 * 1024 * 1024;
const RESULT_RESERVATION: usize = 32 * 1024;
const MAX_RECORD: u64 = (validation::MAX_REQUEST_BYTES + MAX_CONTROL_FRAME_BYTES) as u64;
const LOCK_BYTES: &[u8] = b"{\"format\":\"relay_v1\",\"version\":1}";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeRecord {
    format: String,
    version: u32,
    scope: JournalScopeV1,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Mutation {
    Send(crate::relay::RelaySendV1),
    Ack(crate::relay::RelayAckV1),
}
impl Mutation {
    fn id(&self) -> &str {
        match self {
            Self::Send(input) => &input.operation_id,
            Self::Ack(input) => &input.operation_id,
        }
    }
    fn resource(&self) -> &str {
        match self {
            Self::Send(input) => &input.operation_id,
            Self::Ack(input) => &input.message_id,
        }
    }
    fn validate(&self, secrets: &Secrets) -> Result<(), ControlError> {
        match self {
            Self::Send(input) => {
                validation::validate_send(input, None).map_err(|_| ControlError::InvalidRequest)?;
                for secret in &secrets.0 {
                    validation::validate_send(input, Some(secret))
                        .map_err(|_| ControlError::InvalidRequest)?;
                }
            }
            Self::Ack(input) => {
                validation::validate_ack(input).map_err(|_| ControlError::InvalidRequest)?
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    format: String,
    version: u32,
    endpoint: RelayEndpointV1,
    credential_id: String,
    request: Mutation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchRecord {
    format: String,
    version: u32,
    request_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRecord {
    format: String,
    version: u32,
    request_sha256: String,
    receipt: crate::relay::RelayReceiptV1,
}

struct Stored {
    directory: Directory,
    request: RequestRecord,
    exact: PrivateFile,
    dispatch: Option<PrivateFile>,
    receipt: Option<PrivateFile>,
}
impl Stored {
    fn revalidate(&self) -> Result<(), ControlError> {
        fs(self.directory.revalidate())?;
        for file in std::iter::once(&self.exact)
            .chain(self.dispatch.iter())
            .chain(self.receipt.iter())
        {
            fs(file.persistent_revalidate())?;
        }
        // Inventory notices unretained/partial/unexpected records as well.
        let expected =
            1 + usize::from(self.dispatch.is_some()) + usize::from(self.receipt.is_some());
        if fs(self.directory.persistent_entries(4))?.len() != expected {
            return Err(ControlError::Persistence);
        }
        Ok(())
    }
}

pub(super) struct Outbox {
    directory: Directory,
    scope: PrivateFile,
    lease: PrivateFile,
    operations: BTreeMap<String, Stored>,
    reserved: usize,
    poisoned: bool,
}
impl Outbox {
    /// Must be supplied a dedicated broker-business parent outside any database,
    /// source, workspace or export namespace. Existing `relay_v1` is never adopted.
    pub(super) fn create(
        parent: &Directory,
        scope: &JournalScopeV1,
        secrets: &Secrets,
    ) -> Result<Self, ControlError> {
        let scope_bytes = encode(&ScopeRecord {
            format: NAMESPACE.into(),
            version: 1,
            scope: scope.clone(),
        })?;
        secrets.check(&scope_bytes)?;
        secrets.check(LOCK_BYTES)?;
        let directory = fs(parent.exclusive_child(NAMESPACE))?;
        let lease = durable(&directory, ".owner.lock", LOCK_BYTES, secrets)?;
        if !fs(lease.persistent_try_lock())? {
            return Err(ControlError::Busy);
        }
        let scope = durable(&directory, "scope.json", &scope_bytes, secrets)?;
        fs(directory.durable_bootstrap())?;
        Ok(Self {
            directory,
            scope,
            lease,
            operations: BTreeMap::new(),
            reserved: scope_bytes.len() + 64,
            poisoned: false,
        })
    }

    /// Reopening is maintenance only: it grants no channel/runtime authority.
    /// A dispatch without a receipt is retained as unresolved, never rolled back.
    pub(super) fn open(
        parent: &Directory,
        scope: &JournalScopeV1,
        secrets: &Secrets,
    ) -> Result<Self, ControlError> {
        let directory =
            fs(parent.persistent_child(NAMESPACE, false))?.ok_or(ControlError::Persistence)?;
        let lease = read(&directory, ".owner.lock", MAX_CONTROL_FRAME_BYTES as u64)?
            .ok_or(ControlError::Persistence)?;
        secrets.check(lease.bytes())?;
        if lease.bytes() != LOCK_BYTES {
            return Err(ControlError::Persistence);
        }
        if !fs(lease.persistent_try_lock())? {
            return Err(ControlError::Busy);
        }
        let scope_file = read(&directory, "scope.json", MAX_CONTROL_FRAME_BYTES as u64)?
            .ok_or(ControlError::Persistence)?;
        secrets.check(scope_file.bytes())?;
        let actual: ScopeRecord = decode(scope_file.bytes())?;
        let expected = encode(&ScopeRecord {
            format: NAMESPACE.into(),
            version: 1,
            scope: scope.clone(),
        })?;
        if encode(&actual)? != expected {
            return Err(ControlError::Binding);
        }
        let mut outbox = Self {
            directory,
            scope: scope_file,
            lease,
            operations: BTreeMap::new(),
            reserved: expected.len() + 64,
            poisoned: false,
        };
        for entry in fs(outbox
            .directory
            .persistent_entries((MAX_OPERATIONS + 2) as u32))?
        {
            secrets.check(&encode(&entry.name)?)?;
            if matches!(entry.name.as_str(), "scope.json" | ".owner.lock") && !entry.directory {
                continue;
            }
            let id = entry
                .name
                .strip_prefix("op-")
                .ok_or(ControlError::Persistence)?;
            if !entry.directory || !validation::uuid(id) {
                return Err(ControlError::Persistence);
            }
            let child = fs(outbox.directory.persistent_child(&entry.name, false))?
                .ok_or(ControlError::Persistence)?;
            let exact =
                read(&child, "request.json", MAX_RECORD)?.ok_or(ControlError::Persistence)?;
            secrets.check(exact.bytes())?;
            let request: RequestRecord = decode(exact.bytes())?;
            validate_record(&request, id, secrets)?;
            if encode(&request)? != exact.bytes() {
                return Err(ControlError::Persistence);
            }
            let dispatch = read(&child, "dispatch.json", MAX_CONTROL_FRAME_BYTES as u64)?;
            let receipt = read(&child, "receipt.json", MAX_CONTROL_FRAME_BYTES as u64)?;
            if let Some(file) = &dispatch {
                secrets.check(file.bytes())?;
                let marker: DispatchRecord = decode(file.bytes())?;
                if marker.format != NAMESPACE
                    || marker.version != 1
                    || marker.request_sha256 != exact.digest()
                {
                    return Err(ControlError::Persistence);
                }
            }
            if let Some(file) = &receipt {
                secrets.check(file.bytes())?;
                let confirmed: ReceiptRecord = decode(file.bytes())?;
                if dispatch.is_none()
                    || confirmed.format != NAMESPACE
                    || confirmed.version != 1
                    || confirmed.request_sha256 != exact.digest()
                {
                    return Err(ControlError::Persistence);
                }
                validation::validate_receipt(
                    &confirmed.receipt,
                    request.request.id(),
                    request.request.resource(),
                )
                .map_err(|_| ControlError::Persistence)?;
            }
            let stored = Stored {
                directory: child,
                request,
                exact,
                dispatch,
                receipt,
            };
            stored.revalidate()?;
            outbox.reserved = outbox
                .reserved
                .checked_add(stored.exact.bytes().len() + RESULT_RESERVATION)
                .ok_or(ControlError::Capacity)?;
            if outbox.reserved > MAX_TENANT_BYTES {
                return Err(ControlError::Capacity);
            }
            if outbox.operations.values().any(|prior| {
                prior.request.endpoint == stored.request.endpoint
                    && prior.receipt.is_none()
                    && stored.receipt.is_none()
            }) {
                return Err(ControlError::Persistence);
            }
            outbox.operations.insert(id.into(), stored);
        }
        outbox.revalidate()?;
        Ok(outbox)
    }

    pub(super) fn revalidate(&self) -> Result<(), ControlError> {
        if self.poisoned {
            return Err(ControlError::Persistence);
        }
        fs(self.scope.persistent_revalidate())?;
        fs(self.lease.persistent_revalidate())?;
        if fs(self
            .directory
            .persistent_entries((MAX_OPERATIONS + 2) as u32))?
        .len()
            != self.operations.len() + 2
        {
            return Err(ControlError::Persistence);
        }
        for stored in self.operations.values() {
            stored.revalidate()?;
        }
        Ok(())
    }

    pub(super) fn prepare(
        &mut self,
        binding: &Binding,
        input: &RequestPayloadV1,
        client: &WorkerRelayClient,
        secrets: &Secrets,
    ) -> Result<(), ControlError> {
        // Both final HTTP target and decoded body secrets are checked before
        // creating even an operation directory. These bytes confer no authority.
        let request = match input {
            RequestPayloadV1::Send(input) => {
                client.validated_send_bytes(input).map_err(client_error)?;
                Mutation::Send(input.clone())
            }
            RequestPayloadV1::Ack(input) => {
                client.validated_ack_bytes(input).map_err(client_error)?;
                Mutation::Ack(input.clone())
            }
            RequestPayloadV1::Inbox(_) => return Err(ControlError::InvalidRequest),
        };
        request.validate(secrets)?;
        let record = RequestRecord {
            format: NAMESPACE.into(),
            version: 1,
            endpoint: binding.endpoint.clone(),
            credential_id: binding.credential_id.clone(),
            request,
        };
        validate_record(&record, record.request.id(), secrets)?;
        let bytes = encode(&record)?;
        secrets.check(&bytes)?;
        let operation_name = format!("op-{}", record.request.id());
        secrets.check(&encode(&operation_name)?)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(ControlError::Capacity);
        }
        self.revalidate()?;
        if let Some(prior) = self.operations.get(record.request.id()) {
            return if prior.exact.bytes() == bytes {
                Ok(())
            } else {
                Err(ControlError::Conflict)
            };
        }
        if self.operations.len() >= MAX_OPERATIONS
            || self
                .reserved
                .saturating_add(bytes.len() + RESULT_RESERVATION)
                > MAX_TENANT_BYTES
            || self.unresolved(binding)?
        {
            return Err(ControlError::Capacity);
        }
        // From the first mutation, any failure poisons this owner. Partial files
        // remain for diagnosis and a new owner refuses them instead of repair.
        self.poisoned = true;
        let directory = fs(self.directory.exclusive_child(&operation_name))?;
        let exact = durable(&directory, "request.json", &bytes, secrets)?;
        self.reserved += bytes.len() + RESULT_RESERVATION;
        self.operations.insert(
            record.request.id().into(),
            Stored {
                directory,
                request: record,
                exact,
                dispatch: None,
                receipt: None,
            },
        );
        self.poisoned = false;
        Ok(())
    }

    pub(super) fn dispatched(
        &mut self,
        binding: &Binding,
        input: &RequestPayloadV1,
        secrets: &Secrets,
    ) -> Result<(), ControlError> {
        self.revalidate()?;
        let id = mutation_id(input)?;
        let stored = self
            .operations
            .get_mut(id)
            .ok_or(ControlError::Persistence)?;
        if stored.request.credential_id != binding.credential_id
            || stored.request.endpoint != binding.endpoint
        {
            return Err(ControlError::Binding);
        }
        if stored.dispatch.is_some() {
            return Ok(());
        }
        let bytes = encode(&DispatchRecord {
            format: NAMESPACE.into(),
            version: 1,
            request_sha256: stored.exact.digest().into(),
        })?;
        secrets.check(&bytes)?;
        self.poisoned = true;
        stored.dispatch = Some(durable(
            &stored.directory,
            "dispatch.json",
            &bytes,
            secrets,
        )?);
        self.poisoned = false;
        Ok(())
    }

    pub(super) fn confirmed(
        &mut self,
        input: &RequestPayloadV1,
        receipt: &crate::relay::RelayReceiptV1,
        secrets: &Secrets,
    ) -> Result<(), ControlError> {
        self.revalidate()?;
        let stored = self
            .operations
            .get_mut(mutation_id(input)?)
            .ok_or(ControlError::Persistence)?;
        if stored.dispatch.is_none() {
            return Err(ControlError::Persistence);
        }
        validation::validate_receipt(
            receipt,
            stored.request.request.id(),
            stored.request.request.resource(),
        )
        .map_err(|_| ControlError::Protocol)?;
        let record = ReceiptRecord {
            format: NAMESPACE.into(),
            version: 1,
            request_sha256: stored.exact.digest().into(),
            receipt: receipt.clone(),
        };
        let bytes = encode(&record)?;
        secrets.check(&bytes)?;
        if bytes.len() > MAX_CONTROL_FRAME_BYTES {
            return Err(ControlError::Capacity);
        }
        if let Some(prior) = &stored.receipt {
            let old: ReceiptRecord = decode(prior.bytes())?;
            if old.receipt.operation_id != receipt.operation_id
                || old.receipt.resource_id != receipt.resource_id
                || old.receipt.audit_seq != receipt.audit_seq
                || old.receipt.audit_hash != receipt.audit_hash
            {
                return Err(ControlError::Conflict);
            }
            return Ok(());
        }
        self.poisoned = true;
        stored.receipt = Some(durable(&stored.directory, "receipt.json", &bytes, secrets)?);
        self.poisoned = false;
        Ok(())
    }

    pub(super) fn unresolved(&self, binding: &Binding) -> Result<bool, ControlError> {
        self.revalidate()?;
        Ok(self
            .operations
            .values()
            .any(|stored| stored.request.endpoint == binding.endpoint && stored.receipt.is_none()))
    }

    #[cfg(test)]
    pub(super) fn operation_count(&self) -> Result<usize, ControlError> {
        self.revalidate()?;
        Ok(self.operations.len())
    }
}

fn validate_record(
    record: &RequestRecord,
    id: &str,
    secrets: &Secrets,
) -> Result<(), ControlError> {
    if record.format != NAMESPACE
        || record.version != 1
        || record.request.id() != id
        || !validation::uuid(&record.credential_id)
        || record.endpoint.validate().is_err()
    {
        return Err(ControlError::Persistence);
    }
    record.request.validate(secrets)
}
fn mutation_id(input: &RequestPayloadV1) -> Result<&str, ControlError> {
    match input {
        RequestPayloadV1::Send(input) => Ok(&input.operation_id),
        RequestPayloadV1::Ack(input) => Ok(&input.operation_id),
        RequestPayloadV1::Inbox(_) => Err(ControlError::InvalidRequest),
    }
}
fn encode(input: &impl Serialize) -> Result<Vec<u8>, ControlError> {
    canonical(input).map_err(|_| ControlError::Persistence)
}
fn decode<T: serde::de::DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, ControlError> {
    validation::decode(bytes, None).map_err(|_| ControlError::Persistence)
}
fn fs<T>(result: anyhow::Result<T>) -> Result<T, ControlError> {
    result.map_err(|_| ControlError::Persistence)
}
fn read(
    directory: &Directory,
    name: &str,
    maximum: u64,
) -> Result<Option<PrivateFile>, ControlError> {
    fs(directory.persistent_read(name, maximum, &[]))
}
fn durable(
    directory: &Directory,
    name: &str,
    bytes: &[u8],
    secrets: &Secrets,
) -> Result<PrivateFile, ControlError> {
    secrets.check(bytes)?;
    let file = fs(directory.persistent_create(name, bytes))?;
    fs(file.persistent_sync())?;
    fs(directory.sync())?;
    fs(file.persistent_revalidate())?;
    Ok(file)
}
