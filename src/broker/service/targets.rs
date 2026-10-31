//! Immutable export inputs, distinct from an export grant or completion.
//!
//! A new process authorizes a new parent capability and compares its real
//! identity with the original input. A saved pathname never reconstructs a
//! capability. Existing destinations remain the executor's responsibility: it
//! may replay an acknowledged completion, but cannot adopt an unknown tree.

use super::{
    ValidatedExportTarget, profile::ControlProfileV1, protocol::BrokerErrorCodeV1 as Code,
};
use crate::{
    broker::journal::{JournalScopeV1, canonical},
    control_fs::{Directory, PrivateFile, absolute_path},
    workspace::AuthorizedExportParent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::ErrorKind,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const MAX_INPUT_BYTES: u64 = 32 * 1024;
const MAX_INPUT_FILES: u32 = 16 * 1024;
const MAX_PROFILE_BYTES: u64 = 64 * 1024 * 1024;
const LOCK_NAME: &str = "writer.lock";
const LOCK_BYTES: &[u8] = b"ai-guard:broker-export-inputs-lock:v1\n";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportInputV1 {
    version: u32,
    scope_hash: String,
    job_operation_id: Uuid,
    export_request_id: Uuid,
    destination_normalized: String,
    parent_device: u64,
    parent_inode: u64,
    target_name: String,
}

/// An immutable export input already reserves this command identity, even if
/// a crash occurred before its central ExportRequest was checkpointed. This is
/// metadata-only and never creates a namespace or opens an unknown file.
pub(super) fn reject_reserved_id(
    root: &Directory,
    profile: &str,
    request: Uuid,
) -> Result<(), Code> {
    let Some(inputs) = root
        .persistent_child("export-inputs", false)
        .map_err(|_| Code::NeedsReconciliation)?
    else {
        return Ok(());
    };
    let Some(directory) = inputs
        .persistent_child(profile, false)
        .map_err(|_| Code::NeedsReconciliation)?
    else {
        return Ok(());
    };
    let name = format!("{request}.json");
    match directory
        .private_entry(std::ffi::OsStr::new(&name))
        .map_err(|_| Code::NeedsReconciliation)?
    {
        Some(_) => Err(Code::Conflict),
        None => Ok(()),
    }
}

/// Called only after fresh gateway authentication and under the job's export
/// gate, from the service's bounded blocking-I/O pool. This namespace may hold
/// only the generated input records and its technical lease, never a database.
pub(super) fn validate_and_persist(
    root: &Directory,
    profile: &ControlProfileV1,
    scope: &JournalScopeV1,
    operation: Uuid,
    request: Uuid,
    destination: &Path,
) -> Result<ValidatedExportTarget, Code> {
    if operation.is_nil() || request.is_nil() {
        return Err(Code::InvalidInput);
    }
    if scope.profile_id() != profile.id
        || scope.tenant_id() != profile.tenant_id
        || scope.gateway_origin() != profile.gateway
        || scope.control_namespace_uuid() != profile.control_namespace_uuid
    {
        return Err(Code::Forbidden);
    }
    let destination = normalized_destination(destination)?;
    let target_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 255
                && *name != "."
                && *name != ".."
                && !name.chars().any(char::is_control)
        })
        .ok_or(Code::InvalidInput)?
        .to_owned();
    let parent_path = destination.parent().ok_or(Code::InvalidInput)?;

    root.revalidate().map_err(|_| Code::NeedsReconciliation)?;
    // An allowed export root must also stay physically disjoint from private
    // broker/configuration storage. Broad roots containing that storage are
    // refused conservatively instead of treating an alias as a new authority.
    let mut forbidden = profile.source_roots.clone();
    for path in [root.path(), profile.config_dir.as_path()] {
        if !forbidden.iter().any(|existing| existing == path) {
            forbidden.push(path.to_owned());
        }
    }
    let parent = Arc::new(
        AuthorizedExportParent::open(parent_path, &profile.export_roots, &forbidden)
            .map_err(|_| Code::Forbidden)?,
    );
    let (parent_device, parent_inode) = parent.identity().map_err(|_| Code::Forbidden)?;
    let scope_bytes = canonical(scope).map_err(|_| Code::InvalidInput)?;
    let input = ExportInputV1 {
        version: 1,
        scope_hash: digest(b"ai-guard:broker-journal-scope:v1\0", &scope_bytes),
        job_operation_id: operation,
        export_request_id: request,
        destination_normalized: destination.to_str().ok_or(Code::InvalidInput)?.to_owned(),
        parent_device,
        parent_inode,
        target_name,
    };
    let bytes = canonical(&input).map_err(|_| Code::InvalidInput)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(Code::LimitExceeded);
    }
    let input_hash = digest(b"ai-guard:broker-export-target:v1\0", &bytes);
    let inputs = root
        .persistent_child("export-inputs", true)
        .map_err(|_| Code::NeedsReconciliation)?
        .ok_or(Code::NeedsReconciliation)?;
    let directory = inputs
        .persistent_child(scope.profile_id(), true)
        .map_err(|_| Code::NeedsReconciliation)?
        .ok_or(Code::NeedsReconciliation)?;
    // Serialize only the small input-publication namespace, never execution,
    // the runtime, an RPC or the journal. The nonblocking lease makes capacity
    // checks exact even when different jobs export concurrently.
    let _lease = input_lease(&directory)?;
    let name = format!("{request}.json");
    let inventory = inventory(&directory)?;
    let input_file = match directory
        .persistent_read(&name, MAX_INPUT_BYTES, &[])
        .map_err(|_| Code::NeedsReconciliation)?
    {
        Some(existing) => compare_existing(existing, &bytes)?,
        None => {
            if inventory.0 >= MAX_INPUT_FILES
                || inventory
                    .1
                    .checked_add(bytes.len() as u64)
                    .is_none_or(|size| size > MAX_PROFILE_BYTES)
            {
                return Err(Code::LimitExceeded);
            }
            match directory.persistent_create(&name, &bytes) {
                Ok(created) => created,
                // O_EXCL is still the final publication guard. A cooperating
                // writer holds the lease; a same-UID race can only produce an
                // exact replay or a conflict, never replacement/adoption.
                Err(error) if already_exists(&error) => {
                    let existing = directory
                        .persistent_read(&name, MAX_INPUT_BYTES, &[])
                        .map_err(|_| Code::NeedsReconciliation)?
                        .ok_or(Code::NeedsReconciliation)?;
                    compare_existing(existing, &bytes)?
                }
                Err(_) => return Err(Code::NeedsReconciliation),
            }
        }
    };
    // Exact replay also resynchronizes the retained record and every generated
    // parent before returning. An error preserves partial evidence unchanged.
    input_file
        .persistent_sync()
        .map_err(|_| Code::NeedsReconciliation)?;
    directory.sync().map_err(|_| Code::NeedsReconciliation)?;
    inputs.sync().map_err(|_| Code::NeedsReconciliation)?;
    root.sync().map_err(|_| Code::NeedsReconciliation)?;
    parent.revalidate().map_err(|_| Code::Conflict)?;
    root.revalidate().map_err(|_| Code::NeedsReconciliation)?;

    Ok(ValidatedExportTarget {
        request_id: request,
        destination,
        destination_ref: request.to_string(),
        destination_intent_hash: input_hash,
        parent,
    })
}

