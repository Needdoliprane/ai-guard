//! Opt-in Linux browser QA over the real owned gateway and dashboard.
//! No business runtime, external service, debug HTTP route or mock backend.
#![cfg(test)]
use super::*;
use crate::api::human_sources::owned_control_tests::negative::reserve_workload_fixture_worker;
use crate::audit::identity::projection::Record as IdentityAuditRecord;
use crate::control::{QuotaPolicyV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1};
use crate::control_fs::{Directory, PrivateFile};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};

#[path = "browser_fixture/egress.rs"]
mod egress;

const ENABLE: &str = "AI_GUARD_OWNED_BROWSER_FIXTURE";
const WORKER_MODE: &str = "run-workers-v1";
const USAGE_MODE: &str = "run-usage-v1";
const MEMBERSHIP_MODE: &str = "run-membership-v1";
const DIST: &str = "AI_GUARD_OWNED_BROWSER_DIST";
const CONTROL: &str = "AI_GUARD_OWNED_BROWSER_CONTROL";
const SECONDS: &str = "AI_GUARD_OWNED_BROWSER_SECONDS";
const COMMAND_LIMIT: u16 = 32;
const COMMAND_BYTES: usize = 512;
const RECEIPT_LIMIT: usize = 1_000;
const WORKER_RECEIPT_KIND: &str = "control.identity.workers.access";
const OUTPUT_BYTES: usize = 512 * 1024;
const RESOURCE: &str = "owned-browser-session-b";
const MEMBER_DISPLAY_NAME: &str = "Synthetic browser member";
const PRIVATE_CONTENT: &str = "owned-browser-private-content-must-not-appear-in-metadata";
const CLOSE_RESERVE: Duration = Duration::from_secs(20);

struct Config {
    dist: PathBuf,
    root: Directory,
    commands: Directory,
    acknowledgements: Directory,
    seconds: u64,
}

impl Config {
    fn load(opt_in: &str) -> Result<Self> {
        ensure!(
            std::env::var(ENABLE).is_ok_and(|mode| mode == opt_in),
            "owned browser fixture requires its explicit opt-in"
        );
        let dist = absolute_env(DIST)?;
        ensure!(
            std::fs::canonicalize(&dist)? == dist,
            "frontend dist must have no path aliases or symbolic links"
        );
        let index = std::fs::symlink_metadata(dist.join("index.html"))?;
        ensure!(
            index.is_file() && !index.file_type().is_symlink(),
            "frontend dist requires a real index.html"
        );
        let control = absolute_env(CONTROL)?;
        let temporary = std::fs::canonicalize(std::env::temp_dir())?;
        ensure!(
            control != temporary && control.starts_with(&temporary),
            "control root must be dedicated under TMPDIR"
        );
        ensure!(
            !control.starts_with(&dist) && !dist.starts_with(&control),
            "private controls and served dist must be separate"
        );
        let root = Directory::open_existing(&control)?;
        ensure!(
            root.persistent_entries(1)?.is_empty(),
            "control root must be new and empty"
        );
        let seconds = match std::env::var(SECONDS) {
            Ok(value) => value.parse::<u64>().context("invalid fixture seconds")?,
            Err(std::env::VarError::NotPresent) => 180,
            Err(_) => bail!("fixture seconds must be UTF-8"),
        };
        ensure!(
            (30..=600).contains(&seconds),
            "fixture seconds must be between 30 and 600"
        );
        let commands = root
            .persistent_child("commands", true)?
            .context("command directory absent")?;
        let acknowledgements = root
            .persistent_child("acks", true)?
            .context("ack directory absent")?;
        Ok(Self {
            dist,
            root,
            commands,
            acknowledgements,
            seconds,
        })
    }
}

fn absolute_env(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name).context("required fixture path is absent")?);
    ensure!(path.is_absolute(), "fixture paths must be absolute");
    let normalized = crate::control_fs::absolute_path(&path)?;
    ensure!(
        path == normalized,
        "fixture paths must be normalized without parent traversal"
    );
    Ok(path)
}

