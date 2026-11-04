use agentsandbox::{
    api::{self, GatewayState, TenantGateway},
    audit::Store,
    control_fs::{Directory, absolute_path},
    jev::{
        JevBackend, JevClient, JevTrust, RuleCatalog, SelectionSource,
        validate_credential_separation, validate_transport_configuration,
    },
    operations::{self, OperatorCommand},
    policy::Policy,
    redaction::Redactor,
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use futures_util::FutureExt;
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io::Read,
    net::SocketAddr,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

mod broker_cli;
mod gateway_config;

#[derive(Parser)]
#[command(
    name = "agentsandbox",
    version,
    about = "Sandbox Docker/Podman et supervision locale — Rust + TypeScript"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Explicit durable local broker, jobs and controlled exports.
    Broker(broker_cli::BrokerArgs),
    /// Named local accounts, authorized human reads and filtered audit export.
    Human(agentsandbox::identity_cli::HumanArgs),
    /// Prepare or recover a new private local OIDC installation offline.
    Setup(agentsandbox::setup::SetupArgs),
    /// Inspect an explicitly configured Kubernetes authority without admitting workers.
    Kubernetes(agentsandbox::kubernetes_cli::KubernetesArgs),
    #[command(name = "__broker-enforcer", hide = true)]
    InternalBrokerEnforcer {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        max_slots: u32,
        #[arg(long)]
        recovery_only: bool,
    },
    /// Collecteur d'audit et moteur de décisions ; normalement lancé dans un conteneur.
    Gateway(Box<GatewayArgs>),
    /// Serveur de l'IHM et proxy strictement en lecture seule.
    Dashboard(DashboardArgs),
    /// Relais de confiance sans secrets vers les deux services internes.
    Ingress(IngressArgs),
    /// Isolated model TLS transport; fixed model:8080 upstream, no control credentials.
    ModelIngress(ModelIngressArgs),
    /// Provision local TLS volumes explicitly, without starting model/gateway.
    ModelTls(agentsandbox::model_tls::ModelTlsArgs),
    #[command(name = "__model-tls-import", hide = true)]
    InternalModelTlsImport,
    /// Migrate an offline audit database, preserving a verified new backup.
    MigrateControl {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        backup: PathBuf,
        /// Exact next schema version: 2 (control), 3 (supervision), 4 (identity), 5 (human read audit), 6 (workloads), 7 (human broker registry reads).
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(2..=7))]
        target_version: u32,
        /// Operator attestation: gateway and brokers are stopped; not runtime fencing.
        #[arg(long, required = true)]
        confirm_control_stopped: bool,
        #[arg(long, default_value_t = 100_000)]
        max_events: u64,
    },
    /// Sonde de santé sans secret, utilisable par le runtime.
    Healthcheck {
        #[arg(long, required = true)]
        url: Vec<String>,
        /// Explicit public CA bundle, valid only when every destination is HTTPS.
        #[arg(long)]
        ca_file: Option<PathBuf>,
    },
    #[command(flatten)]
    Operator(OperatorCommand),
}
#[derive(Args)]
struct GatewayArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,
    #[arg(long)]
    container_listen: bool,
    /// Versioned registry of tenant database and credential file references.
    #[arg(long, conflicts_with_all = ["data", "runner_token_file", "viewer_token_file", "worker_limit", "worker_cpu_millis", "worker_memory_bytes", "worker_max_seconds"])]
    tenants_file: Option<PathBuf>,
    /// Private, versioned audit budgets for the local tenant; never resets usage.
    #[arg(long, conflicts_with = "tenants_file")]
    audit_capacity_file: Option<PathBuf>,
    /// Private workload audit budgets; preserve control/identity reserves and cumulative usage.
    #[arg(long, conflicts_with = "tenants_file")]
    workload_audit_file: Option<PathBuf>,
    /// Explicit private OIDC gateway configuration; absent means no SSO/IdP calls.
    #[arg(long)]
    oidc_config: Option<PathBuf>,
    /// Initialize an exclusively new operator-private authentication audit.
    #[arg(long, requires = "oidc_config")]
    initialize_oidc_audit: bool,
    #[arg(long, default_value = ".agentsandbox/audit.sqlite")]
    data: PathBuf,
    #[arg(long, default_value = ".agentsandbox/secrets/runner")]
    runner_token_file: PathBuf,
    #[arg(long, default_value = ".agentsandbox/secrets/viewer")]
    viewer_token_file: PathBuf,
    #[arg(long, default_value = "policies/default.json")]
    policy: PathBuf,
    #[arg(long, default_value_t = 100_000)]
    max_events: u64,
    /// Create only new supervised stores, with private verified schema2 backups.
    #[arg(long, value_name = "BACKUP_ROOT")]
    initialize_supervised_stores: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    worker_limit: u32,
    #[arg(long, default_value_t = 6000)]
    worker_cpu_millis: u32,
    #[arg(long, default_value_t = 3 * 1024 * 1024 * 1024)]
    worker_memory_bytes: u64,
    #[arg(long, default_value_t = 3600)]
    worker_max_seconds: u32,
    /// Decision veto: typesafe, rust-rules or local-openai. JEV_BACKEND is the fallback.
    #[arg(long)]
    jev_backend: Option<String>,
    #[arg(long, env = "JEV_URL")]
    jev_url: Option<String>,
    #[arg(long, env = "JEV_API_KEY_FILE")]
    jev_key_file: Option<PathBuf>,
    #[arg(long, env = "JEV_MODEL")]
    jev_model: Option<String>,
    /// TypeSafe confidence threshold (default 0.85); invalid for other backends.
    #[arg(long)]
    jev_min_confidence: Option<f64>,
    #[arg(long)]
    jev_allow_local_http: bool,
    /// Local compatible OpenAI chat URL; HTTP requires explicit loopback permission.
    #[arg(long)]
    jev_local_url: Option<String>,
    #[arg(long)]
    jev_local_model: Option<String>,
    /// Optional local inference credential, separate from every control role.
    #[arg(long)]
    jev_local_key_file: Option<PathBuf>,
    /// Version 1 exact argv catalogue in an existing private file; otherwise built-in.
    #[arg(long)]
    jev_rules_catalog: Option<PathBuf>,
    /// Explicit public CA bundle; replaces embedded roots for this backend only.
    #[arg(long, env = "JEV_CA_FILE", hide_env_values = true)]
    jev_ca_file: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum SelectedJev {
    Disabled,
    TypeSafe(SelectionSource),
    RustRules(SelectionSource),
    Local(SelectionSource),
}

