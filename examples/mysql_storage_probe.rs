//! Operator-side read-only probe for MySQL transport and a prepared audit target.
//! The `owned_origin_preflight` receipt confirms the expected origin under the
//! schema lock and successful release and disconnect before returning.
use agentsandbox::{
    control_fs::{PrivateFile, absolute_path},
    diagnostics::{
        DEFAULT_MAX_EVENTS,
        gateway_files::{
            MySqlAuthenticationFiles, MySqlStorageFiles, SelectedTenantStorage,
            TenantRegistryFiles, decode_tenant_registry, read_tenant_registry_file,
            select_tenant_storage,
        },
    },
    mysql_storage::{
        MAX_CERTIFICATE_BYTES, MAX_PASSWORD_BYTES, MAX_PRIVATE_KEY_BYTES, MysqlAuthentication,
        MysqlClientIdentity, MysqlCredentials, MysqlEndpoint,
        audit::{AuditHead, MysqlAuditStore},
        preflight_owned_preparation, probe,
    },
    redaction::Redactor,
    tenancy::validate_tenant_id,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    str,
    time::Duration,
};
use uuid::Uuid;

const USAGE: &str = "usage: mysql_storage_probe <private JSON file> | --registry <file> --tenant <id> [--verify-control-projections | --verify-owned-origin <server-uuid> <head-seq> <head-hash> | --preflight-owned-prepare <server-uuid> <14|15>]";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeFile {
    host: String,
    port: u16,
    database: String,
    username: String,
    password_file: PathBuf,
    ca_file: PathBuf,
    authentication: AuthenticationFiles,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum AuthenticationFiles {
    Password {},
    PasswordAndMtls {
        certificate_chain_file: PathBuf,
        private_key_file: PathBuf,
    },
}

enum Invocation {
    Transport(PathBuf),
    Registry {
        path: PathBuf,
        tenant: String,
        verification: RegistryVerification,
    },
}

enum RegistryVerification {
    Binding,
    ControlProjections,
    OwnedOrigin {
        server: Uuid,
        head_seq: i64,
        head_hash: String,
    },
    OwnedPreparation {
        server: Uuid,
        schema_version: u16,
    },
}

struct Binding<'a> {
    tenant: &'a str,
    namespace: Uuid,
    verification: RegistryVerification,
}

#[derive(Serialize)]
struct BoundTargetReceipt {
    configured_authentication: MysqlAuthentication,
    binding_verified: bool,
    audit_head: AuditHead,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_projections_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_mode: Option<&'static str>,
}

fn material(directory: &Path, file: &Path, max: usize) -> Result<PrivateFile> {
    ensure!(
        !file.as_os_str().is_empty(),
        "empty MySQL material reference"
    );
    PrivateFile::open(&directory.join(file), max as u64)
        .map_err(|_| anyhow!("cannot read a private MySQL material reference"))
}

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| anyhow!("cannot start the MySQL probe runtime"))?;
    let result = runtime.block_on(run());
    // A failed DNS lookup can leave a blocking resolver task in Tokio.
    runtime.shutdown_timeout(Duration::from_millis(250));
    result
}

async fn run() -> Result<()> {
    match parse_invocation(std::env::args_os().skip(1))? {
        Invocation::Transport(path) => check_transport(&path).await,
        Invocation::Registry {
            path,
            tenant,
            verification,
        } => check_registry_target(&path, &tenant, verification).await,
    }
}