fn normalized_destination(path: &Path) -> Result<PathBuf, Code> {
    if !path.is_absolute()
        || path.as_os_str().as_bytes().len() > 4096
        || path.components().count() > 64
        || path
            .to_str()
            .is_none_or(|value| value.chars().any(char::is_control))
    {
        return Err(Code::InvalidInput);
    }
    let normalized = absolute_path(path).map_err(|_| Code::InvalidInput)?;
    if normalized.file_name().is_none()
        || normalized.components().count() > 64
        || normalized.as_os_str().as_bytes().len() > 4096
    {
        return Err(Code::InvalidInput);
    }
    Ok(normalized)
}

fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    hex::encode(hash.finalize())
}

fn already_exists(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == ErrorKind::AlreadyExists)
}

fn input_lease(directory: &Directory) -> Result<PrivateFile, Code> {
    let file = match directory
        .persistent_read(LOCK_NAME, LOCK_BYTES.len() as u64, &[])
        .map_err(|_| Code::NeedsReconciliation)?
    {
        Some(file) => file,
        None => match directory.persistent_create(LOCK_NAME, LOCK_BYTES) {
            Ok(file) => file,
            Err(error) if already_exists(&error) => directory
                .persistent_read(LOCK_NAME, LOCK_BYTES.len() as u64, &[])
                .map_err(|_| Code::NeedsReconciliation)?
                .ok_or(Code::NeedsReconciliation)?,
            Err(_) => return Err(Code::NeedsReconciliation),
        },
    };
    if file.bytes() != LOCK_BYTES {
        return Err(Code::NeedsReconciliation);
    }
    if !file
        .persistent_try_lock()
        .map_err(|_| Code::NeedsReconciliation)?
    {
        return Err(Code::Busy);
    }
    file.persistent_sync()
        .map_err(|_| Code::NeedsReconciliation)?;
    directory.sync().map_err(|_| Code::NeedsReconciliation)?;
    Ok(file)
}

