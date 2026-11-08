use super::{
    Result, WorkloadCliError as E,
    manifest::{self, Action, Control, Manifest, Request},
};
use crate::{
    audit::Store,
    broker_registry::{BrokerMutationResultV1, parse_time, registry_uuid, revision},
    control::{ControlProtocolV1, SupervisionProtocolV1},
    control_fs::{Directory, PrivateFile, absolute_path},
    workload::WorkloadBindingReceiptV1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_RECORD: u64 = 16_384;
const MAX_STAGES: usize = 16;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recovery {
    pub version: u32,
    pub action: Action,
    pub gateway: String,
    pub expected_tenant_id: String,
    pub control_namespace_uuid: String,
    operation_path: PathBuf,
    device: u64,
    inode: u64,
    uid: u32,
    pub request: Request,
    pub request_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    version: u32,
    request_sha256: String,
    protocol: ControlProtocolV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    request_sha256: String,
    receipt: RecordedReceipt,
}

// Untagged preserves the exact serialized binding-receipt shape in V1 records.
// The recovery action selects and validates the appropriate closed DTO.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RecordedReceipt {
    Binding(WorkloadBindingReceiptV1),
    Broker(BrokerMutationResultV1),
}

impl RecordedReceipt {
    pub fn revision(&self) -> u64 {
        match self {
            Self::Binding(value) => value.revision,
            Self::Broker(value) => value.receipt.grant_revision,
        }
    }
    pub fn replayed(&self) -> bool {
        match self {
            Self::Binding(value) => value.replayed,
            Self::Broker(value) => value.replayed,
        }
    }
    pub fn normalize(&mut self) {
        match self {
            Self::Binding(value) => value.replayed = false,
            Self::Broker(value) => value.replayed = false,
        }
    }
}

pub(super) struct Operation {
    directory: Directory,
    source: PrivateFile,
    credential: Option<PrivateFile>,
    pub recovery: Recovery,
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| E::UnsafeState)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(E::UnsafeState);
    }
    Ok(bytes)
}

fn save(directory: &Directory, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
    let file = directory
        .persistent_create(name, bytes)
        .map_err(|_| E::UnsafeState)?;
    file.persistent_sync().map_err(|_| E::UnsafeState)?;
    directory.sync().map_err(|_| E::UnsafeState)?;
    file.persistent_revalidate().map_err(|_| E::UnsafeState)?;
    Ok(file)
}

