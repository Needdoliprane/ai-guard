//! Synthetic data set for the unified supervision tests.
//!
//! Every row is produced by a public control mutation on private test SQLite
//! stores: real migrations, admissions, worker transitions, broker grants,
//! identity provisioning and audit appends. No runtime, broker process or
//! model is started, so worker states prove registry content only. This file
//! is shared by `tests/supervision_stack_api.rs` and
//! `examples/supervision_stack.rs`; it is never linked into the product.
#![allow(dead_code)]

use agentsandbox::{
    api::{GatewayState, TenantGateway},
    audit::Store,
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerHeartbeatV1, BrokerRevocationV1,
        BrokerRuntimeKindV1,
    },
    control::*,
    identity::*,
    model::{CommandSpec, Decision, DecisionRequest, Event, Outcome},
    policy::{self, Policy},
    redaction::Redactor,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use anyhow::Context;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
use uuid::Uuid;

pub const ORGANIZATION: &str = "supervision-test-organization";
pub const OTHER_ORGANIZATION: &str = "supervision-test-other-organization";
const INSTALLATION: &str = "909d9572-f1a2-4a53-ad8b-a741030a2b18";
/// Audit capacity of each test store. Human read budgets derive from it.
pub const MAX_EVENTS: u64 = 200_000;
const FAR_FUTURE: &str = "2100-01-01T00:00:00.000Z";

fn id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Clone, Copy)]
pub struct SeedOptions {
    /// Registry rows of tenant A. At most 100 keep their quota (product bound).
    pub workers: usize,
    /// Authorized events appended for tenant A, spread over its resources.
    pub events: usize,
    /// Cumulative human read receipts allowed per principal of tenant A.
    /// `None` keeps the automatic share of the store capacity.
    pub human_principal_events: Option<u64>,
}
impl Default for SeedOptions {
    fn default() -> Self {
        Self {
            workers: 120,
            events: 1500,
            human_principal_events: None,
        }
    }
}

pub struct Account {
    pub token: String,
    pub principal: PrincipalRefV1,
    pub credential: LocalCredentialV1,
    pub membership: MembershipV1,
}

pub struct BrokerSeed {
    pub broker_id: String,
    pub incarnation_id: String,
    pub credential_id: String,
    pub authority_id: String,
    pub grant_revision: u64,
    /// Present only for the broker that owns a workload binding.
    pub token: Option<String>,
    pub heartbeat_revision: u64,
    pub sequence: u64,
}

pub struct Seed {
    pub a: Arc<Store>,
    pub b: Arc<Store>,
    pub c: Arc<Store>,
    pub accounts: BTreeMap<&'static str, Account>,
    /// Tenant A: `current` (after its first heartbeat), `revoked`, `unseen`.
    pub brokers: BTreeMap<&'static str, BrokerSeed>,
    pub unregistered_authority: String,
    pub worker_ids_a: Vec<String>,
    pub worker_ids_b: Vec<String>,
    pub broker_ids_b: Vec<String>,
    /// Session shared by a worker, a resource grant and appended events.
    pub session_owned_by_user_a1: String,
    pub session_shared_with_user_a2: String,
    pub session_owned_by_user_a2: String,
    pub session_without_grant: String,
    pub session_b: String,
    pub granted_sessions_a: Vec<String>,
    runner_tokens: [String; 3],
    viewer_tokens: [String; 3],
}

fn operator() -> PrincipalRefV1 {
    PrincipalRefV1::LegacyControl {
        credential_id: "supervision-test-trusted-control".into(),
        role: LegacyControlRoleV1::Runner,
    }
}
fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: id(),
        actor: operator(),
        change,
    }
}

fn open(path: &Path, tenant: &str) -> anyhow::Result<Store> {
    Store::open_for_tenant(path, tenant, Redactor::new(vec![]), MAX_EVENTS)
        .with_context(|| format!("open the {tenant} test store"))
}
fn migrate(store: &Store, path: &Path) -> anyhow::Result<()> {
    store.migrate_control_v3(&path.with_extension("before3"))?;
    store.migrate_identity_v4(&path.with_extension("before4"))?;
    store.migrate_human_access_v5(&path.with_extension("before5"))?;
    store.migrate_workload_v6(&path.with_extension("before6"))?;
    store.migrate_human_brokers_v7(&path.with_extension("before7"))?;
    store.migrate_kubernetes_create_v8(&path.with_extension("before8"))?;
    store.migrate_kubernetes_policy_v9(&path.with_extension("before9"))?;
    store.migrate_human_workers_v10(&path.with_extension("before10"))?;
    Ok(())
}

