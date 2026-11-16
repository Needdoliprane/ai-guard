//! Operator entry for the pre-effect R2 lifecycle. No Kubernetes credential,
//! API request, client process or runtime invocation exists in this module.
use crate::{
    broker::held_create_client::{
        HeldCreateClientScope,
        sessions::{SessionControlClientV2, SessionControlErrorV2},
    },
    control::kubernetes_session::{
        KubernetesSessionReservationIntentV2, KubernetesSessionReservationStateDataV2,
    },
    control_fs::{Directory, PrivateFile},
    redaction::Redactor,
    runtime::kubernetes::{
        session::{SessionJournalProtection, SessionPhaseV2},
        session_lifecycle::remote::{self, RemoteSessionErrorV2},
    },
};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf, sync::Arc};
use tokio::sync::Semaphore;

#[derive(Args)]
pub(super) struct SessionArgs {
    #[command(subcommand)]
    command: SessionCommand,
}
#[derive(Subcommand)]
enum SessionCommand {
    /// Reserve and live-confirm the exact intent, then prepare or reopen its private journal. No Pod is created.
    Prepare {
        #[command(flatten)]
        connection: Connection,
        #[arg(long)]
        intent_file: PathBuf,
    },
    /// Retain an abandonment once and submit that exact operation; retries resume the same retained UUID.
    Abandon {
        #[command(flatten)]
        connection: Connection,
    },
    /// Read the retained journal and authenticated central state; runtime state stays unknown.
    Status {
        #[command(flatten)]
        connection: Connection,
    },
}
#[derive(Args)]
struct Connection {
    /// Absolute path to a private SessionControlConfigV2 JSON file. Broker token is read from its private file.
    #[arg(long)]
    control_file: PathBuf,
    /// Explicit private journal directory. Prepare may create it; other commands require it to exist.
    #[arg(long)]
    journal_dir: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionControlConfigV2 {
    version: u32,
    gateway: String,
    tenant_id: String,
    control_namespace_uuid: String,
    broker_id: String,
    incarnation_id: String,
    credential_id: String,
    broker_token_file: PathBuf,
    #[serde(default)]
    gateway_ca_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCliErrorV2 {
    Configuration,
    Intent,
    Journal,
    Identity,
    Forbidden,
    NotFound,
    Conflict,
    Capacity,
    Unavailable,
    ReceptionUnknown,
    InvalidAck,
    NoRetainedAbandonment,
    Output,
}
impl std::fmt::Display for SessionCliErrorV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration => "session control configuration or private credential is invalid; verify the explicit unchanged files",
            Self::Intent => "session reservation intent is invalid or contains protected material; use the unchanged closed V2 intent",
            Self::Journal => "session journal is busy, changed or inconsistent; preserve it and retry or inspect it privately",
            Self::Identity => "session control authentication or expected broker scope differs; no fallback identity was used",
            Self::Forbidden => "current broker scope or configured tenant policy refuses the session operation",
            Self::NotFound => "the exact session reservation or retained operation was not found; preserve the journal",
            Self::Conflict => "session operation conflicts with durable state; preserve the same intent and retained operation",
            Self::Capacity => "session quota or audit capacity is unavailable; no success is assumed",
            Self::Unavailable => "session lifecycle control is unavailable; schemas11–13 are supported, native Source17 requires its dedicated broker port",
            Self::ReceptionUnknown => "session control ACK was lost or unavailable; reception is unknown, retry with the same intent or retained abandonment UUID",
            Self::InvalidAck => "session control ACK failed exact validation; reception is unknown, preserve the same request and journal",
            Self::NoRetainedAbandonment => "no abandonment is retained and this incarnation cannot create one; use the authorized control recovery procedure",
            Self::Output => "session report could not be safely written; preserve the journal and retry status",
        })
    }
}
impl std::error::Error for SessionCliErrorV2 {}
impl From<SessionControlErrorV2> for SessionCliErrorV2 {
    fn from(error: SessionControlErrorV2) -> Self {
        use SessionControlErrorV2 as E;
        match error {
            E::InvalidInput => Self::Intent,
            E::Scope | E::Authentication => Self::Identity,
            E::Forbidden => Self::Forbidden,
            E::NotFound => Self::NotFound,
            E::Conflict => Self::Conflict,
            E::Capacity => Self::Capacity,
            E::Unavailable => Self::Unavailable,
            E::Protocol => Self::InvalidAck,
            E::ReceptionUnknown => Self::ReceptionUnknown,
        }
    }
}
impl From<RemoteSessionErrorV2> for SessionCliErrorV2 {
    fn from(error: RemoteSessionErrorV2) -> Self {
        match error {
            RemoteSessionErrorV2::Control(error) => error.into(),
            RemoteSessionErrorV2::Journal(_) => Self::Journal,
            RemoteSessionErrorV2::NoRetainedAbandonment => Self::NoRetainedAbandonment,
            RemoteSessionErrorV2::WorkerUnavailable => Self::Unavailable,
        }
    }
}