impl Operation {
    pub fn prepare(
        manifest: Manifest,
        control: &Control,
        path: &Path,
        runner: &str,
    ) -> Result<Self> {
        let (action, request, secret) = manifest.prepare()?;
        let path = absolute_path(path).map_err(|_| E::UnsafeState)?;
        let leaf = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(E::UnsafeState)?;
        if leaf.is_empty()
            || leaf.len() > 96
            || matches!(leaf, "." | "..")
            || !leaf
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(E::UnsafeState);
        }
        let parent = Directory::open_existing(path.parent().ok_or(E::UnsafeState)?)
            .map_err(|_| E::UnsafeState)?;
        let directory = parent.exclusive_child(leaf).map_err(|_| E::UnsafeState)?;
        let facts = directory
            .try_clone_file()
            .map_err(|_| E::UnsafeState)?
            .metadata()
            .map_err(|_| E::UnsafeState)?;
        let recovery = Recovery {
            version: 1,
            action,
            gateway: control.gateway.to_string(),
            expected_tenant_id: control.expected_tenant_id.clone(),
            control_namespace_uuid: control.control_namespace_uuid.clone(),
            operation_path: directory.path().to_owned(),
            device: facts.dev(),
            inode: facts.ino(),
            uid: facts.uid(),
            request_sha256: request.digest()?,
            request,
        };
        let value = serde_json::to_value(&recovery).map_err(|_| E::UnsafeState)?;
        let mut protected = vec![runner];
        if let Some(key) = secret.as_deref() {
            protected.push(key);
        }
        if super::contains_literal(&value, &protected) {
            return Err(E::UnsafeState);
        }
        let bytes = encode(&recovery)?;
        let credential = secret
            .as_ref()
            .map(|key| save(&directory, "credential", format!("{key}\n").as_bytes()))
            .transpose()?;
        let source = save(&directory, "recovery.json", &bytes)?;
        if !source.persistent_try_lock().map_err(|_| E::UnsafeState)? {
            return Err(E::Busy);
        }
        directory.durable_bootstrap().map_err(|_| E::UnsafeState)?;
        let operation = Self {
            directory,
            source,
            credential,
            recovery,
        };
        operation.revalidate()?;
        Ok(operation)
    }

    fn synchronize_preparation(&self) -> Result<()> {
        // Visible complete bytes after a failed/interrupted preparation are not
        // proof that its file or directory durability barriers completed.
        if let Some(file) = &self.credential {
            file.persistent_sync().map_err(|_| E::UnsafeState)?;
        }
        self.source.persistent_sync().map_err(|_| E::UnsafeState)?;
        self.directory.sync().map_err(|_| E::UnsafeState)?;
        self.directory
            .durable_bootstrap()
            .map_err(|_| E::UnsafeState)?;
        self.revalidate()
    }

    pub fn open(path: &Path) -> Result<Self> {
        let path = absolute_path(path).map_err(|_| E::UnsafeState)?;
        let directory = Directory::open_existing(&path).map_err(|_| E::UnsafeState)?;
        inventory(&directory)?;
        let source = directory
            .persistent_read("recovery.json", MAX_RECORD, &[])
            .map_err(|_| E::UnsafeState)?
            .ok_or(E::IncompletePreparation)?;
        if !source.persistent_try_lock().map_err(|_| E::UnsafeState)? {
            return Err(E::Busy);
        }
        let recovery: Recovery =
            serde_json::from_slice(source.bytes()).map_err(|_| E::IncompletePreparation)?;
        let credential = directory
            .persistent_read("credential", 128, &[])
            .map_err(|_| E::UnsafeState)?;
        let operation = Self {
            directory,
            source,
            credential,
            recovery,
        };
        operation.revalidate()?;
        operation.synchronize_preparation()?;
        Ok(operation)
    }

    pub fn require_control(&self, control: &Control) -> Result<()> {
        if self.recovery.gateway != control.gateway.as_str()
            || self.recovery.expected_tenant_id != control.expected_tenant_id
            || self.recovery.control_namespace_uuid != control.control_namespace_uuid
        {
            return Err(E::ScopeMismatch);
        }
        Ok(())
    }

    pub fn revalidate(&self) -> Result<()> {
        inventory(&self.directory)?;
        self.directory.revalidate().map_err(|_| E::UnsafeState)?;
        self.source
            .persistent_revalidate()
            .map_err(|_| E::UnsafeState)?;
        let facts = self
            .directory
            .try_clone_file()
            .map_err(|_| E::UnsafeState)?
            .metadata()
            .map_err(|_| E::UnsafeState)?;
        let recovery = &self.recovery;
        if recovery.version != 1
            || recovery.operation_path != self.directory.path()
            || recovery.device != facts.dev()
            || recovery.inode != facts.ino()
            || recovery.uid != facts.uid()
            || recovery.request_sha256 != recovery.request.digest()?
            || manifest::origin(&recovery.gateway)?.as_str() != recovery.gateway
        {
            return Err(E::UnsafeState);
        }
        crate::tenancy::validate_tenant_id(&recovery.expected_tenant_id)
            .map_err(|_| E::UnsafeState)?;
        manifest::canonical_uuid(&recovery.control_namespace_uuid)?;
        recovery.request.validate(recovery.action)?;
        match (&recovery.request, &self.credential) {
            (Request::Issue(request), Some(file)) => {
                file.persistent_revalidate().map_err(|_| E::UnsafeState)?;
                let key = key(file)?;
                if request.sha256_digest != hex::encode(Sha256::digest(key.as_bytes())) {
                    return Err(E::UnsafeState);
                }
            }
            (Request::Revoke(_) | Request::BrokerGrant(_) | Request::BrokerRevoke(_), None) => {}
            _ => return Err(E::IncompletePreparation),
        }
        if let Some(scope) = self.read_record::<Scope>("scope", "scope.json")? {
            if scope.version != 1 || scope.request_sha256 != recovery.request_sha256 {
                return Err(E::UnsafeState);
            }
            self.validate_protocol(&scope.protocol)?;
        }
        Ok(())
    }

    fn validate_protocol(&self, protocol: &ControlProtocolV1) -> Result<()> {
        protocol.validate().map_err(|_| E::Protocol)?;
        if protocol.version != 1
            || !matches!(protocol.schema_version, 6 | 7 | 8 | 9 | 10 | 11)
            || protocol.role != "runner"
            || protocol.tenant_id != self.recovery.expected_tenant_id
            || protocol.control_namespace_uuid.as_deref()
                != Some(self.recovery.control_namespace_uuid.as_str())
            || protocol.protocols != [SupervisionProtocolV1::BrokerSupervisionV1]
        {
            return Err(E::ScopeMismatch);
        }
        Ok(())
    }

    pub fn confirm_scope(&self, protocol: &ControlProtocolV1) -> Result<()> {
        self.revalidate()?;
        self.validate_protocol(protocol)?;
        if self.read_record::<Scope>("scope", "scope.json")?.is_none() {
            self.publish(
                "scope",
                "scope.json",
                &Scope {
                    version: 1,
                    request_sha256: self.recovery.request_sha256.clone(),
                    protocol: protocol.clone(),
                },
            )?;
        }
        self.directory
            .durable_bootstrap()
            .map_err(|_| E::UnsafeState)?;
        self.revalidate()
    }

    pub fn has_scope(&self) -> Result<bool> {
        self.revalidate()?;
        Ok(self.read_record::<Scope>("scope", "scope.json")?.is_some())
    }

    pub fn receipt(&self) -> Result<Option<RecordedReceipt>> {
        self.revalidate()?;
        let Some(record) = self.read_record::<Receipt>("receipt", "receipt.json")? else {
            return Ok(None);
        };
        if self.read_record::<Scope>("scope", "scope.json")?.is_none()
            || record.version != 1
            || record.request_sha256 != self.recovery.request_sha256
        {
            return Err(E::UnsafeState);
        }
        validate_receipt(&self.recovery, &record.receipt)?;
        if record.receipt.replayed() {
            return Err(E::UnsafeState);
        }
        Ok(Some(record.receipt))
    }

    pub fn save_receipt(&self, receipt: &RecordedReceipt) -> Result<()> {
        self.revalidate()?;
        validate_receipt(&self.recovery, receipt)?;
        if let Some(previous) = self.receipt()? {
            if previous != *receipt {
                return Err(E::Conflict);
            }
        } else {
            if self.read_record::<Scope>("scope", "scope.json")?.is_none() {
                return Err(E::UnsafeState);
            }
            self.publish(
                "receipt",
                "receipt.json",
                &Receipt {
                    version: 1,
                    request_sha256: self.recovery.request_sha256.clone(),
                    receipt: receipt.clone(),
                },
            )?;
        }
        self.directory
            .durable_bootstrap()
            .map_err(|_| E::UnsafeState)?;
        self.receipt()?.ok_or(E::UnsafeState)?;
        Ok(())
    }

    fn read_record<T: for<'de> Deserialize<'de>>(
        &self,
        name: &str,
        file: &str,
    ) -> Result<Option<T>> {
        let Some(directory) = self
            .directory
            .persistent_child(name, false)
            .map_err(|_| E::UnsafeState)?
        else {
            return Ok(None);
        };
        let entries = directory
            .persistent_entries(2)
            .map_err(|_| E::UnsafeState)?;
        if entries.len() != 1 || entries[0].name != file || entries[0].directory {
            return Err(E::UnsafeState);
        }
        let source = directory
            .persistent_read(file, MAX_RECORD, &[])
            .map_err(|_| E::UnsafeState)?
            .ok_or(E::UnsafeState)?;
        let value = serde_json::from_slice(source.bytes()).map_err(|_| E::UnsafeState)?;
        source.persistent_sync().map_err(|_| E::UnsafeState)?;
        directory.durable_bootstrap().map_err(|_| E::UnsafeState)?;
        Ok(Some(value))
    }

    fn publish(&self, target: &str, file: &str, value: &impl Serialize) -> Result<()> {
        if inventory(&self.directory)? >= MAX_STAGES {
            return Err(E::UnsafeState);
        }
        let staged = self
            .directory
            .exclusive_child(&format!(".stage-{target}-{}", Uuid::new_v4()))
            .map_err(|_| E::UnsafeState)?;
        let record = save(&staged, file, &encode(value)?)?;
        record.persistent_sync().map_err(|_| E::UnsafeState)?;
        staged.durable_bootstrap().map_err(|_| E::UnsafeState)?;
        // Publication never replaces a prior entry. After rename, open the final
        // path and synchronize it; the staged handle's old pathname is obsolete.
        let published = self.directory.publish_child_exclusive(&staged, target);
        if published.is_err()
            && self
                .directory
                .persistent_child(target, false)
                .map_err(|_| E::UnsafeState)?
                .is_none()
        {
            return Err(E::UnsafeState);
        }
        let final_directory = self
            .directory
            .persistent_child(target, false)
            .map_err(|_| E::UnsafeState)?
            .ok_or(E::UnsafeState)?;
        let final_record = final_directory
            .persistent_read(file, MAX_RECORD, &[])
            .map_err(|_| E::UnsafeState)?
            .ok_or(E::UnsafeState)?;
        if final_record.bytes() != encode(value)? {
            return Err(E::Conflict);
        }
        final_record.persistent_sync().map_err(|_| E::UnsafeState)?;
        final_directory
            .durable_bootstrap()
            .map_err(|_| E::UnsafeState)
    }

    pub fn report(
        &self,
        status: &str,
        receipt: Option<&RecordedReceipt>,
        extra_secrets: &[&str],
    ) -> Result<()> {
        self.revalidate()?;
        let mut secrets = extra_secrets.to_vec();
        if let Some(file) = &self.credential {
            secrets.push(key(file)?);
        }
        let mut report = serde_json::json!({
            "version":1, "status":status, "action":self.recovery.action.name(),
            "operation_id":self.recovery.request.operation_id(), "credential_id":self.recovery.request.id(),
            "revision":receipt.map(RecordedReceipt::revision), "receipt":receipt,
            "current_state":"not_observed", "consumer_delivery":"not_observed",
            "credential_file":self.credential.as_ref().map(|_|self.directory.path().join("credential")),
            "recovery_file":self.directory.path().join("recovery.json"),
            "operation_dir":self.directory.path(),
        });
        match &self.recovery.request {
            Request::BrokerGrant(request) => {
                report["broker_id"] = serde_json::json!(request.broker_id);
                report["credential_id"] = serde_json::json!(request.credential_id);
            }
            Request::BrokerRevoke(request) => {
                report["broker_id"] = serde_json::json!(request.broker_id);
                report["credential_id"] = serde_json::Value::Null;
            }
            Request::Issue(_) | Request::Revoke(_) => {}
        }
        if let Some(RecordedReceipt::Broker(result)) = receipt {
            report["receipt"] = serde_json::to_value(&result.receipt).map_err(|_| E::Output)?;
        }
        super::print_json(&report, &secrets)
    }

    pub fn protect(&self, value: &impl Serialize, current_runner: &str) -> Result<()> {
        let value = serde_json::to_value(value).map_err(|_| E::UnsafeState)?;
        let mut secrets = vec![current_runner];
        if let Some(file) = &self.credential {
            secrets.push(key(file)?);
        }
        if super::contains_literal(&value, &secrets) {
            return Err(E::UnsafeState);
        }
        Ok(())
    }
}

