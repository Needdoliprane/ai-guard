use super::{storage::*, *};
use crate::{
    audit::{
        Store,
        authentication::{AuthenticationAudit, AuthenticationReceiptV1},
    },
    control_fs::Directory,
    identity::{IdentityChangeV1, IdentityMutationV1, IdentityReceiptV1, LegacyControlRoleV1},
};
use std::ffi::OsStr;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseBinding {
    version: u32,
    installation_id: Uuid,
    tenant_id: String,
    identity: NativeIdentity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Namespace {
    version: u32,
    installation_id: Uuid,
    tenant_id: String,
    control_namespace_uuid: String,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AuthenticationOrigin {
    version: u32,
    installation_id: Uuid,
    identity: NativeIdentity,
    genesis: AuthenticationReceiptV1,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantCompletion {
    tenant_id: String,
    control_namespace_uuid: String,
    receipts: Vec<IdentityReceiptV1>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    version: u32,
    installation_id: Uuid,
    input_sha256: String,
    authentication_origin: AuthenticationOrigin,
    tenants: Vec<TenantCompletion>,
}

fn authentication(
    directory: &Directory,
    preparation: &Preparation,
) -> Result<(AuthenticationAudit, AuthenticationOrigin)> {
    let path = directory.path().join("authentication.sqlite");
    let existing_origin: Option<AuthenticationOrigin> =
        read(directory, "authentication-origin.json")?;
    if let Some(origin) = &existing_origin {
        ensure!(
            origin.version == 1
                && origin.installation_id == preparation.installation_id
                && NativeIdentity::entry(directory, "authentication.sqlite")?
                    == Some(origin.identity.clone()),
            "authentication audit origin was substituted"
        );
    }
    let audit = if directory
        .private_entry(OsStr::new("authentication.sqlite"))?
        .is_none()
    {
        ensure!(
            existing_origin.is_none(),
            "retained authentication audit is missing; it must not be recreated"
        );
        AuthenticationAudit::init_new(
            &path,
            preparation.installation_id,
            preparation.manifest.authentication_policy.clone(),
            Redactor::new(vec![]),
        )?
    } else {
        AuthenticationAudit::open_existing(
            &path,
            preparation.installation_id,
            preparation.manifest.authentication_policy.clone(),
            Redactor::new(vec![]),
        )?
    };
    let verified = audit.verify()?;
    let origin = if let Some(origin) = existing_origin {
        ensure!(
            origin.genesis.namespace_id == verified.namespace_id
                && origin.genesis.sequence == 1
                && verified.head.sequence >= origin.genesis.sequence
                && (verified.head.sequence > 1 || verified.head == origin.genesis),
            "authentication audit namespace conflicts with retained origin"
        );
        origin
    } else {
        ensure!(
            verified.records == 1 && verified.head.sequence == 1,
            "authentication audit without its preparation receipt cannot adopt later activity"
        );
        AuthenticationOrigin {
            version: 1,
            installation_id: preparation.installation_id,
            identity: NativeIdentity::entry(directory, "authentication.sqlite")?
                .context("authentication audit disappeared")?,
            genesis: verified.head,
        }
    };
    exact(directory, "authentication-origin.json", &bytes(&origin)?)?;
    Ok((audit, origin))
}

fn open_tenant(
    directory: &Directory,
    preparation: &Preparation,
    tenant: &SetupTenantV1,
    capacity: u64,
) -> Result<Store> {
    let binding: Option<DatabaseBinding> = read(directory, "database-binding.json")?;
    let binding = if let Some(binding) = binding {
        ensure!(
            binding.version == 1
                && binding.installation_id == preparation.installation_id
                && binding.tenant_id == tenant.tenant_id
                && NativeIdentity::entry(directory, "audit.sqlite")?
                    == Some(binding.identity.clone()),
            "tenant database differs from the retained installation binding"
        );
        binding
    } else {
        // Reserve exclusively, then durably bind the empty inode before any
        // SQLite initialization. A missing binding never adopts an old DB.
        let file = directory.create_private_entry(OsStr::new("audit.sqlite"))?;
        file.sync_all()?;
        directory.sync()?;
        drop(file);
        let binding = DatabaseBinding {
            version: 1,
            installation_id: preparation.installation_id,
            tenant_id: tenant.tenant_id.clone(),
            identity: NativeIdentity::entry(directory, "audit.sqlite")?
                .context("new tenant database disappeared")?,
        };
        save(directory, "database-binding.json", &bytes(&binding)?)?;
        binding
    };
    let namespace: Option<Namespace> = read(directory, "namespace.json")?;
    let path = directory.path().join("audit.sqlite");
    let store = if namespace.is_none() {
        Store::open_for_tenant(&path, &tenant.tenant_id, Redactor::new(vec![]), capacity)?
    } else {
        Store::open_existing_for_tenant(&path, &tenant.tenant_id, Redactor::new(vec![]), capacity)?
    };
    ensure!(
        NativeIdentity::entry(directory, "audit.sqlite")? == Some(binding.identity),
        "tenant database inode changed during open"
    );
    let state = store.supervision_state()?;
    let actual_namespace = state
        .control_namespace_uuid
        .context("new tenant control namespace unavailable")?;
    let retained = namespace.unwrap_or(Namespace {
        version: 1,
        installation_id: preparation.installation_id,
        tenant_id: tenant.tenant_id.clone(),
        control_namespace_uuid: actual_namespace.clone(),
    });
    ensure!(
        retained.version == 1
            && retained.installation_id == preparation.installation_id
            && retained.tenant_id == tenant.tenant_id
            && retained.control_namespace_uuid == actual_namespace,
        "tenant namespace differs from retained setup authority"
    );
    exact(directory, "namespace.json", &bytes(&retained)?)?;
    Ok(store)
}

fn migrate(store: &Store, directory: &Directory) -> Result<()> {
    let backups = directory
        .persistent_child("backups", true)?
        .context("new backup directory unavailable")?;
    loop {
        let version = store.supervision_state()?.schema_version;
        if version == 5 {
            return Ok(());
        }
        ensure!(
            (2..5).contains(&version),
            "setup requires a fresh schema2..5 tenant store"
        );
        // A failed migration may already have durably copied its backup.
        // Retain every attempt and select a new exclusive name on retry.
        let mut path = None;
        for attempt in 0..1024 {
            let name = format!("before-v{}-{attempt:04}.sqlite", version + 1);
            if backups.private_entry(OsStr::new(&name))?.is_none() {
                path = Some(backups.path().join(name));
                break;
            }
        }
        let path = path.context(
            "retained migration backup attempts exceed their bound; preserve all evidence",
        )?;
        match version {
            2 => {
                store.migrate_control_v3(&path)?;
            }
            3 => {
                store.migrate_identity_v4(&path)?;
            }
            4 => {
                store.migrate_human_access_v5(&path)?;
            }
            _ => anyhow::bail!("unsupported setup migration"),
        }
    }
}

fn registry(preparation: &Preparation) -> serde_json::Value {
    serde_json::json!({"version":1,"tenants":preparation.manifest.tenants.iter().map(|tenant| {
        let root = preparation.final_path.join("tenants").join(&tenant.tenant_id);
        serde_json::json!({"tenant_id":tenant.tenant_id,"data":root.join("audit.sqlite"),"runner_token_file":root.join("runner"),"viewer_token_file":root.join("viewer"),"worker_quota":tenant.worker_quota})
    }).collect::<Vec<_>>()})
}

pub(super) fn complete(
    directory: &Directory,
    preparation: &Preparation,
    material: &SecretMaterial,
    capacity: u64,
) -> Result<()> {
    let (audit, origin) = authentication(directory, preparation)?;
    let tenants = directory
        .persistent_child("tenants", true)?
        .context("setup tenant directory unavailable")?;
    let mut completed = Vec::new();
    for ((tenant, secrets), operations) in preparation
        .manifest
        .tenants
        .iter()
        .zip(&material.tenants)
        .zip(&preparation.operations)
    {
        let root = tenants
            .persistent_child(&tenant.tenant_id, true)?
            .context("setup tenant unavailable")?;
        exact(&root, "runner", format!("{}\n", secrets.runner).as_bytes())?;
        exact(&root, "viewer", format!("{}\n", secrets.viewer).as_bytes())?;
        let store = open_tenant(&root, preparation, tenant, capacity)?;
        audit.ensure_distinct_from_business(&store)?;
        migrate(&store, &root)?;
        let namespace = store
            .supervision_state()?
            .control_namespace_uuid
            .context("tenant namespace unavailable")?;
        // Authority is the private filesystem operator scoped to this exact
        // control namespace, never an input principal or a human login claim.
        // SAFETY: geteuid only reads this process's effective UID.
        let uid = unsafe { libc::geteuid() };
        let actor = PrincipalRefV1::LegacyControl {
            credential_id: format!("offline-filesystem:uid:{uid}:namespace:{namespace}"),
            role: LegacyControlRoleV1::Runner,
        };
        let mut receipts = Vec::new();
        for (identity, ids) in tenant.identities.iter().zip(&operations.identities) {
            for (operation_id, change) in [
                (
                    ids.account,
                    IdentityChangeV1::Account {
                        expected_revision: 0,
                        account: HumanAccountV1 {
                            version: 1,
                            principal: identity.principal.clone(),
                            display_name: identity.display_name.clone(),
                            revision: 1,
                            active: true,
                        },
                    },
                ),
                (
                    ids.membership,
                    IdentityChangeV1::Membership {
                        expected_revision: 0,
                        membership: MembershipV1 {
                            version: 1,
                            membership_id: ids.membership_id.to_string(),
                            principal: identity.principal.clone(),
                            tenant_id: tenant.tenant_id.clone(),
                            organization_scope: tenant.organization_id.clone(),
                            revision: 1,
                            active: true,
                            capabilities: identity.capabilities.iter().copied().collect(),
                        },
                    },
                ),
            ] {
                receipts.push(store.mutate_identity(&IdentityMutationV1 {
                    version: 1,
                    operation_id: operation_id.to_string(),
                    actor: actor.clone(),
                    change,
                })?);
            }
        }
        ensure!(
            store.integrity()?.valid,
            "prepared tenant integrity verification failed"
        );
        completed.push(TenantCompletion {
            tenant_id: tenant.tenant_id.clone(),
            control_namespace_uuid: namespace,
            receipts,
        });
        drop(store);
        root.sync()?;
    }
    exact(directory, "tenants.json", &bytes(&registry(preparation))?)?;
    if let Some(pem) = &preparation.manifest.ca_bundle_pem {
        exact(directory, "ca-bundle.pem", pem.as_bytes())?;
    }
    exact(
        directory,
        "oidc.json",
        &bytes(&serde_json::json!({
            "version":1,"gateway":preparation.manifest.gateway,
            "authentication_audit":{"path":preparation.final_path.join("authentication.sqlite"),"installation_id":preparation.installation_id,"policy":preparation.manifest.authentication_policy},
            "ca_bundle_file":preparation.manifest.ca_bundle_pem.as_ref().map(|_| preparation.final_path.join("ca-bundle.pem"))
        }))?,
    )?;
    exact(
        directory,
        "completion.json",
        &bytes(&Completion {
            version: 1,
            installation_id: preparation.installation_id,
            input_sha256: preparation.input_sha256.clone(),
            authentication_origin: origin,
            tenants: completed,
        })?,
    )?;
    drop(audit);
    Ok(())
}