fn publish_bytes(directory: &Directory, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
    ensure!(
        bytes.len() <= OUTPUT_BYTES,
        "fixture output exceeds its bound"
    );
    let file = directory.persistent_create(name, bytes)?;
    file.persistent_sync()?;
    directory.sync()?;
    Ok(file)
}

fn publish(directory: &Directory, name: &str, value: &impl Serialize) -> Result<PrivateFile> {
    publish_bytes(directory, name, &serde_json::to_vec(value)?)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    LockOwnSqlite,
    UnlockOwnSqlite,
    Stop,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    version: u32,
    sequence: u16,
    action: Action,
}

fn decode(bytes: &[u8], expected: u16) -> Result<Command> {
    ensure!(
        bytes.len() <= COMMAND_BYTES,
        "fixture command exceeds 512 bytes"
    );
    // Serde diagnostics can echo an unknown action value. Do not propagate
    // untrusted command bytes into libtest stdout/stderr.
    let command: Command = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid closed fixture command"))?;
    ensure!(
        command.version == 1
            && command.sequence == expected
            && (1..=COMMAND_LIMIT).contains(&expected),
        "invalid fixture command version or sequence"
    );
    Ok(command)
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum AckStatus {
    Locked,
    Unlocked,
    Stopping,
    Rejected,
}

#[derive(Serialize)]
struct Ack<'a> {
    version: u32,
    sequence: u16,
    action: Action,
    command_sha256: &'a str,
    status: AckStatus,
    own_sqlite_locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

#[derive(Default)]
struct OwnSqliteLock(Option<rusqlite::Connection>);

impl OwnSqliteLock {
    fn acquire(&mut self, path: &Path) -> Result<(AckStatus, Option<&'static str>)> {
        if self.0.is_some() {
            return Ok((AckStatus::Rejected, Some("already_locked")));
        }
        let connection = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::ZERO)?;
        if let Err(error) = connection.execute_batch("BEGIN IMMEDIATE") {
            if matches!(&error, rusqlite::Error::SqliteFailure(code, _) if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
            {
                return Ok((AckStatus::Rejected, Some("own_sqlite_busy")));
            }
            return Err(error.into());
        }
        ensure!(
            !connection.is_autocommit(),
            "fixture lock was not established"
        );
        self.0 = Some(connection);
        Ok((AckStatus::Locked, None))
    }

    fn release(&mut self) -> Result<bool> {
        let Some(connection) = self.0.take() else {
            return Ok(false);
        };
        connection.execute_batch("ROLLBACK")?;
        ensure!(connection.is_autocommit(), "fixture lock was not released");
        Ok(true)
    }
}

impl Drop for OwnSqliteLock {
    fn drop(&mut self) {
        if let Some(connection) = self.0.take() {
            let _ = connection.execute_batch("ROLLBACK");
        }
    }
}

struct Servers {
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<std::io::Result<()>>>,
    human: GatewayHumanRetirement,
    control: GatewayControlRecoveryRetirementV2,
    closed: bool,
}

impl Servers {
    fn new(state: &GatewayState) -> Result<Self> {
        let (stop, _) = watch::channel(false);
        Ok(Self {
            stop,
            tasks: Vec::new(),
            human: state
                .human_retirement()
                .context("owned retirement absent")?,
            control: state.control_recovery_retirement(),
            closed: false,
        })
    }

    async fn bind(&mut self, router: Router) -> Result<std::net::SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let mut stopped = self.stop.subscribe();
        self.tasks.push(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.wait_for(|stop| *stop).await;
                })
                .await
        }));
        Ok(address)
    }

    async fn close(&mut self) -> Result<()> {
        self.human.retire();
        self.control.retire();
        self.stop.send_replace(true);
        let mut failed = false;
        for task in &mut self.tasks {
            match tokio::time::timeout(Duration::from_secs(5), &mut *task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(_) => {
                    failed = true;
                }
                Err(_) => {
                    failed = true;
                    task.abort();
                    let _ = tokio::time::timeout(Duration::from_secs(1), &mut *task).await;
                }
            }
        }
        self.tasks.clear();
        let drain = tokio::time::timeout(Duration::from_secs(5), self.human.drain()).await;
        self.closed = !failed && matches!(drain, Ok(Ok(()))) && self.control.drained();
        ensure!(
            self.closed,
            "fixture server shutdown or real owner drain was not established"
        );
        Ok(())
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        self.human.retire();
        self.control.retire();
        self.stop.send_replace(true);
        // Emergency cleanup never leaves a listening test task detached. It
        // does not manufacture CLOSED or claim the real owner drain succeeded.
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FixtureMode {
    Journal,
    Workers,
    Usage,
    Membership,
}

enum FixtureSeed {
    Journal(Vec<String>),
    Worker {
        worker_id: String,
        attempt_id: String,
        session_id: String,
    },
    Usage {
        worker_id: String,
        attempt_id: String,
        terminal_observation_id: String,
        unmeasured_worker_id: String,
        unmeasured_attempt_id: String,
    },
    Membership(Box<MembershipSeed>),
}

struct MembershipSeed {
    member: MembershipV1,
    baseline: IdentityReceiptV1,
}

fn fixture_identity_mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-browser-fixture-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn seed_membership(
    store: &Store,
    administrator: &PrincipalRefV1,
    organization_scope: String,
) -> Result<FixtureSeed> {
    let PrincipalRefV1::HumanLocal {
        installation_id, ..
    } = administrator
    else {
        bail!("browser membership administrator is not a local identity");
    };
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: installation_id.clone(),
        account_id: uuid::Uuid::new_v4().to_string(),
    };
    store.mutate_identity(&fixture_identity_mutation(IdentityChangeV1::Account {
        expected_revision: 0,
        account: HumanAccountV1 {
            version: 1,
            principal: principal.clone(),
            display_name: MEMBER_DISPLAY_NAME.into(),
            revision: 1,
            active: true,
        },
    }))?;
    let member = MembershipV1 {
        version: 1,
        membership_id: uuid::Uuid::new_v4().to_string(),
        principal,
        tenant_id: "B".into(),
        organization_scope,
        revision: 1,
        active: true,
        capabilities: [
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadContent,
        ]
        .into_iter()
        .collect(),
    };
    let baseline =
        store.mutate_identity(&fixture_identity_mutation(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: member.clone(),
        }))?;
    Ok(FixtureSeed::Membership(Box::new(MembershipSeed {
        member,
        baseline,
    })))
}

