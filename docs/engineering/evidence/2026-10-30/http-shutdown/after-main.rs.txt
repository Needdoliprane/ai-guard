use agentsandbox::{
    api::{self, GatewayState, TenantGateway},
    audit::Store,
    jev::{JevClient, validate_credential_separation},
    operations::{self, OperatorCommand},
    policy::Policy,
    redaction::Redactor,
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use futures_util::FutureExt;
use std::{
    future::Future,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

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
    /// Collecteur d'audit et moteur de décisions ; normalement lancé dans un conteneur.
    Gateway(GatewayArgs),
    /// Serveur de l'IHM et proxy strictement en lecture seule.
    Dashboard(DashboardArgs),
    /// Relais de confiance sans secrets vers les deux services internes.
    Ingress(IngressArgs),
    /// Migrate an offline audit database, preserving a verified new backup.
    MigrateControl {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        backup: PathBuf,
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
    #[arg(long, default_value_t = 3)]
    worker_limit: u32,
    #[arg(long, default_value_t = 6000)]
    worker_cpu_millis: u32,
    #[arg(long, default_value_t = 3 * 1024 * 1024 * 1024)]
    worker_memory_bytes: u64,
    #[arg(long, default_value_t = 3600)]
    worker_max_seconds: u32,
    #[arg(long, env = "JEV_URL")]
    jev_url: Option<String>,
    #[arg(long, env = "JEV_API_KEY_FILE")]
    jev_key_file: Option<PathBuf>,
    #[arg(long, env = "JEV_MODEL")]
    jev_model: Option<String>,
    #[arg(long, default_value_t = 0.85)]
    jev_min_confidence: f64,
    #[arg(long)]
    jev_allow_local_http: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Gateway(args) => {
            validate_bind(args.listen, args.container_listen)?;
            let tenants = if let Some(path) = &args.tenants_file {
                gateway_config::load(path)?
            } else {
                vec![gateway_config::ConfiguredTenant {
                    tenant_id: "local".into(),
                    data: args.data,
                    runner_token: read_token(&args.runner_token_file)?,
                    viewer_token: read_token(&args.viewer_token_file)?,
                    worker_quota: agentsandbox::control::QuotaPolicyV1 {
                        version: 1,
                        max_active_workers: args.worker_limit,
                        max_cpu_millis: args.worker_cpu_millis,
                        max_memory_bytes: args.worker_memory_bytes,
                        max_timeout_seconds: args.worker_max_seconds,
                    },
                }]
            };
            let mut secrets: Vec<String> = tenants
                .iter()
                .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
                .collect();
            let jev = if let Some(url) = args.jev_url {
                let key = read_token(
                    args.jev_key_file
                        .as_deref()
                        .context("--jev-key-file requis lorsque Jev est activé")?,
                )?;
                validate_credential_separation(&key, secrets.iter().map(String::as_str))?;
                secrets.push(key.clone());
                Some(JevClient::new(
                    &url,
                    key,
                    args.jev_model.context("--jev-model explicite requis")?,
                    args.jev_min_confidence,
                    args.jev_allow_local_http,
                )?)
            } else {
                anyhow::ensure!(
                    args.jev_key_file.is_none() && args.jev_model.is_none(),
                    "Configuration Jev incomplète : fournir --jev-url"
                );
                None
            };
            let redactor = Redactor::new(secrets);
            let policy: Policy = serde_json::from_slice(
                &std::fs::read(&args.policy).context("Politique locale manquante")?,
            )
            .context("Politique locale invalide")?;
            anyhow::ensure!(
                args.max_events > 0 && args.max_events <= 1_000_000,
                "max-events doit être compris entre 1 et 1000000"
            );
            let mut gateways = Vec::new();
            let mut quotas = Vec::new();
            let mut event_count = 0_u64;
            for tenant in tenants {
                tenant
                    .worker_quota
                    .validate()
                    .context("Quota workers invalide")?;
                quotas.push((tenant.tenant_id.clone(), tenant.worker_quota));
                let store = Arc::new(Store::open_for_tenant(
                    &tenant.data,
                    &tenant.tenant_id,
                    redactor.clone(),
                    args.max_events,
                )?);
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
            let router = api::gateway_router(
                GatewayState::for_tenants(gateways, policy, jev, redactor)?
                    .with_worker_quotas(quotas)?,
            );
            let shutdown = shutdown_signal()?;
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            eprintln!(
                "Gateway {} — {} tenant(s), audit vérifié, {} événements",
                listener.local_addr()?,
                tenant_count,
                event_count
            );
            serve_http(listener, router, shutdown).await?;
        }
        Command::MigrateControl {
            data,
            tenant_id,
            backup,
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
            let store =
                Store::open_for_tenant(&data, &tenant_id, Redactor::new(vec![]), max_events)?;
            let report = store.migrate_control_v2(&backup)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Dashboard(args) => {
            validate_bind(args.listen, args.container_listen)?;
            anyhow::ensure!(
                args.front_dir.join("index.html").is_file(),
                "Front absent : exécuter npm ci && npm run build dans front/"
            );
            let router = if args.shared_tenants {
                api::dashboard_shared_router(&args.upstream, args.front_dir)?
            } else {
                let token_path = args
                    .viewer_token_file
                    .unwrap_or_else(|| PathBuf::from(".agentsandbox/secrets/viewer"));
                api::dashboard_router(&args.upstream, read_token(&token_path)?, args.front_dir)?
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
        Command::Healthcheck { url: urls } => {
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
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
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
        Command::Operator(command) => operations::execute(command).await?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