fn quota() -> QuotaPolicyV1 {
    QuotaPolicyV1 {
        version: 1,
        max_active_workers: 100,
        max_cpu_millis: 200_000,
        max_memory_bytes: 100 * 1024 * 1024 * 1024,
        max_timeout_seconds: 3600,
    }
}

/// One real decision plus admission. `legacy` uses the schema-2 admission,
/// which is the only path whose later transitions need no supervision lease.
fn admit(store: &Store, authority: &str, session: &str, legacy: bool) -> anyhow::Result<WorkerV1> {
    let request = DecisionRequest {
        request_id: id(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        session_id: Some(session.into()),
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let decision = policy::evaluate(&policy, &request);
    let stored = store
        .decision(&request, Some((decision.clone(), json!(null))))?
        .context("the test decision must be recorded")?;
    let admission = AdmissionV1 {
        version: 1,
        operation_id: id(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id: session.into(),
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: authority.into(),
            image_ref: "worker:supervision-test".into(),
            image_id: format!("sha256:{}", "d".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "e".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        },
        decision_request: request,
    };
    let result = if legacy {
        store.admit_worker(&admission, &policy, decision, json!(null), &quota())?
    } else {
        let compact = Uuid::new_v4().simple().to_string();
        let supervised = SupervisedAdmissionV1 {
            version: 1,
            protocol: SupervisionProtocolV1::BrokerSupervisionV1,
            intent_hash: format!("{compact}{compact}"),
            admission,
        };
        supervised.validate()?;
        store.admit_supervised(&supervised, &policy, decision, json!(null), &quota())?
    };
    anyhow::ensure!(result.accepted, "the test admission was refused");
    result
        .worker
        .context("an accepted admission returns its worker")
}

fn step(
    store: &Store,
    worker: &WorkerV1,
    transition: WorkerTransitionV1,
) -> anyhow::Result<WorkerV1> {
    Ok(store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: id(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition,
        })?
        .worker)
}

fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

/// Historical schema-2 lifecycle up to the single start claim.
fn start(store: &Store, worker: WorkerV1) -> anyhow::Result<WorkerV1> {
    let mut worker = step(store, &worker, WorkerTransitionV1::Prepare)?;
    worker = step(store, &worker, WorkerTransitionV1::CreateIssued)?;
    let inspection = inspection(&worker);
    worker = step(store, &worker, WorkerTransitionV1::Created { inspection })?;
    step(
        store,
        &worker,
        WorkerTransitionV1::StartClaim { emitter_id: id() },
    )
}

enum Observed {
    Running,
    Exited,
    OomKilled,
    UnreadableStart,
}

fn observe(
    store: &Store,
    worker: &WorkerV1,
    kind: Observed,
    minutes_ago: i64,
) -> anyhow::Result<WorkerV1> {
    let at = |minutes: i64| {
        (chrono::Utc::now() - chrono::Duration::minutes(minutes))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    let started = at(minutes_ago + 5);
    let finished = at(minutes_ago + 1);
    let observation = match kind {
        Observed::Running => RuntimeObservationV1 {
            inspection: inspection(worker),
            observed_at: at(minutes_ago),
            status: RuntimeStatusV1::Running,
            running: Some(true),
            started_at: Some(started),
            finished_at: None,
            exit_code: None,
            oom_killed: Some(false),
            restart_count: Some(0),
        },
        Observed::Exited | Observed::OomKilled => RuntimeObservationV1 {
            inspection: inspection(worker),
            observed_at: at(minutes_ago),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some(started),
            finished_at: Some(finished),
            exit_code: Some(if matches!(kind, Observed::OomKilled) {
                137
            } else {
                0
            }),
            oom_killed: Some(matches!(kind, Observed::OomKilled)),
            restart_count: Some(0),
        },
        Observed::UnreadableStart => RuntimeObservationV1 {
            inspection: inspection(worker),
            observed_at: at(minutes_ago),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("unavailable".into()),
            finished_at: Some(finished),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        },
    };
    step(store, worker, WorkerTransitionV1::Observe { observation })
}

fn provision(
    store: &Store,
    name: &str,
    organization: &str,
    capabilities: &[HumanCapabilityV1],
) -> anyhow::Result<Account> {
    let token = generate_local_credential()?;
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: INSTALLATION.into(),
        account_id: id(),
    };
    let credential = LocalCredentialV1 {
        version: 1,
        credential_id: id(),
        principal: principal.clone(),
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        revision: 1,
        active: true,
        expires_at: None,
    };
    let membership = MembershipV1 {
        version: 1,
        membership_id: id(),
        principal: principal.clone(),
        tenant_id: store.tenant_id().into(),
        organization_scope: organization.into(),
        revision: 1,
        active: true,
        capabilities: capabilities.iter().copied().collect(),
    };
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                change(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: format!("Supervision test {name}"),
                        revision: 1,
                        active: true,
                    },
                }),
                change(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: credential.clone(),
                }),
                change(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: membership.clone(),
                }),
            ],
        })
        .with_context(|| format!("provision the test account {name}"))?;
    Ok(Account {
        token,
        principal,
        credential,
        membership,
    })
}