fn seed_usage(store: &Store) -> Result<FixtureSeed> {
    // Supply the missing terminal time through the same Store transition.
    // Runtime observations here belong only to the software fixture.
    let worker = store.finish_usage_worker_for_test(None).worker;
    let mut observation = worker
        .last_observation
        .context("usage fixture observation absent")?;
    let started = chrono::DateTime::parse_from_rfc3339(
        observation
            .started_at
            .as_deref()
            .context("usage fixture start absent")?,
    )?;
    observation.finished_at = Some(
        (started + chrono::Duration::nanoseconds(250_000))
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
    );
    observation.observed_at = chrono::Utc::now().to_rfc3339();
    let terminal = store.mutate_worker(&WorkerMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        worker_id: worker.worker_id,
        attempt_id: worker.attempt.attempt_id,
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::Observe { observation },
    })?;
    ensure!(
        terminal.worker.state == WorkerStateV1::Terminal && !terminal.worker.quota_held,
        "usage fixture did not record a terminal allocation"
    );

    let unmeasured = store.finish_usage_worker_for_test(None).worker;
    ensure!(
        unmeasured.state == WorkerStateV1::InDoubt && unmeasured.quota_held,
        "usage fixture unmeasured attempt did not retain its reservation"
    );
    Ok(FixtureSeed::Usage {
        worker_id: terminal.worker.worker_id,
        attempt_id: terminal.worker.attempt.attempt_id,
        terminal_observation_id: terminal.receipt.operation_id,
        unmeasured_worker_id: unmeasured.worker_id,
        unmeasured_attempt_id: unmeasured.attempt.attempt_id,
    })
}