fn key(file: &PrivateFile) -> Result<&str> {
    let value = std::str::from_utf8(file.bytes()).map_err(|_| E::UnsafeState)?;
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value.len() != 70
        || !value.starts_with("aigw1_")
        || !value[6..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(E::UnsafeState);
    }
    Ok(value)
}

fn inventory(directory: &Directory) -> Result<usize> {
    let entries = directory
        .persistent_entries(24)
        .map_err(|_| E::UnsafeState)?;
    let mut stages = 0;
    for entry in entries {
        match entry.name.as_str() {
            "credential" | "recovery.json" if !entry.directory => {}
            "scope" | "receipt" if entry.directory => {}
            name if entry.directory => {
                let id = name
                    .strip_prefix(".stage-scope-")
                    .or_else(|| name.strip_prefix(".stage-receipt-"))
                    .ok_or(E::UnsafeState)?;
                manifest::canonical_uuid(id)?;
                stages += 1;
            }
            _ => return Err(E::UnsafeState),
        }
    }
    if stages > MAX_STAGES {
        return Err(E::UnsafeState);
    }
    Ok(stages)
}

pub(super) fn validate_receipt(recovery: &Recovery, receipt: &RecordedReceipt) -> Result<()> {
    match (&recovery.request, receipt) {
        (Request::Issue(_) | Request::Revoke(_), RecordedReceipt::Binding(receipt)) => {
            validate_binding_receipt(recovery, receipt)
        }
        (Request::BrokerGrant(_) | Request::BrokerRevoke(_), RecordedReceipt::Broker(result)) => {
            validate_broker_receipt(recovery, result)
        }
        _ => Err(E::Protocol),
    }
}