/// Pure selection validation precedes opening any credential or making any request.
fn select_jev(args: &GatewayArgs, environment: Option<&str>) -> Result<SelectedJev> {
    let selection = args
        .jev_backend
        .as_deref()
        .map(|value| (value, SelectionSource::Cli))
        .or_else(|| environment.map(|value| (value, SelectionSource::Environment)));
    let typesafe = args.jev_url.is_some()
        || args.jev_key_file.is_some()
        || args.jev_model.is_some()
        || args.jev_min_confidence.is_some();
    let local = args.jev_local_url.is_some()
        || args.jev_local_model.is_some()
        || args.jev_local_key_file.is_some();
    let selected = match selection {
        Some(("typesafe", source)) => SelectedJev::TypeSafe(source),
        Some(("rust-rules", source)) => SelectedJev::RustRules(source),
        Some(("local-openai", source)) => SelectedJev::Local(source),
        Some(_) => anyhow::bail!("Invalid Jev backend selection"),
        None if args.jev_url.is_some() => SelectedJev::TypeSafe(SelectionSource::Legacy),
        None => SelectedJev::Disabled,
    };
    let valid = match selected {
        SelectedJev::Disabled => {
            !typesafe
                && !local
                && args.jev_rules_catalog.is_none()
                && !args.jev_allow_local_http
                && args.jev_ca_file.is_none()
        }
        SelectedJev::TypeSafe(_) => {
            !local
                && args.jev_rules_catalog.is_none()
                && args.jev_url.is_some()
                && args.jev_key_file.is_some()
                && args.jev_model.is_some()
                && args
                    .jev_min_confidence
                    .is_none_or(|value| (0.0..=1.0).contains(&value))
        }
        SelectedJev::RustRules(_) => {
            !typesafe && !local && !args.jev_allow_local_http && args.jev_ca_file.is_none()
        }
        SelectedJev::Local(_) => {
            !typesafe
                && args.jev_rules_catalog.is_none()
                && args.jev_local_url.is_some()
                && args.jev_local_model.is_some()
        }
    };
    anyhow::ensure!(
        valid,
        "Incomplete or mixed Jev backend configuration; use only the selected backend's options"
    );
    Ok(selected)
}