fn grant_resource(
    store: &Store,
    organization: &str,
    kind: ResourceKindV1,
    resource_id: &str,
    owner: &PrincipalRefV1,
    shares: Vec<ResourceShareV1>,
    active: bool,
) -> anyhow::Result<()> {
    let grant = |revision, active| ResourceGrantV1 {
        version: 1,
        tenant_id: store.tenant_id().into(),
        organization_scope: organization.into(),
        resource: ResourceRefV1 {
            kind,
            resource_id: resource_id.into(),
        },
        owner: owner.clone(),
        revision,
        active,
        shares: shares.clone(),
    };
    store
        .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: grant(1, true),
        }))
        .with_context(|| format!("grant the test resource {resource_id}"))?;
    if !active {
        store
            .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
                expected_revision: 1,
                grant: grant(2, false),
            }))
            .with_context(|| format!("retire the test resource {resource_id}"))?;
    }
    Ok(())
}

fn grant_broker(
    store: &Store,
    authority: &str,
    runtime: BrokerRuntimeKindV1,
) -> anyhow::Result<BrokerSeed> {
    let broker = BrokerSeed {
        broker_id: id(),
        incarnation_id: id(),
        credential_id: id(),
        authority_id: authority.into(),
        grant_revision: 1,
        token: None,
        heartbeat_revision: 0,
        sequence: 0,
    };
    store.mutate_broker_grant(&BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: broker.broker_id.clone(),
        expected_revision: None,
        credential_id: broker.credential_id.clone(),
        incarnation_id: broker.incarnation_id.clone(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: authority.into(),
            runtime_kind: runtime,
        }],
        lease_seconds: 300,
        not_after: FAR_FUTURE.into(),
    })?;
    Ok(broker)
}

/// Event attached to exactly one resource by the control's own projection.
pub fn resource_event(kind: ResourceKindV1, resource_id: &str, index: usize) -> Event {
    const KINDS: [&str; 6] = [
        "command.requested",
        "decision.recorded",
        "command.started",
        "command.completed",
        "file.changed",
        "session.note",
    ];
    let mut event = Event::new(KINDS[index % KINDS.len()]);
    match kind {
        ResourceKindV1::Session => event.session_id = Some(resource_id.into()),
        ResourceKindV1::Sandbox => event.sandbox_id = Some(resource_id.into()),
        ResourceKindV1::Agent => event.agent_id = Some(resource_id.into()),
        ResourceKindV1::Project => event.project_id = Some(resource_id.into()),
    }
    event.source = if index.is_multiple_of(5) {
        "adapter"
    } else {
        "gateway"
    }
    .into();
    event.provenance = ["observed", "declared", "inferred"][index % 3].into();
    event.coverage = ["native", "observed", "generic", "partial"][index % 4].into();
    if index % KINDS.len() == 1 {
        let outcome =
            [Outcome::Allow, Outcome::Deny, Outcome::Review][(index / KINDS.len()) % 3].clone();
        event.decision = Some(Decision::new(
            outcome,
            "supervision-test-rule",
            "synthetic decision of the supervision test data set",
        ));
    }
    if index.is_multiple_of(KINDS.len()) {
        event.command = Some(CommandSpec {
            argv: vec!["/usr/bin/printf".into(), format!("step-{index}")],
            cwd: "/workspace".into(),
        });
    }
    event.data = json!({ "label": format!("supervision-test-{index}") });
    event
}