fn validate_binding_receipt(recovery: &Recovery, receipt: &WorkloadBindingReceiptV1) -> Result<()> {
    if receipt.version != 1
        || receipt.credential_id != recovery.request.id()
        || receipt.revision
            != recovery
                .request
                .revision()
                .checked_add(1)
                .ok_or(E::Protocol)?
        || receipt.audit_seq <= 0
        || receipt.audit_hash.len() != 64
        || !receipt
            .audit_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(E::Protocol);
    }
    manifest::canonical_uuid(&receipt.audit_event_id).map_err(|_| E::Protocol)
}

fn validate_broker_receipt(recovery: &Recovery, result: &BrokerMutationResultV1) -> Result<()> {
    let broker = &result.broker;
    let receipt = &result.receipt;
    let expected = crate::broker_registry::next_revision(recovery.request.revision())
        .map_err(|_| E::Protocol)?;
    if broker.version != 1
        || broker.broker_id != recovery.request.id()
        || receipt.broker_id != broker.broker_id
        || receipt.operation_id != recovery.request.operation_id()
        || broker.grant_revision != expected
        || receipt.grant_revision != expected
        || receipt.heartbeat_revision != broker.heartbeat_revision
        || receipt.audit_seq <= 0
        || !digest_shape(&receipt.audit_hash)
    {
        return Err(E::Protocol);
    }
    for id in [
        &broker.broker_id,
        &broker.credential_id,
        &broker.incarnation_id,
    ] {
        registry_uuid(id).map_err(|_| E::Protocol)?;
    }
    revision(broker.heartbeat_revision).map_err(|_| E::Protocol)?;
    let granted = parse_time(&broker.granted_at).map_err(|_| E::Protocol)?;
    if parse_time(&broker.not_after).map_err(|_| E::Protocol)? <= granted {
        return Err(E::Protocol);
    }
    if !(1..=300).contains(&broker.lease_seconds)
        || broker.authorities.is_empty()
        || broker.authorities.len() > 16
    {
        return Err(E::Protocol);
    }
    let mut authorities = std::collections::BTreeSet::new();
    for authority in &broker.authorities {
        authority.validate().map_err(|_| E::Protocol)?;
        if !authorities.insert(&authority.authority_id) {
            return Err(E::Protocol);
        }
    }
    match (broker.revoked, &broker.revoked_at) {
        (false, None) => {}
        (true, Some(at)) if parse_time(at).map_err(|_| E::Protocol)? >= granted => {}
        _ => return Err(E::Protocol),
    }
    match &broker.last_heartbeat {
        None if broker.heartbeat_revision == 0 => {}
        Some(heartbeat) if broker.heartbeat_revision > 0 => {
            registry_uuid(&heartbeat.incarnation_id).map_err(|_| E::Protocol)?;
            revision(heartbeat.sequence).map_err(|_| E::Protocol)?;
            if heartbeat.sequence == 0
                || parse_time(&heartbeat.lease_expires_at).map_err(|_| E::Protocol)?
                    < parse_time(&heartbeat.received_at).map_err(|_| E::Protocol)?
            {
                return Err(E::Protocol);
            }
        }
        _ => return Err(E::Protocol),
    }
    let input_hash = match &recovery.request {
        Request::BrokerGrant(request) => {
            if broker.revoked
                || (request.expected_revision.is_none()
                    && (broker.heartbeat_revision != 0 || broker.last_heartbeat.is_some()))
                || broker.credential_id != request.credential_id
                || broker.incarnation_id != request.incarnation_id
                || broker.authorities != request.authorities
                || broker.lease_seconds != request.lease_seconds
                || parse_time(&broker.not_after)
                    .map_err(|_| E::Protocol)?
                    .timestamp_millis()
                    != parse_time(&request.not_after)
                        .map_err(|_| E::Protocol)?
                        .timestamp_millis()
            {
                return Err(E::Protocol);
            }
            Store::broker_grant_input_hash(request)
        }
        Request::BrokerRevoke(request) if broker.revoked => {
            Store::broker_revocation_input_hash(request)
        }
        _ => return Err(E::Protocol),
    }
    .map_err(|_| E::Protocol)?;
    if receipt.input_hash != input_hash {
        return Err(E::Protocol);
    }
    Ok(())
}