/// No credential file, tenant registry, audit database or network is opened here.
fn preflight_jev(args: &GatewayArgs, selected: SelectedJev) -> Result<JevTrust> {
    let trust = args
        .jev_ca_file
        .as_deref()
        .map(JevTrust::from_ca_file)
        .transpose()?
        .unwrap_or_default();
    let configuration = match selected {
        SelectedJev::TypeSafe(_) => Some((args.jev_url.as_deref(), args.jev_model.as_deref())),
        SelectedJev::Local(_) => Some((
            args.jev_local_url.as_deref(),
            args.jev_local_model.as_deref(),
        )),
        SelectedJev::Disabled | SelectedJev::RustRules(_) => None,
    };
    if let Some((endpoint, model)) = configuration {
        let model = model.context("Jev requested model required")?;
        anyhow::ensure!(
            !model.trim().is_empty() && model.len() <= 128 && !model.chars().any(char::is_control),
            "Jev configuration is invalid"
        );
        validate_transport_configuration(
            endpoint.context("Jev URL required")?,
            args.jev_allow_local_http,
            &trust,
        )?;
    }
    Ok(trust)
}
#[derive(Args)]
struct DashboardArgs {
    #[arg(long, default_value = "127.0.0.1:4000")]
    listen: SocketAddr,
    #[arg(long)]
    container_listen: bool,
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    upstream: String,
    /// Accept viewer identities of all tenants configured by the gateway.
    #[arg(long, conflicts_with = "viewer_token_file")]
    shared_tenants: bool,
    /// Restrict presentation to this viewer; defaults to .agentsandbox/secrets/viewer.
    #[arg(long)]
    viewer_token_file: Option<PathBuf>,
    #[arg(long, default_value = "front/dist")]
    front_dir: PathBuf,
    /// Enable only the fixed authentication proxy for this exact public origin.
    #[arg(long)]
    oidc_web_origin: Option<String>,
}

#[derive(Args)]
struct IngressArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    gateway_listen: SocketAddr,
    #[arg(long, default_value = "127.0.0.1:4000")]
    dashboard_listen: SocketAddr,
    #[arg(long)]
    container_listen: bool,
    #[arg(long, default_value = "http://gateway:8787")]
    gateway_upstream: String,
    #[arg(long, default_value = "http://dashboard:4000")]
    dashboard_upstream: String,
}
#[derive(Args)]
struct ModelIngressArgs {
    #[arg(long, default_value = "127.0.0.1:8443")]
    listen: SocketAddr,
    #[arg(long)]
    container_listen: bool,
    #[arg(long)]
    tls_cert_file: PathBuf,
    #[arg(long)]
    tls_key_file: PathBuf,
}
fn read_token(path: &Path) -> Result<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Impossible de lire le fichier de secret configuré")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "Le secret doit être un fichier régulier"
    );
    let mut token = String::new();
    file.take(1025).read_to_string(&mut token)?;
    anyhow::ensure!(token.len() <= 1024, "Fichier de secret trop volumineux");
    let token = token.trim().to_string();
    anyhow::ensure!(
        token.len() >= 32 && token.len() <= 512 && !token.chars().any(char::is_whitespace),
        "Secret invalide : 32 à 512 caractères sans espace requis"
    );
    Ok(token)
}
fn validate_bind(addr: SocketAddr, container: bool) -> Result<()> {
    anyhow::ensure!(
        addr.ip().is_loopback() || container,
        "Écoute non locale refusée ; --container-listen est réservé aux services conteneurisés dont le port hôte est limité à 127.0.0.1"
    );
    Ok(())
}
fn shutdown_signal() -> Result<impl Future<Output = ()> + Send + 'static> {
    #[cfg(unix)]
    {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .context("Installation du gestionnaire SIGINT impossible")?;
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("Installation du gestionnaire SIGTERM impossible")?;
        Ok(async move {
            tokio::select! {_ = interrupt.recv()=>{},_ = term.recv()=>{}}
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}

/// Bound draining after a signal; without a signal the server has no deadline.
/// This helper belongs to the binary: after it returns, main exits and Tokio's
/// runtime teardown terminates any connection tasks still holding stream bodies.
async fn serve_http(
    listener: tokio::net::TcpListener,
    router: axum::Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let shutdown = shutdown.shared();
    let server_shutdown = shutdown.clone();
    let serving = async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_shutdown)
            .await
    };
    tokio::pin!(serving);
    tokio::select! {
        result = &mut serving => { result?; }
        _ = shutdown => {
            if let Ok(result) = tokio::time::timeout(std::time::Duration::from_secs(5), &mut serving).await {
                result?;
            } else {
                eprintln!("Délai de drainage HTTP de 5 secondes atteint ; arrêt du service.");
            }
        }
    }
    Ok(())
}

