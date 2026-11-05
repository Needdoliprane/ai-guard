use super::{ApplyArgs, OfflineArgs, ProvisionArgs, ResumeProvisionArgs};
use crate::{
    audit::Store,
    control_fs::{Directory, PrivateFile, absolute_path},
    identity::*,
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Write, os::unix::fs::MetadataExt, path::Path};
use uuid::Uuid;

const MAX_RECOVERY: u64 = 262_144;
const MAX_CREDENTIAL: u64 = 128;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    version: u32,
    data_path: std::path::PathBuf,
    tenant_id: String,
    control_namespace_uuid: String,
    output_path: std::path::PathBuf,
    output_device: u64,
    output_inode: u64,
    provision: LocalHumanProvisionV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    version: u32,
    receipts: Vec<IdentityReceiptV1>,
}

/// This input deliberately cannot carry an actor. Possession of a JSON file
/// does not authenticate an actor, even when it names a valid principal.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorMutation {
    version: u32,
    operation_id: String,
    change: IdentityChangeV1,
}

struct OfflineStore {
    store: Store,
    data_path: std::path::PathBuf,
    namespace: String,
    actor: PrincipalRefV1,
}

fn open(args: &OfflineArgs) -> Result<OfflineStore> {
    ensure!(
        args.confirm_control_stopped,
        "offline identity maintenance requires --confirm-control-stopped"
    );
    let capacity_policy = args
        .audit_capacity_file
        .as_deref()
        .map(Store::load_capacity_policy)
        .transpose()?;
    if let Some(policy) = &capacity_policy {
        policy.validate(args.max_events)?;
    }
    let data_path = absolute_path(&args.data)?;
    let store = Store::open_existing_for_tenant(
        &data_path,
        &args.tenant_id,
        Redactor::new(vec![]),
        args.max_events,
    )?;
    let store = if let Some(policy) = capacity_policy {
        store.with_capacity_policy(policy)?
    } else {
        store
    };
    eprintln!(
        "Audit capacity tenant={} source={} human_read_policy={} policy={} fingerprint={}",
        store.tenant_id(),
        if args.audit_capacity_file.is_some() {
            "explicit_operator_policy"
        } else {
            "derived_defaults"
        },
        if store.capacity_policy().human_reads_enabled() {
            "enabled"
        } else {
            "human_reads_disabled_insufficient_capacity"
        },
        serde_json::to_string(store.capacity_policy())?,
        store.capacity_policy_fingerprint()?,
    );
    let state = store.supervision_state()?;
    ensure!(
        matches!(state.schema_version, 4..=9) && !state.active,
        "offline identity maintenance requires an existing validated schema4, schema5, schema6, schema7, schema8 or schema9 database and inactive supervision; migrate explicitly with controls stopped"
    );
    let namespace = state
        .control_namespace_uuid
        .context("existing control namespace is unavailable")?;
    // SAFETY: geteuid has no preconditions and observes this process only.
    let uid = unsafe { libc::geteuid() };
    let actor = PrincipalRefV1::LegacyControl {
        credential_id: format!("offline-filesystem:uid:{uid}:namespace:{namespace}"),
        role: LegacyControlRoleV1::Runner,
    };
    actor.validate()?;
    Ok(OfflineStore {
        store,
        data_path,
        namespace,
        actor,
    })
}

/// The output parent must already be private. The final ASCII name is limited
/// because persistent control namespaces deliberately use a closed alphabet.
pub(super) fn destination(path: &Path) -> Result<(Directory, String)> {
    let path = absolute_path(path)?;
    let leaf = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("output destination requires an ASCII directory name")?;
    ensure!(
        !leaf.is_empty()
            && leaf.len() <= 96
            && leaf != "."
            && leaf != ".."
            && leaf
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "output directory name must contain 1..96 ASCII letters, digits, dots, underscores or hyphens"
    );
    let parent = Directory::open_existing(
        path.parent()
            .context("output destination requires an existing private parent")?,
    )?;
    Ok((parent, leaf.to_owned()))
}

pub(super) fn save(directory: &Directory, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
    let file = directory.persistent_create(name, bytes)?;
    file.persistent_sync()?;
    directory.sync()?;
    file.persistent_revalidate()?;
    Ok(file)
}