fn digest_shape(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_fs::cost_probe::{Measurement, Operation as FsOperation};
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn resume_resynchronizes_complete_files_before_returning() {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let runner = "public-workload-sync-fixture-runner-0000000000000000";
        let control_path = root.join("control.json");
        fs::write(
            &control_path,
            serde_json::to_vec(&serde_json::json!({
                "version":1, "gateway":"http://127.0.0.1:1/", "expected_tenant_id":"tenant-a",
                "control_namespace_uuid":"11111111-1111-4111-8111-111111111111",
                "runner_token_file":root.join("runner"), "ca_bundle_file":null
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&control_path, fs::Permissions::from_mode(0o600)).unwrap();
        let control = Control::load(&control_path).unwrap();
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "version":1, "action":"issue", "expected_revision":0,
            "subject":{"kind":"broker","broker_id":"22222222-2222-4222-8222-222222222222","incarnation_id":"33333333-3333-4333-8333-333333333333"},
            "expires_at":"2100-01-01T00:00:00.000Z"
        })).unwrap();
        let path = root.join("operation");
        let prepared = Operation::prepare(manifest, &control, &path, runner).unwrap();
        let key = fs::read(path.join("credential")).unwrap();
        let recovery = fs::read(path.join("recovery.json")).unwrap();
        drop(prepared);
        // Complete retained bytes may be visible after an interrupted write
        // without its durability barrier. Do not replace sync_all with a mock.
        fs::write(path.join("credential"), key).unwrap();
        fs::write(path.join("recovery.json"), recovery).unwrap();
        let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.blocking_lock();
        let measurement = Measurement::start(&path).unwrap();
        let reopened = Operation::open(&path).unwrap();
        let sample = measurement.finish().unwrap();
        assert_eq!(
            sample.metric(FsOperation::FileSyncAll).succeeded,
            2,
            "resume must synchronize both retained preparation files before HTTP is possible"
        );
        assert!(
            sample.metric(FsOperation::DirectorySyncAll).succeeded >= 1,
            "resume must synchronize its private directory before returning"
        );
        assert_eq!(sample.outstanding, 0);
        drop(reopened);
    }
}
