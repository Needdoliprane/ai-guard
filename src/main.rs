use agentsandbox::{
    api::{self, GatewayState, TenantGateway},
    audit::Store,
    control_fs::{Directory, absolute_path},
    gateway_server::GatewayServer,
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
mod human_sources_cli;

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
    /// Print static local build identity without opening configuration or data.
    Version {
        #[arg(long)]
        json: bool,
    },
    /// Explicit durable local broker, jobs and controlled exports.
    Broker(broker_cli::BrokerArgs),
    /// Named local accounts, authorized human reads and filtered audit export.
    Human(agentsandbox::identity_cli::HumanArgs),
    /// Explicit private preparation, migration and activation of owned human sources.
    HumanSources(human_sources_cli::HumanSourcesArgs),
    /// Prepare or recover a new private local OIDC installation offline.
    Setup(agentsandbox::setup::SetupArgs),
    /// Prepare, record and resume private workload credential operations.
    Workload(agentsandbox::workload_cli::WorkloadArgs),
    /// Inspect an explicitly configured Kubernetes authority without admitting workers.
    Kubernetes(agentsandbox::kubernetes_cli::KubernetesArgs),
    #[command(name = "__broker-enforcer", hide = true)]
    InternalBrokerEnforcer {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        max_slots: u32,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=1024))]
        max_recovery_stop_owners: u32,
        #[arg(long)]
        recovery_only: bool,
        #[arg(long, conflicts_with_all = ["recovery_only", "full3_existing"], requires_all = ["role_name", "installation_id"])]
        full3_fresh: bool,
        #[arg(long, conflicts_with = "recovery_only")]
        full3_existing: bool,
        #[arg(long, conflicts_with_all = ["recovery_only", "full3_fresh", "full3_existing"], requires = "role_name")]
        legacy_fresh: bool,
        #[arg(long)]
        role_name: Option<String>,
        #[arg(long, requires = "full3_fresh")]
        installation_id: Option<uuid::Uuid>,
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
        /// Exact next schema version: 2 (control), 3 (supervision), 4 (identity), 5 (human read audit), 6 (workloads), 7 (human broker registry reads), 8 (held Kubernetes creates), 9 (held-create policy marker), 10 (human worker registry reads), 11 (effect-free Kubernetes session reservations), 12 (broker checkpoint records), 13 (control-only held-create recovery).
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(2..=13))]
        target_version: u32,
        /// Operator attestation: gateway and brokers are stopped; not runtime fencing.
        #[arg(long, required = true)]
        confirm_control_stopped: bool,
        #[arg(long, default_value_t = 100_000)]
        max_events: u64,
    },
    /// Migrate the existing offline SSO audit one explicit version with a new backup.
    MigrateAuthentication {
        /// Existing private gateway OIDC configuration; no provider contact.
        #[arg(long)]
        oidc_config: PathBuf,
        /// New private backup path; existing destinations are never overwritten.
        #[arg(long)]
        backup: PathBuf,
        /// Exact next format: 2 (verified initial proofs), 3 (native logout terminal reserve), 4 (tenant capacity plan).
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(2..=4))]
        target_version: u32,
        /// Operator attestation: all audit consumers are stopped; not runtime fencing.
        #[arg(long, required = true)]
        confirm_control_stopped: bool,
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
    /// Published private owned-source activation; reconstruct and replay at every startup.
    #[arg(long, conflicts_with_all=["human_source_mode","human_sources_manifest","human_credentials_envelope","human_control_config_revision","human_oidc_mapping_revision","tenants_file","audit_capacity_file","workload_audit_file","relay_retention_file","oidc_config","initialize_oidc_audit","data","runner_token_file","viewer_token_file","max_events","initialize_supervised_stores","worker_limit","worker_cpu_millis","worker_memory_bytes","worker_max_seconds"])]
    human_sources_activation: Option<PathBuf>,
    /// Explicit human source protocol; legacy preserves the existing gateway.
    #[arg(long,value_parser=["legacy","owned_sources"])]
    human_source_mode: Option<String>,
    /// Existing private closure manifest; never creates or migrates a source.
    #[arg(long, requires = "tenants_file")]
    human_sources_manifest: Option<PathBuf>,
    /// Existing private technical credential envelope, matching tenant tokens.
    #[arg(long, requires = "human_sources_manifest")]
    human_credentials_envelope: Option<PathBuf>,
    /// Independent positive revision of the retained public control configuration.
    #[arg(long, requires = "human_sources_manifest")]
    human_control_config_revision: Option<u64>,
    /// Independent positive closure revision; distinct from OIDC mapping.revision String.
    #[arg(long,requires_all=["human_sources_manifest","oidc_config"])]
    human_oidc_mapping_revision: Option<u64>,
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,
    /// Private gateway-root control listener; /control also remains on --listen.
    #[arg(long)]
    control_listen: Option<SocketAddr>,
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
    /// Private relay retention budgets for the local tenant; never deletes retained content.
    #[arg(long, conflicts_with = "tenants_file")]
    relay_retention_file: Option<PathBuf>,
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
    /// Trusted presentation profile; the gateway enforces authority separately.
    #[arg(long,default_value="legacy",value_parser=["legacy","owned_sources"])]
    human_source_mode: String,
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
    /// Fixed dedicated gateway control origin, required; never falls back.
    #[arg(long)]
    control_upstream: String,
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
    agentsandbox::human_authority::migration::operator::parse_private_token_file(token.as_bytes())
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
    bounds: agentsandbox::http_listener_bounds::ListenerBounds,
) -> Result<()> {
    let shutdown = shutdown.shared();
    let server_shutdown = shutdown.clone();
    let serving = async move {
        agentsandbox::http_listener_bounds::serve(listener, router, server_shutdown, bounds).await
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

fn configured_local_tenant(args: &GatewayArgs) -> Result<gateway_config::ConfiguredTenant> {
    Ok(gateway_config::ConfiguredTenant {
        tenant_id: "local".into(),
        data: args.data.clone(),
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
        relay_retention_policy: args
            .relay_retention_file
            .as_deref()
            .map(Store::load_relay_retention_policy)
            .transpose()?,
        held_create_policy: None,
        session_reservation_policy: None,
        session_recovery_policy: None,
        session_control_recovery_policy: None,
    })
}

#[derive(Default)]
struct GatewaySessionPolicies {
    reservation: Vec<(
        String,
        agentsandbox::control::kubernetes_session::KubernetesSessionReservationPolicyV2,
    )>,
    recovery: Vec<(
        String,
        agentsandbox::control::kubernetes_session::KubernetesSessionRecoveryPolicyV2,
    )>,
    control_recovery: Vec<(
        String,
        agentsandbox::control::kubernetes_session::KubernetesSessionControlRecoveryPolicyV2,
    )>,
}

async fn serve_owned_human_gateway(
    args: &GatewayArgs,
    tenants: Vec<gateway_config::ConfiguredTenant>,
    policy: Policy,
    jev: Option<JevBackend>,
    redactor: Redactor,
) -> Result<()> {
    let prepared = gateway_config::load_owned_sources(
        args.human_sources_manifest
            .as_deref()
            .context("owned human manifest required")?,
        args.human_credentials_envelope
            .as_deref()
            .context("owned human credential envelope required")?,
        args.tenants_file
            .as_deref()
            .context("owned human tenant configuration required")?,
        args.oidc_config.as_deref(),
        &tenants,
        args.max_events,
        args.human_control_config_revision
            .context("explicit owned human control configuration revision required")?,
        args.human_oidc_mapping_revision,
        redactor.clone(),
    )?;
    serve_prepared_owned_human_gateway(args, tenants, prepared, policy, jev, redactor).await
}

async fn serve_prepared_owned_human_gateway(
    args: &GatewayArgs,
    tenants: Vec<gateway_config::ConfiguredTenant>,
    prepared: gateway_config::human_sources::PreparedOwnedSources,
    policy: Policy,
    jev: Option<JevBackend>,
    redactor: Redactor,
) -> Result<()> {
    let policies = GatewaySessionPolicies::from_tenants(&tenants);
    let tokens = tenants
        .iter()
        .map(|tenant| {
            (
                tenant.tenant_id.clone(),
                tenant.runner_token.clone(),
                tenant.viewer_token.clone(),
            )
        })
        .collect();
    let mut state = GatewayState::for_owned_human_sources(
        prepared.bundle,
        prepared.credentials,
        tokens,
        policy,
        jev,
        redactor,
    )?;
    // Construction reserves cleanup before any later fallible builder/bind.
    let server = GatewayServer::new_owned(
        state.control_recovery_retirement(),
        state
            .human_retirement()
            .context("owned human cleanup handle required")?,
    )?;
    let quotas = tenants
        .iter()
        .map(|tenant| (tenant.tenant_id.clone(), tenant.worker_quota.clone()))
        .collect();
    let held = tenants
        .iter()
        .filter_map(|tenant| {
            tenant
                .held_create_policy
                .clone()
                .map(|policy| (tenant.tenant_id.clone(), policy))
        })
        .collect();
    state = policies.apply(
        state
            .with_worker_quotas(quotas)?
            .with_kubernetes_held_create_policies(held)?,
    )?;
    let control = args
        .control_listen
        .map(|address| (address, api::gateway_control_router(state.clone())));
    let router = api::gateway_router(state);
    server.serve((args.listen,router),control,shutdown_signal()?,|listen,_|{
        eprintln!("Gateway {} — owned human sources; identity/resources adopted; remaining modern routes closed",listen);
        Ok(())
    }).await
}

impl GatewaySessionPolicies {
    fn from_tenants(tenants: &[gateway_config::ConfiguredTenant]) -> Self {
        let mut policies = Self::default();
        for tenant in tenants {
            if let Some(policy) = &tenant.session_reservation_policy {
                policies
                    .reservation
                    .push((tenant.tenant_id.clone(), policy.clone()));
            }
            if let Some(policy) = &tenant.session_recovery_policy {
                policies
                    .recovery
                    .push((tenant.tenant_id.clone(), policy.clone()));
            }
            if let Some(policy) = &tenant.session_control_recovery_policy {
                policies
                    .control_recovery
                    .push((tenant.tenant_id.clone(), policy.clone()));
            }
        }
        policies
    }

    fn apply(self, state: GatewayState) -> Result<GatewayState> {
        state
            .with_kubernetes_session_reservation_policies(self.reservation)?
            .with_kubernetes_session_recovery_policies(self.recovery)?
            .with_kubernetes_session_control_recovery_policies(self.control_recovery)
    }
}

fn main() -> Result<()> {
    if let Some(code) = agentsandbox::build_identity::early_exit(
        agentsandbox::build_identity::Component::Agentsandbox,
    ) {
        std::process::exit(code);
    }
    run()
}

#[tokio::main]
async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Version { json } => {
            let args = if json {
                vec!["version".into(), "--json".into()]
            } else {
                vec!["version".into()]
            };
            if let Some(output) = agentsandbox::build_identity::early_output(
                agentsandbox::build_identity::Component::Agentsandbox,
                &args,
            ) {
                print!("{}", output?);
            }
        }
        Command::Broker(args) => broker_cli::execute(args).await?,
        Command::Human(args) => agentsandbox::identity_cli::execute(args).await?,
        Command::HumanSources(args) => human_sources_cli::run(args).await?,
        Command::Setup(args) => agentsandbox::setup::execute(args)?,
        Command::Workload(args) => agentsandbox::workload_cli::run(args).await?,
        Command::Kubernetes(args) => agentsandbox::kubernetes_cli::execute(args).await?,
        Command::ModelTls(args) => {
            agentsandbox::model_tls::execute(args, shutdown_signal()?).await?
        }
        Command::InternalModelTlsImport => agentsandbox::model_tls::internal_import()
            .await
            .map_err(|_| anyhow::anyhow!("model TLS import refused"))?,
        Command::Gateway(args) => {
            let args = *args;
            gateway_config::validate_human_mode(
                args.human_source_mode.as_deref().unwrap_or("legacy"),
                args.human_sources_manifest.as_deref(),
                args.human_credentials_envelope.as_deref(),
                args.tenants_file.as_deref(),
                args.initialize_supervised_stores.is_some(),
                args.initialize_oidc_audit,
            )?;
            validate_bind(args.listen, args.container_listen)?;
            if let Some(control) = args.control_listen {
                validate_bind(control, args.container_listen)?;
                anyhow::ensure!(
                    control != args.listen,
                    "control listener must differ from public listener"
                );
            }
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
            let activation = args
                .human_sources_activation
                .as_deref()
                .map(human_sources_cli::load_human_sources_activation)
                .transpose()?;
            let (tenants, activation_request) =
                if let Some(human_sources_cli::LoadedHumanSourcesActivation {
                    tenants,
                    request,
                    credentials,
                }) = activation
                {
                    (tenants, Some((request, credentials)))
                } else {
                    (
                        if let Some(path) = &args.tenants_file {
                            if args.human_source_mode.as_deref() == Some("owned_sources") {
                                gateway_config::load_owned_tenants(path)?
                            } else {
                                gateway_config::load(path)?
                            }
                        } else {
                            vec![configured_local_tenant(&args)?]
                        },
                        None,
                    )
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
            if let Some((mut request, credentials)) = activation_request {
                // Apply the same complete redactor as the direct owned path;
                // public configuration digests exclude private redaction data.
                for options in request.sources.values_mut() {
                    use agentsandbox::human_authority::startup::HumanOwnedSourceOptions;
                    match options {
                        HumanOwnedSourceOptions::TenantStore {
                            redactor: source, ..
                        }
                        | HumanOwnedSourceOptions::AuthenticationLedger {
                            redactor: source, ..
                        } => *source = source.union(&redactor),
                    }
                }
                let bundle =
                    agentsandbox::human_authority::startup::HumanOwnedStartupBundle::open(request)?;
                return serve_prepared_owned_human_gateway(
                    &args,
                    tenants,
                    gateway_config::human_sources::PreparedOwnedSources {
                        bundle,
                        credentials,
                    },
                    policy,
                    jev,
                    redactor,
                )
                .await;
            }
            // Validate the complete registry before preparing backups or any
            // fresh origin: a later tenant must not strand an earlier one.
            gateway_config::validate_policies(&tenants, args.max_events)?;
            if args.human_source_mode.as_deref() == Some("owned_sources") {
                return serve_owned_human_gateway(&args, tenants, policy, jev, redactor).await;
            }
            let session_policies = GatewaySessionPolicies::from_tenants(&tenants);
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
            let mut held_create_policies = Vec::new();
            let mut event_count = 0_u64;
            let mut capacity_diagnostics = Vec::with_capacity(tenants.len());
            for tenant in tenants {
                quotas.push((tenant.tenant_id.clone(), tenant.worker_quota));
                if let Some(held_create_policy) = tenant.held_create_policy {
                    held_create_policies.push((tenant.tenant_id.clone(), held_create_policy));
                }
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
                let relay_source = match (&tenant.relay_retention_policy, &args.tenants_file) {
                    (None, _) => "derived_defaults",
                    (Some(_), Some(_)) => "tenant_registry",
                    (Some(_), None) => "private_policy_file",
                };
                let store = if let Some(policy) = tenant.relay_retention_policy {
                    store.with_relay_retention_policy(policy)?
                } else {
                    store
                };
                capacity_diagnostics.push(redactor.text(&format!(
                    "Relay retention tenant={} source={} release_on_ack={} policy={} fingerprint={}",
                    store.tenant_id(),
                    relay_source,
                    if store.relay_retention_policy().release_on_ack {
                        "enabled_history_unreadable_by_earlier_binaries"
                    } else {
                        "disabled"
                    },
                    serde_json::to_string(store.relay_retention_policy())?,
                    store.relay_retention_policy_fingerprint()?,
                )));
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
                GatewayState::for_tenants_with_backend(gateways, policy, jev, redactor)?;
            // Own retirement before any fallible setup after state creation.
            let server = GatewayServer::new(state.control_recovery_retirement());
            state = state
                .with_worker_quotas(quotas)?
                .with_kubernetes_held_create_policies(held_create_policies)?;
            state = session_policies.apply(state)?;
            if let Some(oidc) = oidc {
                state = state.with_oidc(oidc)?;
            }
            let control_router = args
                .control_listen
                .map(|_| api::gateway_control_router(state.clone()));
            let router = api::gateway_router(state);
            let shutdown = shutdown_signal()?;
            let control = args.control_listen.zip(control_router);
            server
                .serve(
                    (args.listen, router),
                    control,
                    shutdown,
                    move |listen, _control| {
                        eprintln!(
                            "Gateway {} — {} tenant(s), audit vérifié, {} événements",
                            listen, tenant_count, event_count
                        );
                        for diagnostic in capacity_diagnostics {
                            eprintln!("{diagnostic}");
                        }
                        Ok(())
                    },
                )
                .await?;
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
                8 => store.migrate_kubernetes_create_v8(&backup)?,
                9 => store.migrate_kubernetes_policy_v9(&backup)?,
                10 => store.migrate_human_workers_v10(&backup)?,
                11 => store.migrate_kubernetes_session_reservations_v11(&backup)?,
                12 => store.migrate_broker_checkpoints_v12(&backup)?,
                13 => store.migrate_kubernetes_recovery_v13(&backup)?,
                _ => anyhow::bail!("unsupported migration target"),
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::MigrateAuthentication {
            oidc_config,
            backup,
            target_version,
            confirm_control_stopped,
        } => {
            anyhow::ensure!(
                confirm_control_stopped,
                "Stop every authentication audit consumer before migration; --confirm-control-stopped is an operator attestation, not runtime fencing"
            );
            let migration = match target_version {
                2 => "verified_initial_v2",
                3 => "native_logout_v3",
                4 => "tenant_capacity_v4",
                _ => anyhow::bail!("unsupported authentication migration target"),
            };
            let report = gateway_config::migrate_authentication(
                &oidc_config,
                &backup,
                target_version,
                Redactor::new(vec![]),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "version": 1,
                    "migration": migration,
                    "namespace_id": report.namespace_id,
                    "records": report.records,
                    "serialized_bytes": report.serialized_bytes,
                    "head": report.head,
                }))?
            );
        }
        Command::Dashboard(args) => {
            validate_bind(args.listen, args.container_listen)?;
            anyhow::ensure!(
                args.human_source_mode != "owned_sources" || args.shared_tenants,
                "owned human dashboard requires --shared-tenants; no viewer fallback"
            );
            anyhow::ensure!(
                args.front_dir.join("index.html").is_file(),
                "Front absent : exécuter npm ci && npm run build dans front/"
            );
            let router = if args.shared_tenants {
                api::dashboard_router_with_human_mode(
                    &args.upstream,
                    None,
                    args.front_dir,
                    args.oidc_web_origin,
                    args.human_source_mode == "owned_sources",
                )?
            } else {
                let token_path = args
                    .viewer_token_file
                    .unwrap_or_else(|| PathBuf::from(".agentsandbox/secrets/viewer"));
                api::dashboard_router_with_human_mode(
                    &args.upstream,
                    Some(read_token(&token_path)?),
                    args.front_dir,
                    args.oidc_web_origin,
                    args.human_source_mode == "owned_sources",
                )?
            };
            let shutdown = shutdown_signal()?;
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            eprintln!("Dashboard {} — lecture seule", listener.local_addr()?);
            serve_http(
                listener,
                router,
                shutdown,
                agentsandbox::http_listener_bounds::ListenerBounds::new(
                    128,
                    std::time::Duration::from_secs(15),
                )?,
            )
            .await?;
        }
        Command::Ingress(args) => {
            validate_bind(args.gateway_listen, args.container_listen)?;
            validate_bind(args.dashboard_listen, args.container_listen)?;
            let gateway = agentsandbox::ingress::ingress_router_with_control(
                &args.gateway_upstream,
                &args.control_upstream,
            )?;
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
                        agentsandbox::http_listener_bounds::serve(
                            gateway_listener,
                            gateway,
                            gateway_shutdown,
                            agentsandbox::http_listener_bounds::ListenerBounds::new(
                                128,
                                std::time::Duration::from_secs(15),
                            )?,
                        )
                        .await
                    },
                    async {
                        agentsandbox::http_listener_bounds::serve(
                            dashboard_listener,
                            dashboard,
                            dashboard_shutdown,
                            agentsandbox::http_listener_bounds::ListenerBounds::new(
                                128,
                                std::time::Duration::from_secs(15),
                            )?,
                        )
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
            max_recovery_stop_owners,
            recovery_only,
            full3_fresh,
            full3_existing,
            legacy_fresh,
            role_name,
            installation_id,
        } => {
            let root = absolute_path(&root)
                .and_then(|root| Directory::open_existing(&root))
                .map_err(|_| anyhow::anyhow!("private enforcer root validation failed"))?;
            if !legacy_fresh && !full3_fresh {
                anyhow::ensure!(
                    role_name.is_none() && installation_id.is_none(),
                    "existing/recovery role does not accept fresh role metadata"
                );
            }
            if legacy_fresh {
                use std::io::Read;
                let role_name = role_name.ok_or_else(|| {
                    anyhow::anyhow!("explicit fresh Legacy role name is required")
                })?;
                anyhow::ensure!(
                    installation_id.is_none(),
                    "Legacy does not accept a Full3 installation binding"
                );
                // Private pipe from the actual spawning parent, bounded and
                // consumed only on this explicit child factory branch.
                let mut bytes = Vec::with_capacity(17);
                std::io::stdin().take(17).read_to_end(&mut bytes)?;
                anyhow::ensure!(
                    bytes.len() == 16,
                    "private Legacy launch correlation is missing"
                );
                let nonce = uuid::Uuid::from_slice(&bytes)?;
                anyhow::ensure!(
                    !nonce.is_nil(),
                    "private Legacy launch correlation is invalid"
                );
                agentsandbox::broker::enforcer::run_internal_legacy_fresh(
                    &root,
                    &role_name,
                    max_slots,
                    max_recovery_stop_owners,
                    nonce,
                )
                .await?;
            } else if full3_fresh {
                let role_name = role_name
                    .ok_or_else(|| anyhow::anyhow!("explicit fresh Full3 role name is required"))?;
                let installation_id = installation_id.ok_or_else(|| {
                    anyhow::anyhow!("explicit fresh Full3 installation is required")
                })?;
                agentsandbox::broker::enforcer::run_internal_full3_fresh_with_budget(
                    &root,
                    &role_name,
                    installation_id,
                    max_slots,
                    max_recovery_stop_owners,
                )
                .await?;
            } else if full3_existing {
                agentsandbox::broker::enforcer::run_internal_full3_existing_with_budget(
                    &root,
                    max_slots,
                    max_recovery_stop_owners,
                )
                .await?;
            } else if recovery_only {
                agentsandbox::broker::enforcer::run_internal_recovery_enforcer_with_budget(
                    &root,
                    max_slots,
                    max_recovery_stop_owners,
                )
                .await?;
            } else {
                anyhow::ensure!(
                    role_name.is_none() && installation_id.is_none(),
                    "existing Legacy does not accept fresh role metadata"
                );
                agentsandbox::broker::enforcer::run_internal_enforcer_with_budget(
                    &root,
                    max_slots,
                    max_recovery_stop_owners,
                )
                .await?;
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
#[path = "gateway_main_tests.rs"]
mod gateway_main_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_cli_accepts_12_and_preserves_default_and_upper_bound() {
        let base = [
            "agentsandbox",
            "migrate-control",
            "--data",
            "fixture.sqlite",
            "--tenant-id",
            "tenant-a",
            "--backup",
            "fixture-backup.sqlite",
            "--confirm-control-stopped",
        ];
        let parsed = Cli::try_parse_from(base).unwrap();
        assert!(matches!(
            parsed.command,
            Command::MigrateControl {
                target_version: 2,
                ..
            }
        ));
        for target in ["11", "12", "13"] {
            let parsed = Cli::try_parse_from(base.into_iter().chain(["--target-version", target]))
                .expect("explicit supported migration target");
            let Command::MigrateControl { target_version, .. } = parsed.command else {
                panic!("migration command required");
            };
            assert_eq!(target_version.to_string(), target);
        }
        for unknown in ["1", "14", "4294967295"] {
            assert!(
                Cli::try_parse_from(base.into_iter().chain(["--target-version", unknown])).is_err()
            );
        }
    }

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
                max_recovery_stop_owners: 0,
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
    fn recovery_stop_budget_is_explicit_bounded_and_preserved_at_full3_genesis() {
        let base = [
            "agentsandbox",
            "__broker-enforcer",
            "--root",
            "/private/fixture",
            "--max-slots",
            "8",
        ];
        for mode in ["--recovery-only", "--full3-existing"] {
            for budget in ["0", "1", "1024"] {
                let parsed = Cli::try_parse_from(base.into_iter().chain([
                    mode,
                    "--max-recovery-stop-owners",
                    budget,
                ]))
                .unwrap();
                let Command::InternalBrokerEnforcer {
                    max_recovery_stop_owners,
                    ..
                } = parsed.command
                else {
                    panic!("explicit internal role is required");
                };
                assert_eq!(max_recovery_stop_owners.to_string(), budget);
            }
            for invalid in ["-1", "1025", "4294967295"] {
                assert!(
                    Cli::try_parse_from(base.into_iter().chain([
                        mode,
                        "--max-recovery-stop-owners",
                        invalid,
                    ]))
                    .is_err()
                );
            }
        }
        let fresh = Cli::try_parse_from(base.into_iter().chain([
            "--full3-fresh",
            "--role-name",
            "fresh-role",
            "--installation-id",
            "11111111-1111-4111-8111-111111111111",
            "--max-recovery-stop-owners",
            "1",
        ]))
        .unwrap();
        assert!(matches!(
            fresh.command,
            Command::InternalBrokerEnforcer {
                full3_fresh: true,
                max_recovery_stop_owners: 1,
                ..
            }
        ));
    }

    #[test]
    fn legacy_fresh_requires_explicit_factory_and_preserves_recovery_budget() {
        let base = [
            "agentsandbox",
            "__broker-enforcer",
            "--root",
            "/private/fixture",
            "--max-slots",
            "1",
        ];
        for budget in ["0", "1", "2", "1024"] {
            let parsed = Cli::try_parse_from(base.into_iter().chain([
                "--legacy-fresh",
                "--role-name",
                "native-legacy",
                "--max-recovery-stop-owners",
                budget,
            ]))
            .unwrap();
            let Command::InternalBrokerEnforcer {
                legacy_fresh: true,
                full3_fresh: false,
                full3_existing: false,
                recovery_only: false,
                role_name: Some(name),
                max_recovery_stop_owners,
                ..
            } = parsed.command
            else {
                panic!("explicit Legacy factory required");
            };
            assert_eq!(name, "native-legacy");
            assert_eq!(max_recovery_stop_owners.to_string(), budget);
        }
        assert!(Cli::try_parse_from(base.into_iter().chain(["--legacy-fresh"])).is_err());
        assert!(
            Cli::try_parse_from(base.into_iter().chain([
                "--legacy-fresh",
                "--role-name",
                "native-legacy",
                "--recovery-only"
            ]))
            .is_err()
        );
        assert!(
            Cli::try_parse_from(base.into_iter().chain([
                "--legacy-fresh",
                "--role-name",
                "native-legacy",
                "--max-recovery-stop-owners",
                "1025"
            ]))
            .is_err()
        );
        // Parser only: the private stdin pipe and genuine mkdir/lease are
        // exercised by native tests; this constructs no Fresh permission.
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
    fn ingress_requires_an_explicit_control_upstream() {
        assert!(Cli::try_parse_from(["agentsandbox", "ingress"]).is_err());
        assert!(
            Cli::try_parse_from([
                "agentsandbox",
                "ingress",
                "--gateway-upstream",
                "http://gateway:8787",
                "--control-upstream",
                "http://gateway:8788",
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