fn prepare_supervised_backups(root: &Path, credential_files: &[&Path]) -> Result<Directory> {
    let mut credential_directories = Vec::with_capacity(credential_files.len());
    for credential_file in credential_files {
        let path = absolute_path(credential_file)
            .context("supervised backup credential reference is unsafe")?;
        let parent = path
            .parent()
            .context("supervised backup credential reference has no parent")?;
        credential_directories.push(
            Directory::open_existing(parent)
                .context("supervised backup requires private credential directories")?,
        );
    }
    let root = Directory::open_or_create(&absolute_path(root)?)
        .context("supervised backup root is missing or unsafe")?;
    let identity = |path: &Path| -> Result<(u64, u64)> {
        let metadata = std::fs::metadata(path)
            .context("supervised backup credential directory identity is unavailable")?;
        anyhow::ensure!(
            metadata.is_dir(),
            "supervised backup credential directory identity is invalid"
        );
        Ok((metadata.dev(), metadata.ino()))
    };
    root.revalidate()?;
    let root_identity = identity(root.path())?;
    for directory in credential_directories {
        directory.revalidate()?;
        let credential_identity = identity(directory.path())?;
        // Compare directory identities along both validated chains. Lexical
        // paths alone miss ordinary aliases on case-insensitive filesystems.
        for ancestor in root.path().ancestors() {
            anyhow::ensure!(
                identity(ancestor)? != credential_identity,
                "supervised backup root overlaps a configured credential directory"
            );
        }
        for ancestor in directory.path().ancestors() {
            anyhow::ensure!(
                identity(ancestor)? != root_identity,
                "supervised backup root overlaps a configured credential directory"
            );
        }
        directory.revalidate()?;
        root.revalidate()?;
    }
    Ok(root)
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Broker(args) => broker_cli::execute(args).await?,
        Command::Human(args) => agentsandbox::identity_cli::execute(args).await?,
        Command::Setup(args) => agentsandbox::setup::execute(args)?,
        Command::Kubernetes(args) => agentsandbox::kubernetes_cli::execute(args).await?,
        Command::ModelTls(args) => {
            agentsandbox::model_tls::execute(args, shutdown_signal()?).await?
        }
        Command::InternalModelTlsImport => agentsandbox::model_tls::internal_import()
            .await
            .map_err(|_| anyhow::anyhow!("model TLS import refused"))?,
        Command::Gateway(args) => {
            let args = *args;
            validate_bind(args.listen, args.container_listen)?;
            let backend_environment = if args.jev_backend.is_none() {
                match std::env::var("JEV_BACKEND") {
                    Ok(value) => Some(value),
                    Err(std::env::VarError::NotPresent) => None,
                    Err(_) => anyhow::bail!("Invalid Jev backend selection"),
                }
            } else {
                None
            };
            let selected_jev = select_jev(&args, backend_environment.as_deref())?;
            let jev_trust = preflight_jev(&args, selected_jev)?;
            let tenants = if let Some(path) = &args.tenants_file {
                gateway_config::load(path)?
            } else {
                vec![gateway_config::ConfiguredTenant {
                    tenant_id: "local".into(),
                    data: args.data,
                    runner_token_file: args.runner_token_file.clone(),
                    viewer_token_file: args.viewer_token_file.clone(),
                    runner_token: read_token(&args.runner_token_file)?,
                    viewer_token: read_token(&args.viewer_token_file)?,
                    worker_quota: agentsandbox::control::QuotaPolicyV1 {
                        version: 1,
                        max_active_workers: args.worker_limit,
                        max_cpu_millis: args.worker_cpu_millis,
                        max_memory_bytes: args.worker_memory_bytes,
                        max_timeout_seconds: args.worker_max_seconds,
                    },
                    audit_capacity_policy: args
                        .audit_capacity_file
                        .as_deref()
                        .map(Store::load_capacity_policy)
                        .transpose()?,
                    workload_audit_policy: args
                        .workload_audit_file
                        .as_deref()
                        .map(Store::load_workload_audit_policy)
                        .transpose()?,
                }]
            };
            let mut secrets: Vec<String> = tenants
                .iter()
                .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
                .collect();
            let jev = match selected_jev {
                SelectedJev::Disabled => None,
                SelectedJev::TypeSafe(source) => {
                    let key = read_token(
                        args.jev_key_file
                            .as_deref()
                            .context("TypeSafe key file required")?,
                    )?;
                    validate_credential_separation(&key, secrets.iter().map(String::as_str))?;
                    secrets.push(key.clone());
                    Some(JevBackend::typesafe(
                        JevClient::new_with_trust(
                            args.jev_url.as_deref().context("TypeSafe URL required")?,
                            key,
                            args.jev_model
                                .clone()
                                .context("TypeSafe requested model required")?,
                            args.jev_min_confidence.unwrap_or(0.85),
                            args.jev_allow_local_http,
                            jev_trust.clone(),
                        )?,
                        source,
                    ))
                }
                SelectedJev::RustRules(source) => {
                    let catalogue = args
                        .jev_rules_catalog
                        .as_deref()
                        .map(RuleCatalog::from_private_file)
                        .transpose()?
                        .unwrap_or_default();
                    Some(JevBackend::rust_rules(catalogue, source))
                }
                SelectedJev::Local(source) => {
                    let key = args
                        .jev_local_key_file
                        .as_deref()
                        .map(read_token)
                        .transpose()?;
                    if let Some(key) = &key {
                        validate_credential_separation(key, secrets.iter().map(String::as_str))?;
                        secrets.push(key.clone());
                    }
                    Some(JevBackend::local_openai_with_trust(
                        args.jev_local_url
                            .as_deref()
                            .context("Local Jev URL required")?,
                        key,
                        args.jev_local_model
                            .clone()
                            .context("Local requested model required")?,
                        args.jev_allow_local_http,
                        source,
                        jev_trust.clone(),
                    )?)
                }
            };
            let redactor = Redactor::new(vec![]).union(&Redactor::new(secrets));
            if let Some(backend) = &jev {
                backend.validate_redaction(&redactor)?;
            }
            let policy: Policy = serde_json::from_slice(
                &std::fs::read(&args.policy).context("Politique locale manquante")?,
            )
            .context("Politique locale invalide")?;
            // Validate the complete registry before preparing backups or any
            // fresh origin: a later tenant must not strand an earlier one.
            gateway_config::validate_policies(&tenants, args.max_events)?;
            let backup_root = if let Some(root) = args.initialize_supervised_stores.as_deref() {
                let mut credential_files: Vec<&Path> = tenants
                    .iter()
                    .flat_map(|tenant| {
                        [
                            tenant.runner_token_file.as_path(),
                            tenant.viewer_token_file.as_path(),
                        ]
                    })
                    .collect();
                if let Some(path) = args.jev_key_file.as_deref() {
                    credential_files.push(path);
                }
                if let Some(path) = args.jev_local_key_file.as_deref() {
                    credential_files.push(path);
                }
                if let Some(path) = args.jev_rules_catalog.as_deref() {
                    credential_files.push(path);
                }
                Some(prepare_supervised_backups(root, &credential_files)?)
            } else {
                None
            };
            let mut gateways = Vec::new();
            let mut quotas = Vec::new();
            let mut event_count = 0_u64;
            let mut capacity_diagnostics = Vec::with_capacity(tenants.len());
            for tenant in tenants {
                quotas.push((tenant.tenant_id.clone(), tenant.worker_quota));
                let store = if let Some(root) = &backup_root {
                    root.revalidate()?;
                    let name = format!(
                        "tenant-{}-v2.sqlite",
                        hex::encode(Sha256::digest(tenant.tenant_id.as_bytes()))
                    );
                    Store::create_supervised_for_tenant(
                        &tenant.data,
                        &tenant.tenant_id,
                        redactor.clone(),
                        args.max_events,
                        &root.path().join(name),
                    )?
                } else {
                    Store::open_for_tenant(
                        &tenant.data,
                        &tenant.tenant_id,
                        redactor.clone(),
                        args.max_events,
                    )?
                };
                let capacity_source = if tenant.audit_capacity_policy.is_some() {
                    "explicit_operator_policy"
                } else {
                    "derived_defaults"
                };
                let store = if let Some(policy) = tenant.audit_capacity_policy {
                    store.with_capacity_policy(policy)?
                } else {
                    store
                };
                let workload_source = if tenant.workload_audit_policy.is_some() {
                    if args.tenants_file.is_some() {
                        "tenant_registry"
                    } else {
                        "private_policy_file"
                    }
                } else {
                    "derived_defaults"
                };
                let store = if let Some(policy) = tenant.workload_audit_policy {
                    store.with_workload_audit_policy(policy)?
                } else {
                    store
                };
                capacity_diagnostics.push(redactor.text(&format!(
                    "Audit capacity tenant={} source={} human_read_policy={} policy={} fingerprint={}",
                    store.tenant_id(),
                    capacity_source,
                    if store.capacity_policy().human_reads_enabled() {
                        "enabled"
                    } else {
                        "human_reads_disabled_insufficient_capacity"
                    },
                    serde_json::to_string(store.capacity_policy())?,
                    store.capacity_policy_fingerprint()?,
                )));
                capacity_diagnostics.push(redactor.text(&format!(
                    "Workload audit capacity tenant={} source={} admission_budget={} policy={} fingerprint={}",
                    store.tenant_id(),workload_source,
                    if store.workload_audit_policy().enabled() { "enabled" } else { "workload_audit_disabled_insufficient_capacity" },
                    serde_json::to_string(store.workload_audit_policy())?,store.workload_audit_policy_fingerprint()?,
                )));
                let store = Arc::new(store);
                let check = store.integrity()?;
                anyhow::ensure!(
                    check.valid,
                    "Audit altéré : le collecteur refuse de démarrer"
                );
                event_count = event_count
                    .checked_add(check.event_count)
                    .context("Nombre d'événements hors limite")?;
                gateways.push(TenantGateway {
                    tenant_id: tenant.tenant_id,
                    store,
                    runner_token: tenant.runner_token,
                    viewer_token: tenant.viewer_token,
                });
            }
            let tenant_count = gateways.len();
            let oidc = args
                .oidc_config
                .as_ref()
                .map(|path| {
                    gateway_config::load_oidc(path, args.initialize_oidc_audit, redactor.clone())
                })
                .transpose()?;
            let mut state =
                GatewayState::for_tenants_with_backend(gateways, policy, jev, redactor)?
                    .with_worker_quotas(quotas)?;
            if let Some(oidc) = oidc {
                state = state.with_oidc(oidc)?;
            }
            let router = api::gateway_router(state);
            let shutdown = shutdown_signal()?;
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            eprintln!(
                "Gateway {} — {} tenant(s), audit vérifié, {} événements",
                listener.local_addr()?,
                tenant_count,
                event_count
            );
            for diagnostic in capacity_diagnostics {
                eprintln!("{diagnostic}");
            }
            serve_http(listener, router, shutdown).await?;
        }
        Command::MigrateControl {
            data,
            tenant_id,
            backup,
            target_version,
            confirm_control_stopped,
            max_events,
        } => {
            anyhow::ensure!(
                confirm_control_stopped,
                "Stop the control plane and all brokers before migration"
            );
            anyhow::ensure!(
                (1..=1_000_000).contains(&max_events),
                "invalid audit capacity"
            );
            let metadata = std::fs::symlink_metadata(&data)
                .context("Migration requires an existing audit database")?;
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0,
                "Migration requires an existing regular audit database"
            );
            let store = Store::open_existing_for_tenant(
                &data,
                &tenant_id,
                Redactor::new(vec![]),
                max_events,
            )?;
            let report = match target_version {
                2 => store.migrate_control_v2(&backup)?,
                3 => store.migrate_control_v3(&backup)?,
                4 => store.migrate_identity_v4(&backup)?,
                5 => store.migrate_human_access_v5(&backup)?,
                6 => store.migrate_workload_v6(&backup)?,
                7 => store.migrate_human_brokers_v7(&backup)?,
                _ => anyhow::bail!("unsupported migration target"),
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Dashboard(args) => {
            validate_bind(args.listen, args.container_listen)?;
            anyhow::ensure!(
                args.front_dir.join("index.html").is_file(),
                "Front absent : exécuter npm ci && npm run build dans front/"
            );
            let router = if args.shared_tenants {
                api::dashboard_router_with_oidc(
                    &args.upstream,
                    None,
                    args.front_dir,
                    args.oidc_web_origin,
                )?
            } else {
                let token_path = args
                    .viewer_token_file
                    .unwrap_or_else(|| PathBuf::from(".agentsandbox/secrets/viewer"));
                api::dashboard_router_with_oidc(
                    &args.upstream,
                    Some(read_token(&token_path)?),
                    args.front_dir,
                    args.oidc_web_origin,
                )?
            };
            let shutdown = shutdown_signal()?;
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            eprintln!("Dashboard {} — lecture seule", listener.local_addr()?);
            serve_http(listener, router, shutdown).await?;
        }
        Command::Ingress(args) => {
            validate_bind(args.gateway_listen, args.container_listen)?;
            validate_bind(args.dashboard_listen, args.container_listen)?;
            let gateway = agentsandbox::ingress::ingress_router(&args.gateway_upstream)?;
            let dashboard = agentsandbox::ingress::ingress_router(&args.dashboard_upstream)?;
            let shutdown = shutdown_signal()?.shared();
            let gateway_listener = tokio::net::TcpListener::bind(args.gateway_listen).await?;
            let dashboard_listener = tokio::net::TcpListener::bind(args.dashboard_listen).await?;
            eprintln!(
                "Ingress gateway {} / dashboard {} — relais sans secrets",
                gateway_listener.local_addr()?,
                dashboard_listener.local_addr()?
            );
            let gateway_shutdown = shutdown.clone();
            let dashboard_shutdown = shutdown.clone();
            let serving = async {
                tokio::try_join!(
                    async {
                        axum::serve(gateway_listener, gateway)
                            .with_graceful_shutdown(gateway_shutdown)
                            .await
                    },
                    async {
                        axum::serve(dashboard_listener, dashboard)
                            .with_graceful_shutdown(dashboard_shutdown)
                            .await
                    }
                )
                .map(|_| ())
            };
            tokio::pin!(serving);
            tokio::select! {
                result = &mut serving => { result?; }
                _ = shutdown => {
                    if let Ok(result) = tokio::time::timeout(std::time::Duration::from_secs(5), &mut serving).await {
                        result?;
                    } else {
                        // Main exits after this branch; runtime teardown ends
                        // remaining connection tasks, not the dropped future alone.
                        eprintln!("Ingress arrêté après le délai de drainage de 5 secondes.");
                    }
                }
            }
        }
        Command::ModelIngress(args) => {
            validate_bind(args.listen, args.container_listen)?;
            let identity = agentsandbox::tls::ServerIdentity::from_files(
                &args.tls_cert_file,
                &args.tls_key_file,
            )?;
            let router = agentsandbox::model_ingress::model_ingress_router()?;
            let shutdown = shutdown_signal()?;
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            eprintln!(
                "Model ingress {} — TLS; TCP32, handshakes8/2s, idle30s, drain5s; fixed model upstream, inference1, health4, body65536/2s, exchange5s",
                listener.local_addr()?
            );
            agentsandbox::tls::serve(listener, router, identity, shutdown).await?;
        }
        Command::Healthcheck { url: urls, ca_file } => {
            anyhow::ensure!(urls.len() <= 8, "Healthcheck limité à huit destinations");
            let destinations = urls
                .into_iter()
                .map(|value| {
                    let url = reqwest::Url::parse(&value).context("URL de healthcheck invalide")?;
                    anyhow::ensure!(
                        ["http", "https"].contains(&url.scheme())
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none(),
                        "URL de healthcheck sans credentials, query ni fragment requise"
                    );
                    Ok(url)
                })
                .collect::<Result<Vec<_>>>()?;
            anyhow::ensure!(
                ca_file.is_none() || destinations.iter().all(|url| url.scheme() == "https"),
                "Healthcheck CA requires HTTPS destinations"
            );
            let trust = ca_file
                .as_deref()
                .map(JevTrust::from_ca_file)
                .transpose()?
                .unwrap_or_default();
            let http = trust
                .apply_to_client(
                    reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(3))
                        .redirect(reqwest::redirect::Policy::none())
                        .retry(reqwest::retry::never())
                        .no_proxy(),
                )?
                .build()?;
            futures_util::future::try_join_all(destinations.into_iter().map(|url| {
                let http = &http;
                async move {
                    let mut response = http
                        .get(url)
                        .send()
                        .await
                        .map_err(|_| anyhow::anyhow!("Connexion healthcheck échouée"))?;
                    anyhow::ensure!(response.status().is_success(), "Healthcheck HTTP échoué");
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response
                        .chunk()
                        .await
                        .map_err(|_| anyhow::anyhow!("Lecture healthcheck échouée"))?
                    {
                        anyhow::ensure!(
                            bytes.len() + chunk.len() <= 4096,
                            "Réponse healthcheck trop volumineuse"
                        );
                        bytes.extend_from_slice(&chunk);
                    }
                    let status: serde_json::Value =
                        serde_json::from_slice(&bytes).context("Réponse healthcheck invalide")?;
                    anyhow::ensure!(status["status"] == "ok", "État de santé invalide");
                    Ok::<(), anyhow::Error>(())
                }
            }))
            .await?;
        }
        Command::InternalBrokerEnforcer {
            root,
            max_slots,
            recovery_only,
        } => {
            let root = absolute_path(&root)
                .and_then(|root| Directory::open_existing(&root))
                .map_err(|_| anyhow::anyhow!("private enforcer root validation failed"))?;
            if recovery_only {
                agentsandbox::broker::enforcer::run_internal_recovery_enforcer(&root, max_slots)
                    .await?;
            } else {
                agentsandbox::broker::enforcer::run_internal_enforcer(&root, max_slots).await?;
            }
        }
        Command::Operator(command) => operations::execute(command).await?,
    }
    Ok(())
}