impl Seed {
    pub fn build(directory: &Path, options: SeedOptions) -> anyhow::Result<Self> {
        anyhow::ensure!(
            (12..=500).contains(&options.workers) && options.events <= 20_000,
            "unsupported supervision seed size"
        );
        let path_a = directory.join("tenant-a.sqlite");
        let path_b = directory.join("tenant-b.sqlite");
        let path_c = directory.join("tenant-c.sqlite");
        let a = open(&path_a, "tenant-a")?;
        let authorities: Vec<String> = (0..4).map(|_| id()).collect();
        let sessions: Vec<String> = (0..options.workers).map(|_| id()).collect();
        let mut workers = Vec::with_capacity(options.workers);

        // Tenant A is built entirely from historical schema-2 control
        // mutations, the only lifecycle that needs no live supervision lease:
        // admission, creation, the single start claim, runtime observations,
        // stop and cancellation. The store is then migrated normally. Quota is
        // released as rows end, so at most 100 rows ever hold it.
        let mut held = 0usize;
        for (index, session) in sessions.iter().enumerate() {
            let authority = &authorities[index % authorities.len()];
            let worker = admit(&a, authority, session, true)?;
            let minutes = i64::try_from(index % 240)? * 3;
            let variant = if held >= 96 {
                2 + (index % 2) * 3
            } else {
                index % 8
            };
            let worker = match variant {
                0 | 1 => observe(&a, &start(&a, worker)?, Observed::Running, minutes)?,
                2 => observe(&a, &start(&a, worker)?, Observed::Exited, minutes)?,
                3 => observe(&a, &start(&a, worker)?, Observed::OomKilled, minutes)?,
                4 => observe(&a, &start(&a, worker)?, Observed::UnreadableStart, minutes)?,
                5 => step(&a, &worker, WorkerTransitionV1::CancelReserved)?,
                6 => worker,
                _ => match (index / 8) % 4 {
                    0 => start(&a, worker)?,
                    1 => step(&a, &worker, WorkerTransitionV1::Prepare)?,
                    2 => {
                        let running = observe(&a, &start(&a, worker)?, Observed::Running, minutes)?;
                        step(&a, &running, WorkerTransitionV1::Stop)?
                    }
                    _ => step(
                        &a,
                        &worker,
                        WorkerTransitionV1::Unknown {
                            reason: "supervision test: outcome not established".into(),
                        },
                    )?,
                },
            };
            if worker.quota_held {
                held += 1;
            }
            workers.push(worker.worker_id);
        }
        migrate(&a, &path_a)?;
        let a = match options.human_principal_events {
            Some(events) => {
                let mut policy = a.capacity_policy().clone();
                policy.human_principal.events = events;
                a.with_capacity_policy(policy)?
            }
            None => a,
        };

        let mut brokers = BTreeMap::new();
        let mut current = grant_broker(&a, &authorities[0], BrokerRuntimeKindV1::Docker)?;
        let token = format!("supervision-test-broker-{}", Uuid::new_v4().simple()).repeat(2);
        a.issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: id(),
            credential_id: current.credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: current.broker_id.clone(),
                incarnation_id: current.incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: FAR_FUTURE.into(),
        })?;
        current.token = Some(token);
        brokers.insert("current", current);
        let mut revoked = grant_broker(&a, &authorities[1], BrokerRuntimeKindV1::Docker)?;
        a.revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: id(),
            broker_id: revoked.broker_id.clone(),
            expected_revision: 1,
        })?;
        revoked.grant_revision = 2;
        brokers.insert("revoked", revoked);
        // Declared as Podman while its workers were launched for Docker: the
        // registry reports `runtime_matches=false` for this association.
        brokers.insert(
            "unseen",
            grant_broker(&a, &authorities[2], BrokerRuntimeKindV1::Podman)?,
        );

        let mut accounts = BTreeMap::new();
        accounts.insert(
            "admin_a",
            provision(
                &a,
                "administrator A",
                ORGANIZATION,
                &[
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadContent,
                    HumanCapabilityV1::Export,
                    HumanCapabilityV1::ReadTenantMetadata,
                    HumanCapabilityV1::ReadBrokerRegistry,
                    HumanCapabilityV1::ReadWorkerRegistry,
                ],
            )?,
        );
        accounts.insert(
            "user_a1",
            provision(
                &a,
                "user A1",
                ORGANIZATION,
                &[
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadContent,
                    HumanCapabilityV1::Export,
                ],
            )?,
        );
        accounts.insert(
            "user_a2",
            provision(
                &a,
                "user A2",
                ORGANIZATION,
                &[HumanCapabilityV1::ReadMetadata],
            )?,
        );
        accounts.insert(
            "workers_only_a",
            provision(
                &a,
                "worker registry reader A",
                ORGANIZATION,
                &[HumanCapabilityV1::ReadWorkerRegistry],
            )?,
        );
        accounts.insert(
            "brokers_only_a",
            provision(
                &a,
                "broker registry reader A",
                ORGANIZATION,
                &[HumanCapabilityV1::ReadBrokerRegistry],
            )?,
        );
        accounts.insert(
            "other_org_a",
            provision(
                &a,
                "user of another organization",
                OTHER_ORGANIZATION,
                &[HumanCapabilityV1::ReadMetadata],
            )?,
        );
        let other_org = accounts["other_org_a"].principal.clone();
        let user_a1 = accounts["user_a1"].principal.clone();
        let user_a2 = accounts["user_a2"].principal.clone();
        let share_metadata = vec![ResourceShareV1 {
            principal: user_a2.clone(),
            permissions: BTreeSet::from([ResourcePermissionV1::Metadata]),
        }];

        // Grants. The first sessions are shared by a worker row, a grant and
        // events, which is the only correlation the named contract exposes.
        let granted = sessions.len().min(10);
        let mut targets: Vec<(ResourceKindV1, String)> = Vec::new();
        for (index, session) in sessions.iter().take(granted).enumerate() {
            let (owner, shares) = match index {
                1 => (&user_a1, share_metadata.clone()),
                2 => (&user_a2, vec![]),
                _ => (&user_a1, vec![]),
            };
            grant_resource(
                &a,
                ORGANIZATION,
                ResourceKindV1::Session,
                session,
                owner,
                shares,
                true,
            )?;
            targets.push((ResourceKindV1::Session, session.clone()));
        }
        for (kind, name) in [
            (ResourceKindV1::Agent, "agent-planner"),
            (ResourceKindV1::Agent, "agent-reviewer"),
            (ResourceKindV1::Sandbox, "sandbox-build"),
            (ResourceKindV1::Sandbox, "sandbox-tests"),
            (ResourceKindV1::Project, "project-atlas"),
        ] {
            // The control grants only a resource already known to its audit.
            a.append(resource_event(kind, name, 0))?;
            grant_resource(&a, ORGANIZATION, kind, name, &user_a1, vec![], true)?;
            targets.push((kind, name.into()));
        }
        // Negative controls for authorization: an inactive grant, a grant of
        // another organization and a session that nobody was granted.
        a.append(resource_event(
            ResourceKindV1::Sandbox,
            "sandbox-retired",
            0,
        ))?;
        a.append(resource_event(
            ResourceKindV1::Project,
            "project-elsewhere",
            0,
        ))?;
        grant_resource(
            &a,
            ORGANIZATION,
            ResourceKindV1::Sandbox,
            "sandbox-retired",
            &user_a1,
            vec![],
            false,
        )?;
        grant_resource(
            &a,
            OTHER_ORGANIZATION,
            ResourceKindV1::Project,
            "project-elsewhere",
            &other_org,
            vec![],
            true,
        )?;
        let session_without_grant = sessions[granted.min(sessions.len() - 1)].clone();
        let hidden = [
            (ResourceKindV1::Sandbox, "sandbox-retired".to_string()),
            (ResourceKindV1::Project, "project-elsewhere".to_string()),
            (ResourceKindV1::Session, session_without_grant.clone()),
        ];
        for index in 0..options.events {
            // Skew the volume so the first session owns a dense history.
            let (kind, resource) = if index % 2 == 0 {
                &targets[0]
            } else {
                &targets[index % targets.len()]
            };
            a.append(resource_event(*kind, resource, index))?;
            if index % 97 == 0 {
                let (kind, resource) = &hidden[(index / 97) % hidden.len()];
                a.append(resource_event(*kind, resource, index))?;
            }
        }

        let b = open(&path_b, "tenant-b")?;
        migrate(&b, &path_b)?;
        let authority_b = id();
        let session_b = id();
        let broker_b = grant_broker(&b, &authority_b, BrokerRuntimeKindV1::Docker)?;
        let worker_b = admit(&b, &authority_b, &session_b, false)?;
        accounts.insert(
            "admin_b",
            provision(
                &b,
                "administrator B",
                ORGANIZATION,
                &[
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadTenantMetadata,
                    HumanCapabilityV1::ReadBrokerRegistry,
                    HumanCapabilityV1::ReadWorkerRegistry,
                ],
            )?,
        );
        grant_resource(
            &b,
            ORGANIZATION,
            ResourceKindV1::Session,
            &session_b,
            &accounts["admin_b"].principal.clone(),
            vec![],
            true,
        )?;
        for index in 0..6 {
            b.append(resource_event(ResourceKindV1::Session, &session_b, index))?;
        }

        // Tenant C has an administrator and no registry row at all.
        let c = open(&path_c, "tenant-c")?;
        migrate(&c, &path_c)?;
        accounts.insert(
            "admin_c",
            provision(
                &c,
                "administrator C",
                ORGANIZATION,
                &[
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadTenantMetadata,
                    HumanCapabilityV1::ReadBrokerRegistry,
                    HumanCapabilityV1::ReadWorkerRegistry,
                ],
            )?,
        );

        let tokens = |role: &str| {
            ["a", "b", "c"].map(|tenant| {
                format!(
                    "supervision-test-{role}-{tenant}-{}",
                    Uuid::new_v4().simple()
                )
            })
        };
        Ok(Self {
            a: Arc::new(a),
            b: Arc::new(b),
            c: Arc::new(c),
            accounts,
            brokers,
            unregistered_authority: authorities[3].clone(),
            worker_ids_a: workers,
            worker_ids_b: vec![worker_b.worker_id],
            broker_ids_b: vec![broker_b.broker_id],
            session_owned_by_user_a1: sessions[0].clone(),
            session_shared_with_user_a2: sessions[1].clone(),
            session_owned_by_user_a2: sessions[2].clone(),
            session_without_grant,
            session_b,
            granted_sessions_a: sessions[..granted].to_vec(),
            runner_tokens: tokens("runner"),
            viewer_tokens: tokens("viewer"),
        })
    }

    pub fn gateway_state(&self) -> anyhow::Result<GatewayState> {
        let tenants = [
            ("tenant-a", &self.a),
            ("tenant-b", &self.b),
            ("tenant-c", &self.c),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (tenant, store))| TenantGateway {
            tenant_id: tenant.into(),
            store: Arc::clone(store),
            runner_token: self.runner_tokens[index].clone(),
            viewer_token: self.viewer_tokens[index].clone(),
        })
        .collect();
        GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![]))
    }

    /// Next heartbeat of the broker that owns a workload binding. It must be
    /// posted to the real gateway route; the Store accepts no unauthenticated
    /// heartbeat. The caller reports success with `heartbeat_accepted`.
    pub fn next_heartbeat(&self) -> anyhow::Result<(String, BrokerHeartbeatV1)> {
        let broker = &self.brokers["current"];
        Ok((
            broker
                .token
                .clone()
                .context("the current broker owns a binding")?,
            BrokerHeartbeatV1 {
                version: 1,
                operation_id: id(),
                expected_heartbeat_revision: broker.heartbeat_revision,
                incarnation_id: broker.incarnation_id.clone(),
                sequence: broker.sequence + 1,
            },
        ))
    }
    pub fn heartbeat_accepted(&mut self, heartbeat_revision: u64) -> anyhow::Result<()> {
        let broker = self
            .brokers
            .get_mut("current")
            .context("the current broker exists")?;
        broker.sequence += 1;
        broker.heartbeat_revision = heartbeat_revision;
        Ok(())
    }

    /// New membership revision with exactly these capabilities.
    pub fn set_capabilities(
        &mut self,
        account: &str,
        capabilities: &[HumanCapabilityV1],
    ) -> anyhow::Result<u64> {
        let store = self.store_of(account)?;
        let account = self
            .accounts
            .get_mut(account)
            .context("unknown test account")?;
        let mut membership = account.membership.clone();
        membership.revision += 1;
        membership.capabilities = capabilities.iter().copied().collect();
        store.mutate_identity(&change(IdentityChangeV1::Membership {
            expected_revision: account.membership.revision,
            membership: membership.clone(),
        }))?;
        account.membership = membership;
        Ok(account.membership.revision)
    }

    pub fn revoke_credential(&mut self, account: &str) -> anyhow::Result<()> {
        let store = self.store_of(account)?;
        let account = self
            .accounts
            .get_mut(account)
            .context("unknown test account")?;
        let mut credential = account.credential.clone();
        credential.revision += 1;
        credential.active = false;
        store.mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: account.credential.revision,
            binding: credential.clone(),
        }))?;
        account.credential = credential;
        Ok(())
    }

    /// New credential revision that expires at this RFC 3339 instant. The
    /// contexts the control offers afterwards cannot outlive it.
    pub fn set_credential_expiry(&mut self, account: &str, expires_at: &str) -> anyhow::Result<()> {
        let store = self.store_of(account)?;
        let account = self
            .accounts
            .get_mut(account)
            .context("unknown test account")?;
        let mut credential = account.credential.clone();
        credential.revision += 1;
        credential.expires_at = Some(expires_at.into());
        store.mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: account.credential.revision,
            binding: credential.clone(),
        }))?;
        account.credential = credential;
        Ok(())
    }

    pub fn revoke_broker(&mut self, name: &str) -> anyhow::Result<()> {
        let broker = self.brokers.get_mut(name).context("unknown test broker")?;
        self.a.revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: id(),
            broker_id: broker.broker_id.clone(),
            expected_revision: broker.grant_revision,
        })?;
        broker.grant_revision += 1;
        Ok(())
    }

    fn store_of(&self, account: &str) -> anyhow::Result<Arc<Store>> {
        Ok(Arc::clone(match account {
            "admin_b" => &self.b,
            "admin_c" => &self.c,
            name if self.accounts.contains_key(name) => &self.a,
            _ => anyhow::bail!("unknown test account"),
        }))
    }
}