fn seed(mode: FixtureMode) -> Result<(Fixture, FixtureSeed)> {
    let fixture = if matches!(mode, FixtureMode::Usage | FixtureMode::Membership) {
        Fixture::new_owned_with_human_rights()
    } else {
        Fixture::new_owned()
    };
    let store = fixture.legacy_store("B");
    let mut membership = store
        .identity_memberships(fixture.principal())?
        .into_iter()
        .find(|member| member.tenant_id == "B" && member.organization_scope == "synthetic-org")
        .context("synthetic B membership absent")?;
    let organization_scope = membership.organization_scope.clone();
    let expected_revision = membership.revision;
    membership.revision = membership
        .revision
        .checked_add(1)
        .context("synthetic membership revision overflow")?;
    match mode {
        FixtureMode::Workers | FixtureMode::Usage => {
            membership.capabilities = [HumanCapabilityV1::ReadWorkerRegistry]
                .into_iter()
                .collect();
        }
        FixtureMode::Membership => {
            membership.capabilities = [
                HumanCapabilityV1::ManageMemberships,
                HumanCapabilityV1::ReadMemberships,
            ]
            .into_iter()
            .collect();
        }
        FixtureMode::Journal => {
            // The Journal tests profile refusal even when the owner holds the right.
            membership.capabilities.insert(HumanCapabilityV1::Export);
        }
    }
    store.mutate_identity(&fixture_identity_mutation(IdentityChangeV1::Membership {
        expected_revision,
        membership,
    }))?;
    if mode == FixtureMode::Membership {
        let seeded = seed_membership(store, fixture.principal(), organization_scope)?;
        return Ok((fixture, seeded));
    }
    if mode != FixtureMode::Journal {
        let supervision = store.supervision_state()?;
        ensure!(
            supervision.schema_version == 13 && supervision.active,
            "worker fixture requires the active pre-adoption Store"
        );
        let seeded = if mode == FixtureMode::Usage {
            seed_usage(store)?
        } else {
            let worker = reserve_workload_fixture_worker(store);
            FixtureSeed::Worker {
                worker_id: worker.worker_id,
                attempt_id: worker.attempt.attempt_id,
                session_id: worker.launch.session_id,
            }
        };
        return Ok((fixture, seeded));
    }
    let mut ids = Vec::new();
    for ordinal in [1, 2] {
        let mut event = Event::new("fixture.owned-browser");
        event.session_id = Some(RESOURCE.into());
        event.data = json!({"private_content": PRIVATE_CONTENT, "ordinal": ordinal});
        ids.push(store.append(event)?.event.id);
    }
    store.mutate_identity(&fixture_identity_mutation(
        IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: "B".into(),
                organization_scope: "synthetic-org".into(),
                resource: ResourceRefV1 {
                    kind: ResourceKindV1::Session,
                    resource_id: RESOURCE.into(),
                },
                owner: fixture.principal().clone(),
                revision: 1,
                active: true,
                shares: Vec::new(),
            },
        },
    ))?;
    Ok((fixture, FixtureSeed::Journal(ids)))
}

async fn control(
    config: &Config,
    database: &Path,
    deadline: Instant,
    servers: &Servers,
    lock: &mut OwnSqliteLock,
) -> Result<u16> {
    let mut accepted = Vec::<PrivateFile>::new();
    let mut sequence = 1;
    loop {
        ensure!(
            Instant::now() < deadline,
            "fixture control deadline exceeded"
        );
        ensure!(
            !servers.tasks.iter().any(JoinHandle::is_finished),
            "fixture HTTP server stopped unexpectedly"
        );
        for file in &accepted {
            file.revalidate()?;
        }
        let name = format!("{sequence:04}.json");
        let Some(file) = config
            .commands
            .persistent_read(&name, COMMAND_BYTES as u64, &[])?
        else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let command = decode(file.bytes(), sequence)?;
        file.revalidate()?;
        let (status, code) = match command.action {
            Action::LockOwnSqlite => lock.acquire(database)?,
            Action::UnlockOwnSqlite => {
                if lock.release()? {
                    (AckStatus::Unlocked, None)
                } else {
                    (AckStatus::Rejected, Some("not_locked"))
                }
            }
            Action::Stop => {
                lock.release()?;
                (AckStatus::Stopping, None)
            }
        };
        file.revalidate()?;
        publish(
            &config.acknowledgements,
            &name,
            &Ack {
                version: 1,
                sequence,
                action: command.action,
                command_sha256: file.digest(),
                status,
                own_sqlite_locked: lock.0.is_some(),
                code,
            },
        )?;
        accepted.push(file);
        if matches!(command.action, Action::Stop) {
            return Ok(sequence);
        }
        ensure!(
            sequence < COMMAND_LIMIT,
            "fixture command limit reached without stop"
        );
        sequence += 1;
    }
}