struct Loaded {
    client: SessionControlClientV2,
    root: Directory,
    protection: SessionJournalProtection,
    reopen_protection: SessionJournalProtection,
    redactor: Redactor,
    intent: Option<KubernetesSessionReservationIntentV2>,
}
fn load(connection: Connection, intent_file: Option<PathBuf>) -> Result<Loaded, SessionCliErrorV2> {
    let source = PrivateFile::open(&connection.control_file, 16 * 1024)
        .map_err(|_| SessionCliErrorV2::Configuration)?;
    let value = crate::json::decode_unique(source.bytes(), 16)
        .map_err(|_| SessionCliErrorV2::Configuration)?;
    let config: SessionControlConfigV2 =
        serde_json::from_value(value).map_err(|_| SessionCliErrorV2::Configuration)?;
    if config.version != 2
        || !config.broker_token_file.is_absolute()
        || config
            .gateway_ca_file
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
    {
        return Err(SessionCliErrorV2::Configuration);
    }
    let token = PrivateFile::open(&config.broker_token_file, 4097)
        .map_err(|_| SessionCliErrorV2::Configuration)?;
    let bearer =
        std::str::from_utf8(token.bytes()).map_err(|_| SessionCliErrorV2::Configuration)?;
    let secret = bearer.strip_suffix('\n').unwrap_or(bearer).to_owned();
    let ca = config
        .gateway_ca_file
        .as_ref()
        .map(|p| PrivateFile::open(p, 64 * 1024).map_err(|_| SessionCliErrorV2::Configuration))
        .transpose()?;
    let mut files = vec![source, token.clone()];
    if let Some(ca) = &ca {
        files.push(ca.clone());
    }
    let secrets = vec![secret];
    let redactor = Redactor::new(secrets.clone());
    let client = SessionControlClientV2::new(
        HeldCreateClientScope {
            gateway: config.gateway,
            tenant_id: config.tenant_id,
            control_namespace_uuid: config.control_namespace_uuid,
            broker_id: config.broker_id,
            credential_id: config.credential_id,
            incarnation_id: config.incarnation_id,
        },
        token,
        ca,
        secrets.clone(),
    )
    .map_err(|_| SessionCliErrorV2::Configuration)?;
    let preparing = intent_file.is_some();
    let intent = intent_file
        .map(|path| {
            let file =
                PrivateFile::open(&path, 64 * 1024).map_err(|_| SessionCliErrorV2::Intent)?;
            let value = crate::json::decode_unique(file.bytes(), 64)
                .map_err(|_| SessionCliErrorV2::Intent)?;
            if redactor.value_unclipped(&value) != value {
                return Err(SessionCliErrorV2::Intent);
            }
            let intent = KubernetesSessionReservationIntentV2::retained_from_value(value)
                .map_err(|_| SessionCliErrorV2::Intent)?;
            files.push(file);
            Ok(intent)
        })
        .transpose()?;
    let root = if preparing {
        Directory::open_or_create(&connection.journal_dir)
    } else {
        Directory::open_existing(&connection.journal_dir)
    }
    .map_err(|_| SessionCliErrorV2::Journal)?;
    let reopen_protection = SessionJournalProtection::new(files.clone(), secrets.clone())
        .map_err(|_| SessionCliErrorV2::Configuration)?;
    let protection = SessionJournalProtection::new(files, secrets)
        .map_err(|_| SessionCliErrorV2::Configuration)?;
    Ok(Loaded {
        client,
        root,
        protection,
        reopen_protection,
        redactor,
        intent,
    })
}