fn parse_invocation(mut arguments: impl Iterator<Item = OsString>) -> Result<Invocation> {
    let first = arguments.next().context(USAGE)?;
    let Some(path) = arguments.next() else {
        return Ok(Invocation::Transport(first.into()));
    };
    ensure!(first.as_os_str() == "--registry", "{USAGE}");
    let flag = arguments.next().context(USAGE)?;
    ensure!(flag.as_os_str() == "--tenant", "{USAGE}");
    let tenant = arguments.next().context(USAGE)?;
    let verification = match arguments.next() {
        None => RegistryVerification::Binding,
        Some(flag) if flag.as_os_str() == "--verify-control-projections" => {
            RegistryVerification::ControlProjections
        }
        Some(flag) if flag.as_os_str() == "--verify-owned-origin" => {
            let mut next = || {
                arguments
                    .next()
                    .context(USAGE)?
                    .into_string()
                    .map_err(|_| anyhow!("{USAGE}"))
            };
            let server = Uuid::parse_str(&next()?).map_err(|_| anyhow!("{USAGE}"))?;
            let head_seq: i64 = next()?.parse().map_err(|_| anyhow!("{USAGE}"))?;
            let head_hash = next()?;
            ensure!(
                !server.is_nil()
                    && head_seq > 0
                    && head_hash.len() == 64
                    && head_hash
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
                "{USAGE}"
            );
            RegistryVerification::OwnedOrigin {
                server,
                head_seq,
                head_hash,
            }
        }
        Some(flag) if flag.as_os_str() == "--preflight-owned-prepare" => {
            let server_text = arguments
                .next()
                .context(USAGE)?
                .into_string()
                .map_err(|_| anyhow!("{USAGE}"))?;
            let server = Uuid::parse_str(&server_text).map_err(|_| anyhow!("{USAGE}"))?;
            ensure!(
                !server.is_nil() && server.to_string() == server_text,
                "{USAGE}"
            );
            let format = arguments.next().context(USAGE)?;
            let schema_version = match format.to_str() {
                Some("14") => 14,
                Some("15") => 15,
                _ => bail!("{USAGE}"),
            };
            RegistryVerification::OwnedPreparation {
                server,
                schema_version,
            }
        }
        Some(_) => bail!("{USAGE}"),
    };
    ensure!(arguments.next().is_none(), "{USAGE}");
    let tenant = tenant.into_string().map_err(|_| anyhow!("{USAGE}"))?;
    validate_tenant_id(&tenant)?;
    Ok(Invocation::Registry {
        path: path.into(),
        tenant,
        verification,
    })
}

async fn check_transport(file: &Path) -> Result<()> {
    let path = absolute_path(file).map_err(|_| anyhow!("invalid probe file path"))?;
    let directory = path
        .parent()
        .context("probe file has no parent directory")?;
    let config_file = material(directory, &path, 8_192)?;
    let config: ProbeFile = serde_json::from_slice(config_file.bytes())
        .map_err(|_| anyhow!("invalid MySQL storage probe file"))?;
    let client_files = match &config.authentication {
        AuthenticationFiles::Password {} => None,
        AuthenticationFiles::PasswordAndMtls {
            certificate_chain_file,
            private_key_file,
        } => Some((certificate_chain_file.as_path(), private_key_file.as_path())),
    };
    check_target(
        directory,
        MysqlEndpoint {
            host: &config.host,
            port: config.port,
            database: &config.database,
            username: &config.username,
        },
        &config.password_file,
        &config.ca_file,
        client_files,
        None,
    )
    .await
}

fn select_mysql_target<'a>(
    registry: &'a TenantRegistryFiles,
    requested: &str,
) -> Result<&'a MySqlStorageFiles> {
    let mut seen = BTreeSet::new();
    let mut selected = None;
    for tenant in &registry.tenants {
        validate_tenant_id(&tenant.tenant_id)?;
        ensure!(
            seen.insert(tenant.tenant_id.as_str()),
            "duplicate tenant identity in the registry"
        );
        let storage = select_tenant_storage(tenant.data.as_deref(), tenant.storage.as_ref())?;
        if tenant.tenant_id == requested {
            selected = Some(storage);
        }
    }
    match selected.context("requested tenant is absent from the registry")? {
        SelectedTenantStorage::MySql(profile) => Ok(profile),
        SelectedTenantStorage::Sqlite(_) => bail!("requested tenant does not select MySQL"),
    }
}