#[derive(Serialize)]
struct Receipt {
    audit_event_id: String,
    request_id: String,
    action: String,
    received_at: String,
}

fn open_receipt_database(path: &Path) -> Result<rusqlite::Connection> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::ZERO)?;
    Ok(connection)
}

#[derive(Serialize)]
struct MembershipChangeProof {
    mutation_count: u8,
    receipt: Option<HumanMembershipChangeReceiptV1>,
}

fn membership_change_proof(
    path: &Path,
    seed: &MembershipSeed,
    administrator: &PrincipalRefV1,
) -> Result<MembershipChangeProof> {
    let connection = open_receipt_database(path)?;
    let mut statement = connection.prepare(
        "SELECT seq,payload FROM events WHERE kind='control.identity.mutation' AND seq>?1
         ORDER BY seq LIMIT 2",
    )?;
    let mut rows = statement.query([seed.baseline.audit_seq])?;
    let Some(row) = rows.next()? else {
        return Ok(MembershipChangeProof {
            mutation_count: 0,
            receipt: None,
        });
    };
    let audit_seq: i64 = row.get(0)?;
    let payload: String = row.get(1)?;
    ensure!(
        rows.next()?.is_none(),
        "more than one identity mutation after seed"
    );
    ensure!(
        payload.len() <= OUTPUT_BYTES,
        "membership audit exceeds its bound"
    );
    let event: Event = serde_json::from_str(&payload)?;
    let record = IdentityAuditRecord::decode_replay(&event.data)?;
    let mut expected = seed.member.clone();
    expected.revision = 2;
    expected
        .capabilities
        .remove(&HumanCapabilityV1::ReadContent);
    ensure!(
        record.mutation.actor == *administrator
            && record.mutation.change
                == (IdentityChangeV1::Membership {
                    expected_revision: 1,
                    membership: expected.clone(),
                })
            && seed.baseline.authorization_revision.checked_add(1)
                == Some(record.authorization_revision),
        "membership audit differs from the single delegated change"
    );
    let indexed: (i64, String) = connection.query_row(
        "SELECT event_seq,input_hash FROM identity_operations WHERE operation_id=?1",
        [&record.mutation.operation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (projected_seq, member_json): (i64, String) = connection.query_row(
        "SELECT event_seq,payload_json FROM identity_memberships WHERE membership_id=?1",
        [&expected.membership_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(
        indexed == (audit_seq, record.input_hash)
            && projected_seq == audit_seq
            && serde_json::from_str::<MembershipV1>(&member_json)? == expected,
        "membership operation index or projection differs from the audit"
    );
    Ok(MembershipChangeProof {
        mutation_count: 1,
        receipt: Some(HumanMembershipChangeReceiptV1 {
            version: 1,
            operation_id: record.mutation.operation_id,
            membership_id: expected.membership_id,
            membership_revision: expected.revision.to_string(),
            authorization_revision: record.authorization_revision.to_string(),
            audit_seq: audit_seq.to_string(),
        }),
    })
}

fn export_receipts(root: &Directory, path: &Path, tenant: &str) -> Result<()> {
    let connection = open_receipt_database(path)?;
    // Worker registry receipts carry their own event kind instead of data.action.
    let mut statement = connection.prepare(
        "SELECT id,json_extract(payload,'$.data.request_id'),
                CASE kind WHEN ?1 THEN kind
                ELSE json_extract(payload,'$.data.action') END,received_at
         FROM events WHERE kind IN ('control.identity.access',?1)
         ORDER BY seq LIMIT 1001",
    )?;
    let rows = statement.query_map([WORKER_RECEIPT_KIND], |row| {
        Ok(Receipt {
            audit_event_id: row.get(0)?,
            request_id: row.get(1)?,
            action: row.get(2)?,
            received_at: row.get(3)?,
        })
    })?;
    let mut receipts = Vec::new();
    for row in rows {
        let row = row?;
        ensure!(
            row.audit_event_id.len() <= 64
                && row.request_id.len() <= 64
                && row.received_at.len() <= 64
                && [
                    "identity",
                    "resources",
                    "events",
                    "event",
                    "summary",
                    "quota",
                    "usage",
                    "memberships",
                    WORKER_RECEIPT_KIND
                ]
                .contains(&row.action.as_str()),
            "invalid bounded access receipt"
        );
        receipts.push(row);
    }
    let complete = receipts.len() <= RECEIPT_LIMIT;
    receipts.truncate(RECEIPT_LIMIT);
    publish(
        root,
        "RECEIPTS.json",
        &json!({"version":1,"tenant_id":tenant,"complete":complete,"limit":RECEIPT_LIMIT,"records":receipts}),
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit owned browser QA only; requires private control root and admitted frontend dist"]
async fn serve_owned_browser_fixture() -> Result<()> {
    let started = Instant::now();
    let (mode, opt_in) = match std::env::var(ENABLE).as_deref() {
        Ok("run-v1") => (FixtureMode::Journal, "run-v1"),
        Ok(WORKER_MODE) => (FixtureMode::Workers, WORKER_MODE),
        Ok(USAGE_MODE) => (FixtureMode::Usage, USAGE_MODE),
        Ok(MEMBERSHIP_MODE) => (FixtureMode::Membership, MEMBERSHIP_MODE),
        _ => bail!("owned browser fixture requires its explicit opt-in"),
    };
    let config = Config::load(opt_in)?;
    let deadline = started + Duration::from_secs(config.seconds) - CLOSE_RESERVE;
    let (fixture, seeded) = seed(mode)?;
    let (bundle, credentials, tokens) = fixture.owned_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )?;
    let state = if mode == FixtureMode::Usage {
        state.with_worker_quotas(vec![(
            "B".into(),
            QuotaPolicyV1 {
                version: 1,
                max_active_workers: 5,
                max_cpu_millis: 7000,
                max_memory_bytes: 4 * 1024 * 1024 * 1024,
                max_timeout_seconds: 7200,
            },
        )])?
    } else {
        state
    };
    let source = state
        .owned_human
        .as_ref()
        .context("owned source absent")?
        .sources[1]
        .descriptor();
    if mode != FixtureMode::Journal {
        ensure!(
            source.tenant_id.as_deref() == Some("B") && source.required_format == 14,
            "administration fixture requires the retained tenant B Store14"
        );
    }
    let database = source.storage_binding.path();
    let mut servers = Servers::new(&state)?;
    let mut lock = OwnSqliteLock::default();
    let work = async {
        ensure!(Instant::now() < deadline, "fixture setup exhausted its budget");
        let gateway = servers.bind(gateway_router(state.clone())).await?;
        let dashboard = dashboard_router_with_human_mode(&format!("http://{gateway}"), None, config.dist.clone(), None, true)?;
        let dashboard = servers.bind(dashboard).await?;
        publish_bytes(&config.root, "BEARER.txt", Fixture::bearer("B").as_bytes())?;
        let mut ready = json!({
            "version":1,"dashboard_url":format!("http://{dashboard}"),"bearer_b":Fixture::bearer("B"),
            "tenant_id":"B",
            "budget_seconds":config.seconds,"commands":"commands","acks":"acks","command_limit":COMMAND_LIMIT
        });
        match &seeded {
            FixtureSeed::Journal(event_ids) => {
                ready["resource"] = json!({"kind":"session","resource_id":RESOURCE});
                ready["event_ids"] = json!(event_ids);
            }
            FixtureSeed::Worker { worker_id, attempt_id, session_id } => {
                ready["mode"] = json!("worker_registry");
                ready["worker"] = json!({"worker_id":worker_id,"attempt_id":attempt_id,"session_id":session_id});
            }
            FixtureSeed::Usage {
                worker_id,
                attempt_id,
                terminal_observation_id,
                unmeasured_worker_id,
                unmeasured_attempt_id,
            } => {
                ready["mode"] = json!("usage");
                ready["usage"] = json!({
                    "allocated": {
                        "worker_id": worker_id,
                        "attempt_id": attempt_id,
                        "terminal_observation_id": terminal_observation_id,
                    },
                    "unmeasured": {
                        "worker_id": unmeasured_worker_id,
                        "attempt_id": unmeasured_attempt_id,
                    },
                });
            }
            FixtureSeed::Membership(seed) => {
                ready["mode"] = json!("membership");
                ready["membership"] = json!({
                    "membership_id": seed.member.membership_id,
                    "display_name": MEMBER_DISPLAY_NAME,
                    "revision": seed.member.revision.to_string(),
                });
            }
        }
        publish(&config.root, "READY.json", &ready)?;
        eprintln!("OWNED_BROWSER_READY {}", config.root.path().join("READY.json").display());
        control(&config, &database, deadline, &servers, &mut lock).await
    }.await;
    // All normal errors, protocol errors and deadline expiry take this same
    // cleanup path. Unlock before waiting for actual HTTP/blocking usages.
    let unlocked = lock.release().is_ok();
    let stopped = servers.close().await;
    let receipts = export_receipts(&config.root, &database, "B");
    let membership_change = match &seeded {
        FixtureSeed::Membership(seed) if stopped.is_ok() => {
            membership_change_proof(&database, seed, fixture.principal()).map(Some)
        }
        _ => Ok(None),
    };
    let receipts_exported = receipts.is_ok() && membership_change.is_ok();
    let mut closed = json!({
        "version":1,"completed":work.is_ok() && unlocked && stopped.is_ok() && receipts_exported,
        "commands_processed":work.as_ref().ok().copied(),"sqlite_lock_released":unlocked,
        "servers_and_owner_drained":stopped.is_ok(),"receipts_exported":receipts_exported,
        "stop_reason":if work.is_ok() {"requested"} else {"error_or_deadline"}
    });
    if mode == FixtureMode::Membership {
        closed["membership_change"] =
            json!(membership_change.as_ref().ok().and_then(Option::as_ref));
    }
    publish(&config.root, "CLOSED.json", &closed)?;
    eprintln!(
        "OWNED_BROWSER_CLOSED {}",
        config.root.path().join("CLOSED.json").display()
    );
    ensure!(unlocked, "fixture SQLite lock cleanup failed");
    work?;
    stopped?;
    receipts?;
    let _ = membership_change?;
    Ok(())
}

#[test]
fn closed_browser_commands_refuse_unknown_inputs_and_sequence_changes() {
    assert!(
        decode(
            br#"{"version":1,"sequence":1,"action":"lock_own_sqlite"}"#,
            1
        )
        .is_ok()
    );
    for bytes in [
        br#"{"version":1,"sequence":2,"action":"stop"}"#.as_slice(),
        br#"{"version":2,"sequence":1,"action":"stop"}"#.as_slice(),
        br#"{"version":1,"sequence":1,"action":"sql"}"#.as_slice(),
        br#"{"version":1,"sequence":1,"action":"stop","path":"/elsewhere"}"#.as_slice(),
        br#"{"version":1,"sequence":1,"action":"stop","tenant":"A"}"#.as_slice(),
    ] {
        assert!(decode(bytes, 1).is_err());
    }
    assert!(decode(&vec![b' '; COMMAND_BYTES + 1], 1).is_err());
    assert!(decode(br#"{"version":1,"sequence":33,"action":"stop"}"#, 33).is_err());
    let error = decode(
        br#"{"version":1,"sequence":1,"action":"synthetic-value-must-not-be-echoed"}"#,
        1,
    )
    .err()
    .unwrap();
    assert!(!format!("{error:#}").contains("synthetic-value-must-not-be-echoed"));
}