#[derive(Serialize)]
struct Report<'a> {
    version: u32,
    reservation_id: &'a str,
    local_phase: SessionPhaseV2,
    central_state: Option<&'a KubernetesSessionReservationStateDataV2>,
    execution_available: bool,
    runtime_state: &'static str,
}
fn print_report(report: &Report<'_>, redactor: &Redactor) -> Result<(), SessionCliErrorV2> {
    let value = serde_json::to_value(report).map_err(|_| SessionCliErrorV2::Output)?;
    if redactor.value_unclipped(&value) != value {
        return Err(SessionCliErrorV2::Output);
    }
    let mut bytes = serde_json::to_vec(&value).map_err(|_| SessionCliErrorV2::Output)?;
    if bytes.len() > 8192 {
        return Err(SessionCliErrorV2::Output);
    }
    bytes.push(b'\n');
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| SessionCliErrorV2::Output)
}
pub(super) async fn execute(args: SessionArgs) -> Result<(), SessionCliErrorV2> {
    let (connection, intent_file, action) = match args.command {
        SessionCommand::Prepare {
            connection,
            intent_file,
        } => (connection, Some(intent_file), "prepare"),
        SessionCommand::Abandon { connection } => (connection, None, "abandon"),
        SessionCommand::Status { connection } => (connection, None, "status"),
    };
    let permit = Arc::new(Semaphore::new(1))
        .try_acquire_owned()
        .map_err(|_| SessionCliErrorV2::Capacity)?;
    let loaded = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        load(connection, intent_file)
    })
    .await
    .map_err(|_| SessionCliErrorV2::Unavailable)??;
    let Loaded {
        client,
        root,
        protection,
        reopen_protection,
        redactor,
        intent,
    } = loaded;
    if let Some(intent) = intent {
        let id = intent.reservation_id().to_owned();
        let journal = remote::prepare(&client, root, protection, intent).await?;
        let permit = client.filesystem_permit()?;
        let phase = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            journal.phase()
        })
        .await
        .map_err(|_| SessionCliErrorV2::Unavailable)?
        .map_err(|_| SessionCliErrorV2::Journal)?;
        return print_report(
            &Report {
                version: 2,
                reservation_id: &id,
                local_phase: phase,
                central_state: None,
                execution_available: false,
                runtime_state: "not_observed",
            },
            &redactor,
        );
    }
    let view = remote::inspect(&client, root.clone(), protection).await?;
    if action == "status" {
        let current = client.observe(view.intent()).await?;
        return print_report(
            &Report {
                version: 2,
                reservation_id: view.intent().reservation_id(),
                local_phase: view.phase(),
                central_state: Some(&current.snapshot.state),
                execution_available: false,
                runtime_state: "not_observed",
            },
            &redactor,
        );
    }
    let close = match view.abandonment() {
        Some(close) => close.clone(),
        None => {
            let ids = view
                .intent()
                .ids()
                .map_err(|_| SessionCliErrorV2::Journal)?;
            if ids.incarnation_id.to_string() != client.scope().incarnation_id {
                return Err(SessionCliErrorV2::NoRetainedAbandonment);
            }
            let confirmed = client.confirm(view.intent()).await?;
            let intent = view.intent().clone();
            let permit = client.filesystem_permit()?;
            let journal = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                crate::runtime::kubernetes::session::SessionJournal::prepare_or_resume(
                    root,
                    reopen_protection,
                    &intent,
                    &confirmed,
                )
            })
            .await
            .map_err(|_| SessionCliErrorV2::Unavailable)?
            .map_err(|_| SessionCliErrorV2::Journal)?;
            remote::retain_abandonment(&client, journal).await?
        }
    };
    let result = client.abandon(view.intent(), &close).await?;
    print_report(
        &Report {
            version: 2,
            reservation_id: view.intent().reservation_id(),
            local_phase: SessionPhaseV2::Abandoned,
            central_state: Some(&result.snapshot.state),
            execution_available: false,
            runtime_state: "not_observed",
        },
        &redactor,
    )
}