#[cfg(test)]
#[path = "workload/main_config_tests.rs"]
mod workload_config_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_audit_capacity_file_conflicts_with_tenant_registry() {
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "gateway",
                "--audit-capacity-file",
                "capacity.json"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "gateway",
                "--audit-capacity-file",
                "capacity.json",
                "--tenants-file",
                "tenants.json"
            ])
            .is_err()
        );
    }

    fn gateway_arguments(arguments: &[&str]) -> GatewayArgs {
        let cli = Cli::try_parse_from(
            ["agentsandbox", "gateway"]
                .into_iter()
                .chain(arguments.iter().copied()),
        )
        .unwrap();
        match cli.command {
            Command::Gateway(args) => *args,
            _ => unreachable!(),
        }
    }

    #[test]
    fn jev_selection_is_explicit_and_mixed_options_fail_before_assembly() {
        let rules = gateway_arguments(&["--jev-backend", "rust-rules"]);
        assert!(matches!(
            select_jev(&rules, Some("invalid-secret-value")),
            Ok(SelectedJev::RustRules(SelectionSource::Cli))
        ));
        let none = gateway_arguments(&[]);
        assert!(matches!(select_jev(&none, None), Ok(SelectedJev::Disabled)));
        assert!(matches!(
            select_jev(&none, Some("rust-rules")),
            Ok(SelectedJev::RustRules(SelectionSource::Environment))
        ));
        let error = select_jev(&none, Some("private-invalid-value"))
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("private-invalid-value"));
        for arguments in [
            vec!["--jev-backend", "rust-rules", "--jev-model", "fixture"],
            vec!["--jev-backend", "rust-rules", "--jev-min-confidence", "0.9"],
            vec![
                "--jev-backend",
                "local-openai",
                "--jev-url",
                "https://example.invalid",
            ],
            vec![
                "--jev-local-url",
                "http://127.0.0.1:18090/v1/chat/completions",
            ],
            vec!["--jev-key-file", "/nonexistent-fixture/key"],
        ] {
            assert!(select_jev(&gateway_arguments(&arguments), None).is_err());
        }
        let legacy = gateway_arguments(&[
            "--jev-url",
            "https://example.invalid/v1/systemone",
            "--jev-model",
            "fixture",
            "--jev-key-file",
            "/nonexistent-fixture/key",
        ]);
        assert!(matches!(
            select_jev(&legacy, None),
            Ok(SelectedJev::TypeSafe(SelectionSource::Legacy))
        ));
    }

    #[test]
    fn gateway_cli_can_select_each_decision_backend_explicitly() {
        // Parsing the existing CLI opens no configuration/credential files.
        // This first absence test uses no future Rust field or backend type;
        // complete per-backend configuration is validated during assembly.
        for backend in ["typesafe", "rust-rules", "local-openai"] {
            let parsed = Cli::try_parse_from(["agentsandbox", "gateway", "--jev-backend", backend]);
            assert!(
                parsed.is_ok(),
                "the gateway CLI must accept each explicit decision backend selection"
            );
        }
    }

    #[test]
    fn jev_transport_preflight_and_model_tls_cli_are_explicit() {
        for args in [
            vec![
                "--jev-backend",
                "rust-rules",
                "--jev-ca-file",
                "/missing-public-ca",
            ],
            vec!["--jev-ca-file", "/missing-public-ca"],
        ] {
            assert!(select_jev(&gateway_arguments(&args), None).is_err());
        }
        let args = gateway_arguments(&[
            "--jev-backend",
            "local-openai",
            "--jev-local-url",
            "file:///not-http",
            "--jev-local-model",
            "fixture",
        ]);
        let selected = select_jev(&args, None).unwrap();
        assert!(
            preflight_jev(&args, selected)
                .err()
                .unwrap()
                .is::<agentsandbox::jev::JevError>()
        );
        assert!(Cli::try_parse_from(["agentsandbox", "model-ingress"]).is_err());
        assert!(matches!(
            Cli::try_parse_from([
                "agentsandbox",
                "model-ingress",
                "--tls-cert-file",
                "/chain",
                "--tls-key-file",
                "/key"
            ])
            .unwrap()
            .command,
            Command::ModelIngress(_)
        ));
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "healthcheck",
                "--ca-file",
                "/public-ca",
                "--url",
                "https://localhost/health"
            ])
            .is_ok()
        );
    }

    #[test]
    fn enforcer_recovery_requires_an_explicit_internal_mode() {
        let recovery = Cli::try_parse_from([
            "agentsandbox",
            "__broker-enforcer",
            "--root",
            "/private/fixture",
            "--max-slots",
            "8",
            "--recovery-only",
        ])
        .unwrap();
        assert!(matches!(
            recovery.command,
            Command::InternalBrokerEnforcer {
                recovery_only: true,
                ..
            }
        ));
        let legacy = Cli::try_parse_from([
            "agentsandbox",
            "__broker-enforcer",
            "--root",
            "/private/fixture",
            "--max-slots",
            "8",
        ])
        .unwrap();
        assert!(matches!(
            legacy.command,
            Command::InternalBrokerEnforcer {
                recovery_only: false,
                ..
            }
        ));
    }

    #[test]
    fn shared_dashboard_is_explicit_and_excludes_a_fixed_identity() {
        assert!(Cli::try_parse_from(["agentsandbox", "dashboard", "--shared-tenants"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "dashboard",
                "--shared-tenants",
                "--viewer-token-file",
                "viewer"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["agentsandbox", "dashboard"]).is_ok());
    }

    #[test]
    fn gateway_accepts_an_explicit_tenant_registry() {
        assert!(
            Cli::try_parse_from(["agentsandbox", "gateway", "--tenants-file", "tenants.json"])
                .is_ok()
        );
    }

    #[test]
    fn ingress_accepts_two_fixed_upstreams() {
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "ingress",
                "--gateway-upstream",
                "http://gateway:8787",
                "--dashboard-upstream",
                "http://dashboard:4000"
            ])
            .is_ok()
        );
    }

    #[test]
    fn healthcheck_requires_urls_and_accepts_both_ingress_destinations() {
        assert!(Cli::try_parse_from(["agentsandbox", "healthcheck"]).is_err());
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "healthcheck",
                "--url",
                "http://127.0.0.1:8787/healthz",
                "--url",
                "http://127.0.0.1:4000/healthz"
            ])
            .is_ok()
        );
    }

    #[test]
    fn tenant_registry_cannot_be_mixed_with_legacy_identity_options() {
        for option in ["--data", "--runner-token-file", "--viewer-token-file"] {
            assert!(
                Cli::try_parse_from([
                    "agentsandbox",
                    "gateway",
                    "--tenants-file",
                    "tenants.json",
                    option,
                    "legacy"
                ])
                .is_err()
            );
        }
    }
}
