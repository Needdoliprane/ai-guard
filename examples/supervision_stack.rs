//! Test harness, not a product entry point and not a trial path.
//!
//! Serves the real gateway and dashboard routers on the loopback of the
//! process over the synthetic supervision data set, so the browser tests of
//! `front/tests/browser_supervision.mjs` exercise the Rust API instead of
//! JavaScript fixtures. Credentials are generated at start, printed once on
//! stdout for the parent test and never written to disk. No runtime, broker
//! process or model is started. `cargo build --release` and the product
//! image never compile this file.
//!
//! Protocol: the first stdout line is `{"ready":true,...}`; each stdin line is
//! one closed JSON command, acknowledged by one stdout line.
#[path = "../tests/support/supervision_seed.rs"]
mod supervision_seed;

use agentsandbox::identity::{HumanCapabilityV1, ResourceKindV1};
use anyhow::Context;
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
use supervision_seed::{Seed, SeedOptions, post_heartbeat, resource_event, serve};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_COMMAND_BYTES: usize = 4096;
const MAX_APPENDED_EVENTS: usize = 500;

#[derive(Parser)]
#[command(about = "Test-only supervision stack over synthetic data")]
struct Arguments {
    /// Absolute path of a built `front/dist`.
    #[arg(long)]
    front_dir: PathBuf,
    /// Absolute, existing, empty directory with mode 0700 for the test stores.
    #[arg(long)]
    state_dir: PathBuf,
    #[arg(long, default_value_t = 120)]
    workers: usize,
    #[arg(long, default_value_t = 1500)]
    events: usize,
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    SetCapabilities {
        account: String,
        capabilities: Vec<HumanCapabilityV1>,
    },
    RevokeCredential {
        account: String,
    },
    AppendEvent {
        resource_kind: ResourceKindV1,
        resource_id: String,
        count: Option<usize>,
    },
    RevokeBroker {
        broker: String,
    },
    Heartbeat,
    Stop,
}

fn checked_directories(arguments: &Arguments) -> anyhow::Result<()> {
    anyhow::ensure!(
        arguments.front_dir.is_absolute() && arguments.front_dir.join("index.html").is_file(),
        "--front-dir must be an absolute directory holding index.html"
    );
    anyhow::ensure!(
        arguments.state_dir.is_absolute(),
        "--state-dir must be absolute"
    );
    let metadata = std::fs::symlink_metadata(&arguments.state_dir)
        .context("--state-dir must already exist")?;
    anyhow::ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o777 == 0o700,
        "--state-dir must be a directory with mode 0700"
    );
    anyhow::ensure!(
        std::fs::read_dir(&arguments.state_dir)?.next().is_none(),
        "--state-dir must be empty: existing data is never reused or replaced"
    );
    Ok(())
}

fn ready(seed: &Seed, dashboard: &str) -> Value {
    let accounts: serde_json::Map<String, Value> = seed
        .accounts
        .iter()
        .map(|(name, account)| ((*name).to_owned(), json!(account.token)))
        .collect();
    let brokers: serde_json::Map<String, Value> = seed
        .brokers
        .iter()
        .map(|(name, broker)| ((*name).to_owned(), json!(broker.broker_id)))
        .collect();
    json!({
        "ready": true,
        "dashboard": dashboard,
        "accounts": accounts,
        "ids": {
            "tenant_a": {
                "workers": seed.worker_ids_a,
                "brokers": brokers,
                "unregistered_authority": seed.unregistered_authority,
                "session_owned_by_user_a1": seed.session_owned_by_user_a1,
                "session_shared_with_user_a2": seed.session_shared_with_user_a2,
                "session_owned_by_user_a2": seed.session_owned_by_user_a2,
                "session_without_grant": seed.session_without_grant,
                "granted_sessions": seed.granted_sessions_a,
            },
            "tenant_b": {
                "workers": seed.worker_ids_b,
                "brokers": seed.broker_ids_b,
                "session": seed.session_b,
            },
        },
    })
}

async fn apply(
    command: Command,
    seed: &mut Seed,
    served: &supervision_seed::Served,
) -> anyhow::Result<Value> {
    match command {
        Command::SetCapabilities {
            account,
            capabilities,
        } => {
            let revision = seed.set_capabilities(&account, &capabilities)?;
            Ok(json!({ "membership_revision": revision }))
        }
        Command::RevokeCredential { account } => {
            seed.revoke_credential(&account)?;
            Ok(json!({}))
        }
        Command::AppendEvent {
            resource_kind,
            resource_id,
            count,
        } => {
            let count = count.unwrap_or(1);
            anyhow::ensure!(
                (1..=MAX_APPENDED_EVENTS).contains(&count) && resource_id.len() <= 256,
                "append_event accepts 1 to {MAX_APPENDED_EVENTS} events"
            );
            let mut ids = Vec::with_capacity(count);
            for index in 0..count {
                let stored = seed
                    .a
                    .append(resource_event(resource_kind, &resource_id, index))?;
                ids.push(stored.event.id);
            }
            Ok(json!({ "event_ids": ids }))
        }
        Command::RevokeBroker { broker } => {
            seed.revoke_broker(&broker)?;
            Ok(json!({}))
        }
        Command::Heartbeat => {
            post_heartbeat(seed, served).await?;
            Ok(json!({}))
        }
        Command::Stop => Ok(json!({ "stopping": true })),
    }
}

async fn emit(stdout: &mut tokio::io::Stdout, value: Value) -> anyhow::Result<()> {
    stdout.write_all(value.to_string().as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    checked_directories(&arguments)?;
    let mut seed = Seed::build(
        &arguments.state_dir,
        SeedOptions {
            workers: arguments.workers,
            events: arguments.events,
            ..SeedOptions::default()
        },
    )?;
    let served = serve(&seed, arguments.front_dir.clone()).await?;
    post_heartbeat(&mut seed, &served).await?;

    let mut stdout = tokio::io::stdout();
    emit(&mut stdout, ready(&seed, &served.dashboard)).await?;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_COMMAND_BYTES {
            let refusal = json!({ "ok": false, "error": "command exceeds its size bound" });
            emit(&mut stdout, refusal).await?;
            continue;
        }
        let command: Command = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(_) => {
                let refusal = json!({ "ok": false, "error": "unknown or malformed command" });
                emit(&mut stdout, refusal).await?;
                continue;
            }
        };
        let stop = matches!(command, Command::Stop);
        match apply(command, &mut seed, &served).await {
            Ok(mut result) => {
                result["ok"] = json!(true);
                emit(&mut stdout, result).await?;
            }
            Err(error) => {
                let refusal = json!({ "ok": false, "error": error.to_string() });
                emit(&mut stdout, refusal).await?;
            }
        }
        if stop {
            break;
        }
    }
    Ok(())
}