fn preparation(
    args: &ProvisionArgs,
    control: &OfflineStore,
    output: &Directory,
    secret: &str,
) -> Result<Recovery> {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: control.namespace.clone(),
        account_id: Uuid::new_v4().to_string(),
    };
    let changes = [
        IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: args.name.clone(),
                revision: 1,
                active: true,
            },
        },
        IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: LocalCredentialV1 {
                version: 1,
                credential_id: Uuid::new_v4().to_string(),
                principal: principal.clone(),
                sha256_digest: hex::encode(Sha256::digest(secret.as_bytes())),
                revision: 1,
                active: true,
                expires_at: None,
            },
        },
        IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal,
                tenant_id: args.offline.tenant_id.clone(),
                organization_scope: args.organization.clone(),
                revision: 1,
                active: true,
                capabilities: args
                    .capabilities
                    .iter()
                    .copied()
                    .map(Into::into)
                    .collect::<BTreeSet<_>>(),
            },
        },
    ];
    let provision = LocalHumanProvisionV1 {
        version: 1,
        mutations: changes.map(|change| IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: control.actor.clone(),
            change,
        }),
    };
    provision.validate()?;
    let facts = output.try_clone_file()?.metadata()?;
    Ok(Recovery {
        version: 1,
        data_path: control.data_path.clone(),
        tenant_id: args.offline.tenant_id.clone(),
        control_namespace_uuid: control.namespace.clone(),
        output_path: output.path().to_owned(),
        output_device: facts.dev(),
        output_inode: facts.ino(),
        provision,
    })
}

pub(super) fn provision(args: ProvisionArgs) -> Result<()> {
    let control = open(&args.offline)?;
    ensure!(
        !args
            .capabilities
            .iter()
            .any(|cap| matches!(cap, super::Capability::ReadBrokerRegistry))
            || matches!(control.store.supervision_state()?.schema_version, 7 | 8 | 9),
        "read-broker-registry requires explicit migration6→7 before preparing a local identity"
    );
    let secret = generate_local_credential()
        .map_err(|_| anyhow::anyhow!("credential generation failed; no identity was activated"))?;
    let (parent, leaf) = destination(&args.output_dir)?;
    let output = parent.exclusive_child(&leaf)?;
    let result = (|| -> Result<()> {
        let recovery = preparation(&args, &control, &output, &secret)?;
        let bytes = serde_json::to_vec_pretty(&recovery)?;
        ensure!(
            bytes.len() as u64 <= MAX_RECOVERY,
            "identity preparation exceeds its bound"
        );
        let credential = save(&output, "credential", format!("{secret}\n").as_bytes())?;
        let recovery_file = save(&output, "recovery.json", &bytes)?;
        ensure!(
            recovery_file.persistent_try_lock()?,
            "identity preparation is already in use"
        );
        finish(&control, &output, &credential, &recovery_file, &recovery)
    })();
    result.with_context(|| format!(
        "identity preparation retained at {}; do not delete, overwrite or rotate it; when credential and recovery.json are intact, run human resume-provision with the same database, tenant and output directory; otherwise preserve this incomplete preparation and use a new destination",
        output.path().display()
    ))
}

pub(super) fn resume(args: ResumeProvisionArgs) -> Result<()> {
    let control = open(&args.offline)?;
    let output_path = absolute_path(&args.output_dir)?;
    let output = Directory::open_existing(&output_path)?;
    let recovery_file = output
        .persistent_read("recovery.json", MAX_RECOVERY, &[])?
        .context(
            "recovery.json is missing; retain the incomplete preparation and use a new destination",
        )?;
    ensure!(
        recovery_file.persistent_try_lock()?,
        "identity preparation is in use; retry after the other command exits"
    );
    let credential = output
        .persistent_read("credential", MAX_CREDENTIAL, &[])?
        .context("private credential file is missing; retain this preparation, do not regenerate a credential")?;
    let recovery: Recovery = serde_json::from_slice(recovery_file.bytes()).map_err(|_| {
        anyhow::anyhow!("recovery.json is invalid; preserve the preparation for diagnosis")
    })?;
    finish(&control, &output, &credential, &recovery_file, &recovery).with_context(|| format!(
        "identity completion is unresolved; preparation retained at {}; retry human resume-provision with the exact same files after resolving the reported error",
        output.path().display()
    ))
}

fn verify_preparation(
    control: &OfflineStore,
    output: &Directory,
    credential: &PrivateFile,
    recovery: &Recovery,
) -> Result<()> {
    recovery.provision.validate()?;
    let facts = output.try_clone_file()?.metadata()?;
    ensure!(
        recovery.version == 1
            && recovery.data_path == control.data_path
            && recovery.tenant_id == control.store.tenant_id()
            && recovery.control_namespace_uuid == control.namespace
            && recovery.output_path == output.path()
            && recovery.output_device == facts.dev()
            && recovery.output_inode == facts.ino()
            && recovery
                .provision
                .mutations
                .iter()
                .all(|m| m.actor == control.actor),
        "retained preparation does not match this database, tenant, namespace and filesystem operator"
    );
    let secret = super::http::credential_text(credential.bytes())?;
    let digest = hex::encode(Sha256::digest(secret.as_bytes()));
    let IdentityChangeV1::Credential { binding, .. } = &recovery.provision.mutations[1].change
    else {
        anyhow::bail!("retained preparation lacks its exact credential binding");
    };
    let PrincipalRefV1::HumanLocal {
        installation_id, ..
    } = &binding.principal
    else {
        anyhow::bail!("retained preparation is not a local principal");
    };
    let IdentityChangeV1::Membership { membership, .. } = &recovery.provision.mutations[2].change
    else {
        anyhow::bail!("retained preparation lacks its exact membership");
    };
    ensure!(
        binding.sha256_digest == digest
            && membership.tenant_id == control.store.tenant_id()
            && installation_id == &control.namespace,
        "retained credential digest or tenant differs; no credential is adopted or regenerated"
    );
    Ok(())
}