/// Real routers on two loopback listeners: the gateway, and the dashboard
/// proxy in front of it. Dropping the value stops both servers.
pub struct Served {
    pub gateway: String,
    pub dashboard: String,
    pub client: reqwest::Client,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Served {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub async fn serve(seed: &Seed, front: std::path::PathBuf) -> anyhow::Result<Served> {
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let gateway = format!("http://{}", gateway_listener.local_addr()?);
    let router = agentsandbox::api::gateway_router(seed.gateway_state()?);
    let gateway_task = tokio::spawn(async move {
        let _ = axum::serve(gateway_listener, router).await;
    });
    let dashboard_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let dashboard = format!("http://{}", dashboard_listener.local_addr()?);
    let proxy = agentsandbox::api::dashboard_shared_router(&format!("{gateway}/"), front)?;
    let dashboard_task = tokio::spawn(async move {
        let _ = axum::serve(dashboard_listener, proxy).await;
    });
    Ok(Served {
        gateway,
        dashboard,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(20))
            .build()?,
        tasks: vec![gateway_task, dashboard_task],
    })
}

/// Posts the next declared heartbeat through the authenticated broker route.
pub async fn post_heartbeat(seed: &mut Seed, served: &Served) -> anyhow::Result<()> {
    let (token, heartbeat) = seed.next_heartbeat()?;
    let response = served
        .client
        .post(format!("{}/api/v1/brokers/heartbeat", served.gateway))
        .bearer_auth(token)
        .json(&heartbeat)
        .send()
        .await?;
    anyhow::ensure!(
        response.status() == reqwest::StatusCode::OK,
        "the broker heartbeat was refused with HTTP {}",
        response.status().as_u16()
    );
    let body: serde_json::Value = response.json().await?;
    let revision = body["broker"]["heartbeat_revision"]
        .as_u64()
        .context("the heartbeat result carries its revision")?;
    seed.heartbeat_accepted(revision)
}

pub struct Reply {
    pub status: reqwest::StatusCode,
    pub body: serde_json::Value,
    pub raw: Vec<u8>,
}

/// One authenticated GET against a served base URL (gateway or dashboard).
pub async fn http_get(
    served: &Served,
    base: &str,
    path: &str,
    token: &str,
    context: Option<&str>,
) -> anyhow::Result<Reply> {
    let mut request = served
        .client
        .get(format!("{base}{path}"))
        .bearer_auth(token);
    if let Some(context) = context {
        request = request.header("X-AiGuard-Context", context);
    }
    let response = request.send().await?;
    let status = response.status();
    let raw = response.bytes().await?.to_vec();
    Ok(Reply {
        status,
        body: serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null),
        raw,
    })
}