async fn check_registry_target(
    path: &Path,
    tenant: &str,
    verification: RegistryVerification,
) -> Result<()> {
    let registry = decode_tenant_registry(&read_tenant_registry_file(path)?)?;
    let profile = select_mysql_target(&registry, tenant)?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .context("registry directory is unavailable")?;
    let client_files = match &profile.authentication {
        MySqlAuthenticationFiles::Password {} => None,
        MySqlAuthenticationFiles::PasswordAndMtls {
            client_cert_file,
            client_key_file,
        } => Some((client_cert_file.as_path(), client_key_file.as_path())),
    };
    check_target(
        &directory,
        MysqlEndpoint {
            host: &profile.host,
            port: profile.port.get(),
            database: &profile.database,
            username: &profile.username,
        },
        &profile.password_file,
        &profile.ca_file,
        client_files,
        Some(Binding {
            tenant,
            namespace: profile.namespace,
            verification,
        }),
    )
    .await
}

async fn check_target(
    directory: &Path,
    endpoint: MysqlEndpoint<'_>,
    password_file: &Path,
    ca_file: &Path,
    client_files: Option<(&Path, &Path)>,
    binding: Option<Binding<'_>>,
) -> Result<()> {
    let password_file = material(directory, password_file, MAX_PASSWORD_BYTES + 2)?;
    let password = str::from_utf8(password_file.bytes())
        .map_err(|_| anyhow!("invalid MySQL password file"))?;
    let password = password
        .strip_suffix("\r\n")
        .or_else(|| password.strip_suffix('\n'))
        .unwrap_or(password);
    let ca = material(directory, ca_file, MAX_CERTIFICATE_BYTES)?;
    let identity_files = match client_files {
        None => None,
        Some((certificate_chain_file, private_key_file)) => Some((
            material(directory, certificate_chain_file, MAX_CERTIFICATE_BYTES)?,
            material(directory, private_key_file, MAX_PRIVATE_KEY_BYTES)?,
        )),
    };
    let credentials = MysqlCredentials {
        password,
        ca_pem: ca.bytes(),
        client_identity: identity_files
            .as_ref()
            .map(|(cert, key)| MysqlClientIdentity {
                certificate_chain_pem: cert.bytes(),
                private_key_pem: key.bytes(),
            }),
    };
    let receipt = match binding {
        Some(binding) => {
            let configured_authentication = if client_files.is_some() {
                MysqlAuthentication::PasswordAndMtls
            } else {
                MysqlAuthentication::Password
            };
            // The constructor takes the defaults for mutations. verify uses a
            // read-only transaction without applying the mutation capacity.
            let bound_store = |endpoint, credentials| {
                MysqlAuditStore::new(
                    endpoint,
                    credentials,
                    binding.tenant,
                    binding.namespace,
                    Redactor::new(Vec::new()),
                    DEFAULT_MAX_EVENTS as i64,
                )
            };
            let (audit_head, control_projections_verified, verification_mode) = match binding
                .verification
            {
                RegistryVerification::Binding => (
                    bound_store(endpoint, credentials)?.verify().await?,
                    None,
                    None,
                ),
                RegistryVerification::ControlProjections => (
                    bound_store(endpoint, credentials)?
                        .verify_control_projections()
                        .await?,
                    Some(true),
                    None,
                ),
                RegistryVerification::OwnedOrigin {
                    server,
                    head_seq,
                    head_hash,
                } => (
                    bound_store(endpoint, credentials)?
                        .verify_owned_origin(server, head_seq, &head_hash)
                        .await?,
                    Some(true),
                    Some("owned_origin_preflight"),
                ),
                RegistryVerification::OwnedPreparation {
                    server,
                    schema_version,
                } => {
                    let receipt =
                        preflight_owned_preparation(endpoint, credentials, server, schema_version)
                            .await?;
                    println!("{}", serde_json::to_string(&receipt)?);
                    return Ok(());
                }
            };
            serde_json::to_string(&BoundTargetReceipt {
                configured_authentication,
                binding_verified: true,
                audit_head,
                control_projections_verified,
                verification_mode,
            })?
        }
        None => serde_json::to_string(&probe(endpoint, credentials).await?)?,
    };
    println!("{receipt}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn registry() -> Value {
        json!({
            "version": 1,
            "tenants": [
                {
                    "tenant_id": "a",
                    "data": "a.sqlite",
                    "runner_token_file": "unused-a-runner",
                    "viewer_token_file": "unused-a-viewer"
                },
                {
                    "tenant_id": "b",
                    "runner_token_file": "unused-b-runner",
                    "viewer_token_file": "unused-b-viewer",
                    "storage": {
                        "backend": "mysql",
                        "host": "db.example.invalid",
                        "port": 13306,
                        "database": "audit_b",
                        "username": "reader_b",
                        "namespace": "5657019d-5302-4ced-922f-406aa3d8ec5c",
                        "password_file": "sql/password",
                        "ca_file": "/recipe/ca.pem",
                        "authentication": {"mode": "password_and_mtls", "client_cert_file": "sql/client.pem", "client_key_file": "/recipe/client.key"}
                    }
                }
            ]
        })
    }

    #[test]
    fn accepts_transport_path_or_explicit_registry_tenant() -> Result<()> {
        let parse = |args: &[&str]| parse_invocation(args.iter().map(|arg| OsString::from(*arg)));
        for filename in ["legacy.json", "--registry"] {
            assert!(matches!(
                parse(&[filename])?,
                Invocation::Transport(path) if path.as_path() == Path::new(filename)
            ));
        }
        assert!(matches!(
            parse(&["--registry", "tenants.json", "--tenant", "b"])?,
            Invocation::Registry { path, tenant, verification: RegistryVerification::Binding }
                if path.as_path() == Path::new("tenants.json") && tenant == "b"
        ));
        assert!(matches!(
            parse(&["--registry", "tenants.json", "--tenant", "b", "--verify-control-projections"])?,
            Invocation::Registry { path, tenant, verification: RegistryVerification::ControlProjections }
                if path.as_path() == Path::new("tenants.json") && tenant == "b"
        ));
        let server_uuid = "01234567-89ab-4def-8012-3456789abcde";
        let head = "ab".repeat(32);
        assert!(matches!(
            parse(&["--registry", "tenants.json", "--tenant", "b", "--verify-owned-origin", server_uuid, "3", &head])?,
            Invocation::Registry {
                path, tenant, verification: RegistryVerification::OwnedOrigin { server, head_seq: 3, head_hash }
            } if path.as_path() == Path::new("tenants.json") && tenant == "b" && server.to_string() == server_uuid && head_hash == head
        ));
        for args in [
            &[][..],
            &["--registry", "tenants.json"],
            &["--registry", "tenants.json", "--tenant", "b", "legacy.json"],
            &["--registry", "tenants.json", "--tenant", "../b"],
            &["legacy.json", "--verify-control-projections"],
            &[
                "--registry",
                "tenants.json",
                "--verify-control-projections",
                "--tenant",
                "b",
            ],
            &[
                "--registry",
                "tenants.json",
                "--tenant",
                "b",
                "--verify-control-projections",
                "--verify-control-projections",
            ],
            &[
                "--registry",
                "tenants.json",
                "--tenant",
                "b",
                "--verify-owned-origin",
                server_uuid,
                "3",
            ],
            &[
                "--registry",
                "tenants.json",
                "--tenant",
                "b",
                "--verify-owned-origin",
                server_uuid,
                "0",
                &head,
            ],
        ] {
            assert!(parse(args).is_err());
        }
        Ok(())
    }

    #[test]
    fn owned_preparation_preflight_requires_an_explicit_uuid_and_format() -> Result<()> {
        let server = "01234567-89ab-4def-8012-3456789abcde";
        let parse = |server: &str, version: &str| {
            parse_invocation(
                [
                    "--registry",
                    "tenants.json",
                    "--tenant",
                    "b",
                    "--preflight-owned-prepare",
                    server,
                    version,
                ]
                .into_iter()
                .map(OsString::from),
            )
        };
        for format in [14_u16, 15] {
            assert!(matches!(
                parse(server, &format.to_string())?,
                Invocation::Registry {
                    tenant,
                    verification: RegistryVerification::OwnedPreparation { server: observed, schema_version },
                    ..
                } if tenant == "b" && observed.to_string() == server && schema_version == format
            ));
        }
        assert!(parse(server, "1").is_err());
        assert!(parse(server, "16").is_err());
        assert!(parse("00000000-0000-0000-0000-000000000000", "14").is_err());
        Ok(())
    }

    #[test]
    fn only_owned_origin_preflight_labels_the_receipt() -> Result<()> {
        let mut receipt = BoundTargetReceipt {
            configured_authentication: MysqlAuthentication::Password,
            binding_verified: true,
            audit_head: AuditHead {
                seq: 1,
                hash: "ab".repeat(32),
                payload_bytes: 128,
            },
            control_projections_verified: Some(true),
            verification_mode: None,
        };
        let mut expected = json!({
            "configured_authentication": "password",
            "binding_verified": true,
            "audit_head": receipt.audit_head,
            "control_projections_verified": true,
        });
        assert_eq!(serde_json::to_value(&receipt)?, expected);
        receipt.verification_mode = Some("owned_origin_preflight");
        expected["verification_mode"] = json!("owned_origin_preflight");
        assert_eq!(serde_json::to_value(&receipt)?, expected);
        Ok(())
    }

    #[test]
    fn selects_exact_mysql_profile_without_opening_material() -> Result<()> {
        let mut fixture = registry();
        let decoded = decode_tenant_registry(&serde_json::to_vec(&fixture)?)?;
        let selected = select_mysql_target(&decoded, "b")?;
        assert_eq!(selected.host, "db.example.invalid");
        assert_eq!(selected.port.get(), 13306);
        assert_eq!(selected.database, "audit_b");
        assert_eq!(selected.username, "reader_b");
        assert_eq!(
            selected.namespace,
            Uuid::parse_str("5657019d-5302-4ced-922f-406aa3d8ec5c")?
        );
        assert_eq!(selected.password_file.as_path(), Path::new("sql/password"));
        assert_eq!(selected.ca_file.as_path(), Path::new("/recipe/ca.pem"));
        assert!(matches!(
            &selected.authentication,
            MySqlAuthenticationFiles::PasswordAndMtls { client_cert_file, client_key_file }
                if client_cert_file.as_path() == Path::new("sql/client.pem")
                    && client_key_file.as_path() == Path::new("/recipe/client.key")
        ));
        assert!(select_mysql_target(&decoded, "a").is_err());
        assert!(select_mysql_target(&decoded, "missing").is_err());

        fixture["tenants"][1]["storage"]["authentication"] = json!({"mode": "password"});
        let decoded = decode_tenant_registry(&serde_json::to_vec(&fixture)?)?;
        assert!(matches!(
            &select_mysql_target(&decoded, "b")?.authentication,
            MySqlAuthenticationFiles::Password {}
        ));
        fixture["tenants"][0]["tenant_id"] = json!("b");
        let decoded = decode_tenant_registry(&serde_json::to_vec(&fixture)?)?;
        assert!(select_mysql_target(&decoded, "b").is_err());

        fixture["tenants"][0]["tenant_id"] = json!("a");
        fixture["tenants"][1]["data"] = json!("ambiguous.sqlite");
        let decoded = decode_tenant_registry(&serde_json::to_vec(&fixture)?)?;
        assert!(select_mysql_target(&decoded, "b").is_err());
        Ok(())
    }
}