fn finish(
    control: &OfflineStore,
    output: &Directory,
    credential: &PrivateFile,
    recovery_file: &PrivateFile,
    recovery: &Recovery,
) -> Result<()> {
    verify_preparation(control, output, credential, recovery)?;
    // These barriers also make a post-crash preparation durable before replay.
    credential.persistent_sync()?;
    recovery_file.persistent_sync()?;
    output.durable_bootstrap()?;
    credential.persistent_revalidate()?;
    recovery_file.persistent_revalidate()?;
    let receipts = control.store.provision_local_human(&recovery.provision)?;
    let completion = Completion {
        version: 1,
        receipts,
    };
    let bytes = serde_json::to_vec_pretty(&completion)?;
    if let Some(existing) = output.persistent_read("completion.json", MAX_RECOVERY, &[])? {
        let previous: Completion = serde_json::from_slice(existing.bytes())
            .map_err(|_| anyhow::anyhow!("completion receipt is invalid; exact committed operations were replayed, preserve the retained preparation"))?;
        ensure!(
            previous.version == 1 && previous.receipts == completion.receipts,
            "completion receipt conflicts with the exact database replay; preserve all evidence"
        );
        existing.persistent_sync()?;
        output.sync()?;
    } else {
        save(output, "completion.json", &bytes).context(
            "account transaction committed but completion receipt publication is unresolved; resume the exact preparation to recover the same receipts",
        )?;
    }
    credential.persistent_revalidate()?;
    recovery_file.persistent_revalidate()?;
    let IdentityChangeV1::Account { account, .. } = &recovery.provision.mutations[0].change else {
        anyhow::bail!("invalid prepared account");
    };
    print_json(&serde_json::json!({
        "version": 1,
        "status": "provisioned",
        "principal": account.principal,
        "tenant_id": control.store.tenant_id(),
        "credential_file": output.path().join("credential"),
        "recovery_file": output.path().join("recovery.json"),
        "completion_file": output.path().join("completion.json"),
        "operator_authentication": "private_control_filesystem_effective_uid",
        "supervision_active": false
    }))
}

pub(super) fn apply(args: ApplyArgs) -> Result<()> {
    let control = open(&args.offline)?;
    let path = absolute_path(&args.mutation_file)?;
    let input_parent =
        Directory::open_existing(path.parent().context("mutation file parent missing")?)?;
    let data_parent = Directory::open_existing(
        control
            .data_path
            .parent()
            .context("database parent missing")?,
    )?;
    let input_identity = input_parent
        .private_entry(path.file_name().context("mutation filename missing")?)?
        .context("mutation file is missing")?;
    let data_identity = data_parent
        .private_entry(
            control
                .data_path
                .file_name()
                .context("database filename missing")?,
        )?
        .context("database disappeared")?;
    ensure!(
        input_identity != data_identity,
        "mutation input must not be the database inode"
    );
    let data_name = control
        .data_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("database filename is unsupported")?;
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure!(
            data_parent.private_entry(std::ffi::OsStr::new(&format!("{data_name}{suffix}")))?
                != Some(input_identity),
            "mutation input must not be a SQLite sidecar inode"
        );
    }
    let file = PrivateFile::open(&path, 65_536)?;
    let input: OperatorMutation = serde_json::from_slice(file.bytes()).map_err(|_| {
        anyhow::anyhow!("invalid mutation file; expected version, operation_id and typed change only; actor is derived from the private control filesystem")
    })?;
    let mutation = IdentityMutationV1 {
        version: input.version,
        operation_id: input.operation_id,
        actor: control.actor,
        change: input.change,
    };
    mutation.validate()?;
    file.revalidate()?;
    let receipt = control.store.mutate_identity(&mutation)?;
    // No credential digest or mutation content is returned to the terminal.
    print_json(&serde_json::json!({
        "version": 1,
        "status": "applied",
        "operation_id": receipt.operation_id,
        "operator_authentication": "private_control_filesystem_effective_uid",
        "supervision_active": false
    }))
}

pub(super) fn print_json(value: &serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .context("human CLI output unavailable")
}