fn inventory(directory: &Directory) -> Result<(u32, u64), Code> {
    let entries = directory
        .persistent_entries(MAX_INPUT_FILES + 1)
        .map_err(|_| Code::NeedsReconciliation)?;
    let mut count = 0_u32;
    let mut size = 0_u64;
    for entry in entries {
        if entry.directory {
            return Err(Code::NeedsReconciliation);
        }
        if entry.name == LOCK_NAME {
            if entry.bytes != LOCK_BYTES.len() as u64 {
                return Err(Code::NeedsReconciliation);
            }
        } else {
            let id = entry
                .name
                .strip_suffix(".json")
                .and_then(|value| Uuid::parse_str(value).ok())
                .filter(|id| !id.is_nil())
                .ok_or(Code::NeedsReconciliation)?;
            if entry.name != format!("{id}.json") || entry.bytes > MAX_INPUT_BYTES {
                return Err(Code::NeedsReconciliation);
            }
            count = count.checked_add(1).ok_or(Code::LimitExceeded)?;
        }
        size = size.checked_add(entry.bytes).ok_or(Code::LimitExceeded)?;
    }
    if count > MAX_INPUT_FILES || size > MAX_PROFILE_BYTES {
        return Err(Code::LimitExceeded);
    }
    Ok((count, size))
}

