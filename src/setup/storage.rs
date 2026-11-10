use super::*;
use crate::{
    control_fs::{Directory, PrivateFile, publication::PublicationFailure},
    identity::generate_local_credential,
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::Metadata,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[cfg(test)]
thread_local! { pub(super) static FAIL_AFTER_INTENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeIdentity {
    device: u64,
    inode: u64,
    uid: u32,
}
impl NativeIdentity {
    fn from_metadata(facts: &Metadata) -> Self {
        Self {
            device: facts.dev(),
            inode: facts.ino(),
            uid: facts.uid(),
        }
    }
    pub(super) fn directory(directory: &Directory) -> Result<Self> {
        Ok(Self::from_metadata(
            &directory.try_clone_file()?.metadata()?,
        ))
    }
    pub(super) fn entry(directory: &Directory, name: &str) -> Result<Option<Self>> {
        let before = directory.private_entry(OsStr::new(name))?;
        if before.is_none() {
            return Ok(None);
        }
        // Metadata only: never open/close a second raw SQLite descriptor while
        // a Store owns POSIX locks for this inode.
        let facts = std::fs::symlink_metadata(directory.path().join(name))?;
        ensure!(
            directory.private_entry(OsStr::new(name))? == before,
            "setup entry changed during inspection"
        );
        Ok(Some(Self::from_metadata(&facts)))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdentityOperations {
    pub account: Uuid,
    pub membership: Uuid,
    pub membership_id: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TenantOperations {
    pub tenant_id: String,
    pub identities: Vec<IdentityOperations>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preparation {
    pub version: u32,
    pub installation_id: Uuid,
    pub input_sha256: String,
    pub manifest: SetupManifest,
    pub final_path: PathBuf,
    pub stage_path: PathBuf,
    pub directory: NativeIdentity,
    pub parent: NativeIdentity,
    pub secret_material: NativeIdentity,
    pub secret_material_sha256: String,
    pub operations: Vec<TenantOperations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication_namespace_id: Option<Uuid>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TenantSecrets {
    pub tenant_id: String,
    pub runner: String,
    pub viewer: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SecretMaterial {
    pub version: u32,
    pub installation_id: Uuid,
    pub tenants: Vec<TenantSecrets>,
}

pub(super) fn bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value)?;
    ensure!(
        bytes.len() as u64 <= MAX_RECOVERY,
        "setup recovery record exceeds its bound"
    );
    Ok(bytes)
}
pub(super) fn save(directory: &Directory, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
    let file = directory.persistent_create(name, bytes)?;
    file.persistent_sync()?;
    directory.sync()?;
    file.persistent_revalidate()?;
    Ok(file)
}
pub(super) fn exact(directory: &Directory, name: &str, bytes: &[u8]) -> Result<()> {
    if let Some(file) = directory.persistent_read(name, MAX_RECOVERY, &[])? {
        ensure!(
            file.bytes() == bytes,
            "retained setup record conflicts; preserve all material for diagnosis"
        );
        file.persistent_sync()?;
        directory.sync()?;
    } else {
        save(directory, name, bytes)?;
    }
    Ok(())
}
pub(super) fn read<T: for<'de> Deserialize<'de>>(
    directory: &Directory,
    name: &str,
) -> Result<Option<T>> {
    directory
        .persistent_read(name, MAX_RECOVERY, &[])?
        .map(|file| {
            serde_json::from_slice(file.bytes()).map_err(|_| {
                anyhow::anyhow!("invalid retained setup record; preserve it for diagnosis")
            })
        })
        .transpose()
}
fn destination(path: &Path) -> Result<(Directory, String)> {
    let path = absolute_path(path)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("setup destination requires an ASCII name")?;
    ensure!(
        !leaf.is_empty()
            && leaf.len() <= 96
            && leaf != "."
            && leaf != ".."
            && leaf
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "setup destination requires 1..96 ASCII letters, digits, dots, underscores or hyphens"
    );
    let parent = Directory::open_existing(
        path.parent()
            .context("setup requires an existing private parent")?,
    )?;
    Ok((parent, leaf.to_owned()))
}
fn stage_name(final_path: &Path) -> String {
    format!(
        ".setup-{}",
        &hex::encode(Sha256::digest(final_path.as_os_str().as_encoded_bytes()))[..24]
    )
}

pub(super) fn prepare(manifest: impl Into<SetupManifest>, path: &Path) -> Result<PathBuf> {
    let manifest = manifest.into();
    // No directory, lock, secret or database may precede this pure preflight.
    manifest.validate()?;
    let (parent, leaf) = destination(path)?;
    parent.require_absent_child(&leaf)?;
    let final_path = parent.path().join(&leaf);
    let stage = parent.exclusive_child(&stage_name(&final_path))?;
    let result = (|| {
        let installation_id = Uuid::new_v4();
        let material = SecretMaterial {
            version: 1,
            installation_id,
            tenants: manifest
                .tenants
                .iter()
                .map(|tenant| -> Result<_> {
                    Ok(TenantSecrets {
                        tenant_id: tenant.tenant_id.clone(),
                        runner: generate_local_credential()?,
                        viewer: generate_local_credential()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };
        // This is retained recovery material, never a runtime credential file.
        // It is durable before the intent binds it, and is never regenerated.
        // A crash before a complete intent is retained is diagnostic-only.
        let secrets = save(&stage, "secret-material.json", &bytes(&material)?)?;
        let preparation = Preparation {
            version: manifest.version,
            installation_id,
            input_sha256: hex::encode(Sha256::digest(serde_json::to_vec(&manifest)?)),
            final_path: final_path.clone(),
            stage_path: stage.path().to_owned(),
            directory: NativeIdentity::directory(&stage)?,
            parent: NativeIdentity::directory(&parent)?,
            secret_material: NativeIdentity::entry(&stage, "secret-material.json")?
                .context("setup secret material unavailable")?,
            secret_material_sha256: secrets.digest().to_owned(),
            operations: manifest
                .tenants
                .iter()
                .map(|tenant| TenantOperations {
                    tenant_id: tenant.tenant_id.clone(),
                    identities: tenant
                        .identities
                        .iter()
                        .map(|_| IdentityOperations {
                            account: Uuid::new_v4(),
                            membership: Uuid::new_v4(),
                            membership_id: Uuid::new_v4(),
                        })
                        .collect(),
                })
                .collect(),
            authentication_namespace_id: (manifest.version == 2).then(Uuid::new_v4),
            manifest,
        };
        let intent = save(&stage, "preparation.json", &bytes(&preparation)?)?;
        ensure!(intent.persistent_try_lock()?, "setup preparation is in use");
        stage.durable_bootstrap()?;
        #[cfg(test)]
        if FAIL_AFTER_INTENT.with(|flag| flag.replace(false)) {
            anyhow::bail!("injected failure after real durable setup intent");
        }
        finish(
            &parent,
            &stage,
            &intent,
            &preparation,
            &material,
            MAX_EVENTS,
        )
    })();
    result.with_context(|| format!("setup material retained at {} or, after publication, {}; do not overwrite or regenerate it; stop controls and use setup resume --recovery-dir <retained directory> --confirm-control-stopped", stage.path().display(), final_path.display()))
}

pub(super) fn resume(path: &Path) -> Result<PathBuf> {
    let stage = Directory::open_existing(&absolute_path(path)?)?;
    let intent = stage.persistent_read("preparation.json",MAX_RECOVERY,&[])?.context("complete setup intent unavailable; preserve the incomplete preparation and use a different new destination")?;
    ensure!(
        intent.persistent_try_lock()?,
        "setup preparation is in use; retry after the other command exits"
    );
    let preparation: Preparation = serde_json::from_slice(intent.bytes())
        .map_err(|_| anyhow::anyhow!("invalid retained setup intent"))?;
    let (parent, _) = destination(&preparation.final_path)?;
    let secret_file = stage
        .persistent_read("secret-material.json", MAX_RECOVERY, &[])?
        .context(
            "retained setup secret material unavailable; credentials must not be regenerated",
        )?;
    ensure!(
        secret_file.digest() == preparation.secret_material_sha256
            && NativeIdentity::entry(&stage, "secret-material.json")?
                == Some(preparation.secret_material.clone()),
        "retained setup secret material was substituted"
    );
    let material: SecretMaterial = serde_json::from_slice(secret_file.bytes())
        .map_err(|_| anyhow::anyhow!("invalid retained setup secret material"))?;
    secret_file.persistent_sync()?;
    finish(&parent,&stage,&intent,&preparation,&material,MAX_EVENTS).context("setup recovery remains unresolved; preserve the exact retained files and retry after resolving the reported error")
}

fn validate(
    parent: &Directory,
    directory: &Directory,
    preparation: &Preparation,
    material: &SecretMaterial,
) -> Result<()> {
    preparation.manifest.validate()?;
    ensure!(
        matches!(
            (
                preparation.version,
                preparation.manifest.version,
                preparation.authentication_namespace_id
            ),
            (1, 1, None) | (2, 2, Some(_))
        ) && preparation
            .authentication_namespace_id
            .is_none_or(|id| !id.is_nil())
            && !preparation.installation_id.is_nil()
            && material.version == 1
            && material.installation_id == preparation.installation_id
            && preparation.input_sha256
                == hex::encode(Sha256::digest(serde_json::to_vec(&preparation.manifest)?)),
        "retained setup identities or manifest fingerprint conflict"
    );
    let (expected_parent, leaf) = destination(&preparation.final_path)?;
    ensure!(
        expected_parent.path() == parent.path()
            && preparation.final_path == parent.path().join(&leaf)
            && preparation.stage_path == parent.path().join(stage_name(&preparation.final_path))
            && (directory.path() == preparation.stage_path
                || directory.path() == preparation.final_path)
            && preparation.directory == NativeIdentity::directory(directory)?
            && preparation.parent == NativeIdentity::directory(parent)?,
        "retained setup paths, ownership or directory identities conflict"
    );
    ensure!(
        material.tenants.len() == preparation.manifest.tenants.len()
            && preparation.operations.len() == material.tenants.len(),
        "retained setup tenant count conflicts"
    );
    let mut operations = BTreeSet::new();
    let mut credentials = BTreeSet::new();
    for ((tenant, secrets), ops) in preparation
        .manifest
        .tenants
        .iter()
        .zip(&material.tenants)
        .zip(&preparation.operations)
    {
        ensure!(
            tenant.tenant_id == secrets.tenant_id
                && tenant.tenant_id == ops.tenant_id
                && tenant.identities.len() == ops.identities.len(),
            "retained setup tenant or identity order conflicts"
        );
        for token in [&secrets.runner, &secrets.viewer] {
            ensure!(
                token.len() == 70
                    && token.starts_with("aigh1_")
                    && token[6..]
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && credentials.insert(hex::encode(Sha256::digest(token.as_bytes()))),
                "invalid or duplicate retained technical credential"
            );
        }
        for ids in &ops.identities {
            for id in [ids.account, ids.membership, ids.membership_id] {
                ensure!(
                    !id.is_nil() && operations.insert(id),
                    "invalid or duplicate retained operation identity"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn finish(
    parent: &Directory,
    directory: &Directory,
    intent: &PrivateFile,
    preparation: &Preparation,
    material: &SecretMaterial,
    capacity: u64,
) -> Result<PathBuf> {
    validate(parent, directory, preparation, material)?;
    intent.persistent_sync()?;
    directory.durable_bootstrap()?;
    let leaf = preparation
        .final_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("retained final name unavailable")?;
    let already_published = directory.path() == preparation.final_path;
    if already_published {
        ensure!(
            parent
                .persistent_child(&stage_name(&preparation.final_path), false)?
                .is_none(),
            "both staging and final setup directories exist; preserve both for diagnosis"
        );
    } else {
        parent.require_absent_child(leaf)?;
    }
    installation::complete(directory, preparation, material, capacity)?;
    // All SQLite owners have dropped before filesystem synchronization or
    // rename. Never open another raw DB descriptor while SQLite holds locks.
    sync_tree(directory)?;
    intent.persistent_revalidate()?;
    if !already_published {
        match parent.publish_child_exclusive(directory, leaf) {
            Ok(()) => {}
            Err(PublicationFailure::NotPublished) => anyhow::bail!(
                "setup directory publication failed; resume the retained staging directory"
            ),
            Err(PublicationFailure::DurabilityUnknown) => anyhow::bail!(
                "setup was published but parent durability is unconfirmed; preserve the final directory and resume it with controls stopped"
            ),
        }
    }
    let published = Directory::open_existing(&preparation.final_path)?;
    ensure!(
        NativeIdentity::directory(&published)? == preparation.directory,
        "published setup directory identity changed"
    );
    let published_intent = published
        .persistent_read("preparation.json", MAX_RECOVERY, &[])?
        .context("published setup intent unavailable")?;
    ensure!(
        published_intent.bytes() == intent.bytes(),
        "published setup intent differs"
    );
    published.durable_bootstrap()?;
    Ok(preparation.final_path.clone())
}

fn sync_tree(directory: &Directory) -> Result<()> {
    for entry in directory.persistent_entries(4096)? {
        if entry.directory {
            let child = directory
                .persistent_child(&entry.name, false)?
                .context("setup child disappeared")?;
            sync_tree(&child)?;
        } else {
            directory.sync_closed_private_entry(&entry.name)?;
        }
    }
    directory.sync()
}