fn compare_existing(file: PrivateFile, expected: &[u8]) -> Result<PrivateFile, Code> {
    let input: ExportInputV1 =
        serde_json::from_slice(file.bytes()).map_err(|_| Code::NeedsReconciliation)?;
    let canonical = canonical(&input).map_err(|_| Code::NeedsReconciliation)?;
    if input.version != 1
        || input.job_operation_id.is_nil()
        || input.export_request_id.is_nil()
        || input.parent_inode == 0
        || canonical != file.bytes()
    {
        return Err(Code::NeedsReconciliation);
    }
    if canonical != expected {
        return Err(Code::Conflict);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::service::jobs::ProfileJobs;
    use crate::runtime::{NetworkMode, RuntimeKind};
    use std::{fs, os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
    use tokio::sync::oneshot;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: Directory,
        profile: ControlProfileV1,
        scope: JournalScopeV1,
        destination: PathBuf,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let base = temp.path().canonicalize().unwrap();
        for name in ["broker", "configuration", "sources", "exports"] {
            fs::create_dir(base.join(name)).unwrap();
            fs::set_permissions(base.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::create_dir(base.join("exports/parent")).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-export-inputs",
            "http://127.0.0.1:8787",
            Uuid::from_u128(0x22222222222242228222222222222222),
            "export_inputs",
        )
        .unwrap();
        let profile = ControlProfileV1 {
            id: scope.profile_id().into(),
            gateway: scope.gateway_origin().into(),
            tenant_id: scope.tenant_id().into(),
            control_namespace_uuid: scope.control_namespace_uuid(),
            config_dir: base.join("configuration"),
            runner_token_file: base.join("configuration/runner.token"),
            runtime: RuntimeKind::Docker,
            runtime_program: PathBuf::from("/usr/bin/false"),
            runtime_endpoint: "unix:///never-used".into(),
            image: "never-used:test".into(),
            policy_profile: "strict".into(),
            network: NetworkMode::None,
            allow_rootful_runtime: false,
            max_timeout_seconds: 30,
            allow_capture_output: false,
            managed_clients: false,
            providers_file: None,
            manual_credential_file: None,
            source_roots: vec![base.join("sources")],
            export_roots: vec![base.join("exports")],
        };
        Fixture {
            root: Directory::open_existing(&base.join("broker")).unwrap(),
            destination: base.join("exports/parent/result"),
            _temp: temp,
            profile,
            scope,
        }
    }

    #[test]
    fn immutable_export_input_replays_exactly_and_rejects_divergence() {
        let fixture = fixture();
        let operation = Uuid::new_v4();
        let request = Uuid::new_v4();
        let first = validate_and_persist(
            &fixture.root,
            &fixture.profile,
            &fixture.scope,
            operation,
            request,
            &fixture.destination,
        )
        .unwrap();
        assert!(!fixture.destination.exists());
        let replay = validate_and_persist(
            &fixture.root,
            &fixture.profile,
            &fixture.scope,
            operation,
            request,
            &fixture.destination,
        )
        .unwrap();
        assert_eq!(
            first.destination_intent_hash(),
            replay.destination_intent_hash()
        );
        assert_eq!(first.destination_ref(), request.to_string());
        assert_eq!(first.request_id(), request);
        let changed = fixture.destination.with_file_name("different");
        assert_eq!(
            validate_and_persist(
                &fixture.root,
                &fixture.profile,
                &fixture.scope,
                operation,
                request,
                &changed,
            )
            .unwrap_err(),
            Code::Conflict
        );
        assert_eq!(
            validate_and_persist(
                &fixture.root,
                &fixture.profile,
                &fixture.scope,
                Uuid::new_v4(),
                request,
                &fixture.destination,
            )
            .unwrap_err(),
            Code::Conflict
        );
    }

    #[test]
    fn replaced_parent_cannot_reconstruct_original_export_capability() {
        let fixture = fixture();
        let operation = Uuid::new_v4();
        let request = Uuid::new_v4();
        let first = validate_and_persist(
            &fixture.root,
            &fixture.profile,
            &fixture.scope,
            operation,
            request,
            &fixture.destination,
        )
        .unwrap();
        let parent = fixture.destination.parent().unwrap();
        fs::rename(parent, parent.with_file_name("retained-original")).unwrap();
        fs::create_dir(parent).unwrap();
        assert!(first.parent().revalidate().is_err());
        assert_eq!(
            validate_and_persist(
                &fixture.root,
                &fixture.profile,
                &fixture.scope,
                operation,
                request,
                &fixture.destination,
            )
            .unwrap_err(),
            Code::Conflict
        );
        assert!(!fixture.destination.exists());
    }

    #[test]
    fn unauthorized_or_partial_input_never_becomes_an_export_grant() {
        let fixture = fixture();
        let operation = Uuid::new_v4();
        let request = Uuid::new_v4();
        let outside = fixture.root.path().join("result");
        assert_eq!(
            validate_and_persist(
                &fixture.root,
                &fixture.profile,
                &fixture.scope,
                operation,
                request,
                &outside,
            )
            .unwrap_err(),
            Code::Forbidden
        );
        assert!(!fixture.root.path().join("export-inputs").exists());
        let directory = fixture
            .root
            .persistent_child("export-inputs", true)
            .unwrap()
            .unwrap()
            .persistent_child(fixture.scope.profile_id(), true)
            .unwrap()
            .unwrap();
        let name = format!("{request}.json");
        let partial = directory
            .persistent_create(&name, b"{\"version\":")
            .unwrap();
        partial.persistent_sync().unwrap();
        directory.sync().unwrap();
        assert_eq!(
            validate_and_persist(
                &fixture.root,
                &fixture.profile,
                &fixture.scope,
                operation,
                request,
                &fixture.destination,
            )
            .unwrap_err(),
            Code::NeedsReconciliation
        );
        assert_eq!(partial.bytes(), b"{\"version\":");
        assert!(!fixture.destination.exists());
    }

    #[test]
    fn immutable_input_reserves_id_without_creating_metadata_for_a_fresh_id() {
        let fixture = fixture();
        let operation = Uuid::new_v4();
        let request = Uuid::new_v4();
        validate_and_persist(
            &fixture.root,
            &fixture.profile,
            &fixture.scope,
            operation,
            request,
            &fixture.destination,
        )
        .unwrap();
        assert!(!fixture.destination.exists());
        assert_eq!(
            reject_reserved_id(&fixture.root, fixture.scope.profile_id(), request),
            Err(Code::Conflict)
        );

        let fresh = Uuid::new_v4();
        assert_eq!(
            reject_reserved_id(&fixture.root, fixture.scope.profile_id(), fresh),
            Ok(())
        );
        assert!(
            !fixture
                .root
                .path()
                .join("export-inputs")
                .join(fixture.scope.profile_id())
                .join(format!("{fresh}.json"))
                .exists()
        );
        assert!(!fixture.destination.exists());
    }

    #[tokio::test]
    async fn cancel_invalidates_then_waits_for_export_identity_publication() {
        let fixture = fixture();
        let operation = Uuid::new_v4();
        let request = Uuid::new_v4();
        let command = Uuid::new_v4();
        let jobs = Arc::new(ProfileJobs::new());
        let job = jobs.job(operation).unwrap();
        let running = job.enter().unwrap();
        let original_generation = running.generation();
        let identity = jobs.identity_gate().await.unwrap();

        let cancel_jobs = Arc::clone(&jobs);
        let cancel_job = Arc::clone(&job);
        let root = fixture.root.clone();
        let profile = fixture.scope.profile_id().to_owned();
        let (started_tx, started_rx) = oneshot::channel();
        let (finished_tx, mut finished_rx) = oneshot::channel();
        let cancel_task = tokio::spawn(async move {
            let cancellation = cancel_job
                .begin_cancel(command)
                .expect("cancel identity begins")
                .expect("fresh cancel identity");
            started_tx.send(cancellation.generation).unwrap();
            let _identity = cancel_jobs.identity_gate().await.unwrap();
            let outcome = reject_reserved_id(&root, &profile, request);
            finished_tx.send(outcome).unwrap();
            drop(cancellation);
        });

        let invalidated_generation = tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .expect("cancel must begin while identity publication is held")
            .expect("cancel task must report its invalidation");
        assert!(invalidated_generation > original_generation);
        assert!(job.cancel.current() > original_generation);
        assert!(matches!(
            finished_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        // This is the real immutable export-input publisher. It reserves the
        // request ID with O_EXCL while cancel is blocked behind identity_gate.
        validate_and_persist(
            &fixture.root,
            &fixture.profile,
            &fixture.scope,
            operation,
            request,
            &fixture.destination,
        )
        .unwrap();
        assert!(!fixture.destination.exists());
        drop(identity);

        let outcome = tokio::time::timeout(Duration::from_secs(3), finished_rx)
            .await
            .expect("cancel must resume after the publisher releases identity_gate")
            .expect("cancel task must report the reserved identity");
        assert_eq!(outcome, Err(Code::Conflict));
        assert_eq!(job.cancel.current(), invalidated_generation);
        assert!(!fixture.destination.exists());
        drop(running);
        cancel_task.await.unwrap();
    }
}
