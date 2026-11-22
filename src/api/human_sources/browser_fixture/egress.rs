//! A separate opt-in E1 Journal recipe over the real owned Store15.
use super::{
    CLOSE_RESERVE, COMMAND_BYTES, COMMAND_LIMIT, Config, Servers, export_receipts, publish,
};
use crate::{
    api::{GatewayState, dashboard_router_with_human_mode, gateway_router},
    audit::Store,
    control::egress_policy::{
        EgressPolicyExpectedScopeV1, EgressPolicyInputV1, EgressPolicyReceiptV1,
        EgressPolicySelectorV1, EgressPolicyStateV1, EgressPolicyTenantCeilingV1,
        EgressReplacePolicyRequestV1, EgressRevokePolicyRequestV1,
    },
    egress::{Authority, EgressTransport, PolicyLimits},
    human_authority::startup::HumanOwnedStartupBundle,
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, LocalHumanProvisionV1, MembershipV1,
        PrincipalRefV1, ResourceGrantV1, ResourceKindV1, ResourceRefV1, generate_local_credential,
    },
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runner::AuditClient,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

const TENANT: &str = "A";
const ORGANIZATION: &str = "synthetic-egress-journal";
const OWNER_NAME: &str = "Synthetic E1 Journal owner";
const READER_NAME: &str = "Synthetic E1 Journal reader";
const REPLACED: &str = "control.egress_policy.replaced";
const REVOKE_REQUESTED: &str = "control.egress_policy.revoke_requested";

fn require_native_fixture_account() -> Result<()> {
    // SAFETY: these calls only read the effective identities of this process.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    ensure!(
        cfg!(target_arch = "aarch64")
            && ((cfg!(target_os = "linux") && uid == 1000 && gid == 1000)
                || (cfg!(target_os = "macos") && uid != 0)),
        "egress browser fixture requires native Linux ARM64 UID/GID 1000 or macOS ARM64 non-root"
    );
    Ok(())
}

fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-egress-browser-offline-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn provision_humans(
    bundle: &HumanOwnedStartupBundle,
    resource: &ResourceRefV1,
) -> Result<(String, String)> {
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(TENANT) && source.required_format == 15)
        .context("synthetic Store15 source absent")?;
    let source = bundle.owner.source_token(descriptor.source_id)?;
    let store = bundle
        .stores
        .get(&descriptor.source_id)
        .context("synthetic Store15 absent")?;
    let provision = |display_name: &str| -> Result<(PrincipalRefV1, String)> {
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: descriptor.installation_id.to_string(),
            account_id: Uuid::new_v4().to_string(),
        };
        let bearer = generate_local_credential()?;
        let epoch = bundle.owner.try_write_epoch()?;
        store.provision_local_human_scoped(
            &epoch,
            &source,
            &LocalHumanProvisionV1 {
                version: 1,
                mutations: [
                    mutation(IdentityChangeV1::Account {
                        expected_revision: 0,
                        account: HumanAccountV1 {
                            version: 1,
                            principal: principal.clone(),
                            display_name: display_name.into(),
                            revision: 1,
                            active: true,
                        },
                    }),
                    mutation(IdentityChangeV1::Credential {
                        expected_revision: 0,
                        binding: LocalCredentialV1 {
                            version: 1,
                            credential_id: Uuid::new_v4().to_string(),
                            principal: principal.clone(),
                            sha256_digest: hex::encode(Sha256::digest(bearer.as_bytes())),
                            revision: 1,
                            active: true,
                            expires_at: None,
                        },
                    }),
                    mutation(IdentityChangeV1::Membership {
                        expected_revision: 0,
                        membership: MembershipV1 {
                            version: 1,
                            membership_id: Uuid::new_v4().to_string(),
                            principal: principal.clone(),
                            tenant_id: TENANT.into(),
                            organization_scope: ORGANIZATION.into(),
                            revision: 1,
                            active: true,
                            capabilities: [HumanCapabilityV1::ReadMetadata].into_iter().collect(),
                        },
                    }),
                ],
            },
        )?;
        epoch.check()?;
        Ok((principal, bearer))
    };
    let (owner, owner_bearer) = provision(OWNER_NAME)?;
    let (_, reader_bearer) = provision(READER_NAME)?;
    let epoch = bundle.owner.try_write_epoch()?;
    store.mutate_identity_scoped(
        &epoch,
        &source,
        &mutation(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: TENANT.into(),
                organization_scope: ORGANIZATION.into(),
                resource: resource.clone(),
                owner,
                revision: 1,
                active: true,
                shares: vec![],
            },
        }),
    )?;
    epoch.check()?;
    Ok((owner_bearer, reader_bearer))
}

async fn install_policies(
    client: &AuditClient,
    scope: &EgressPolicyExpectedScopeV1,
    resource: &ResourceRefV1,
    receipts: &mut Vec<EgressPolicyReceiptV1>,
) -> Result<EgressRevokePolicyRequestV1> {
    let now = u64::try_from(chrono::Utc::now().timestamp_millis())?;
    let policy = EgressPolicyInputV1::new(
        now.saturating_sub(60_000),
        now.checked_add(600_000)
            .context("synthetic policy validity overflow")?,
        &[Authority::new(
            "owned-egress-fixture.invalid",
            443,
            EgressTransport::ConnectTcpOpaque,
        )?],
        PolicyLimits::new(1, 1)?,
    )?;
    let tenant = EgressReplacePolicyRequestV1::new(
        Uuid::new_v4(),
        scope.clone(),
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        policy.clone(),
        None,
    )?;
    let receipt = client.egress_replace_policy(&tenant).await?;
    let ceiling = EgressPolicyTenantCeilingV1::new(
        receipt.revision(),
        receipt.after_epoch(),
        tenant.policy().to_policy(receipt.revision())?.digest(),
    )?;
    receipts.push(receipt);
    let session = EgressReplacePolicyRequestV1::new(
        Uuid::new_v4(),
        scope.clone(),
        EgressPolicySelectorV1::Session(resource.resource_id.clone()),
        0,
        0,
        policy,
        Some(ceiling),
    )?;
    let receipt = client.egress_replace_policy(&session).await?;
    let revoke = EgressRevokePolicyRequestV1::new(
        Uuid::new_v4(),
        scope.clone(),
        receipt.selector().clone(),
        receipt.revision(),
        receipt.after_epoch(),
    )?;
    receipts.push(receipt);
    Ok(revoke)
}

fn receipt_summary(receipt: &EgressPolicyReceiptV1) -> Value {
    json!({
        "operation_id":receipt.operation_id(), "event_id":receipt.event_id(),
        "event_seq":receipt.event_seq(), "event_hash":receipt.event_hash(), "received_at":receipt.received_at(),
        "revision":receipt.revision(), "before_epoch":receipt.before_epoch(),
        "after_epoch":receipt.after_epoch(), "state":receipt.state()
    })
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    RevokeSessionPolicy,
    Stop,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    version: u32,
    sequence: u16,
    action: Action,
}

async fn control(
    config: &Config,
    deadline: Instant,
    servers: &Servers,
    client: &AuditClient,
    revoke: EgressRevokePolicyRequestV1,
    receipts: &mut Vec<EgressPolicyReceiptV1>,
) -> Result<u16> {
    let mut accepted: Option<crate::control_fs::PrivateFile> = None;
    for sequence in 1..=COMMAND_LIMIT {
        let name = format!("{sequence:04}.json");
        let file = loop {
            ensure!(
                Instant::now() < deadline,
                "egress fixture control deadline exceeded"
            );
            ensure!(
                !servers
                    .tasks
                    .iter()
                    .any(tokio::task::JoinHandle::is_finished),
                "egress fixture HTTP server stopped unexpectedly"
            );
            if let Some(previous) = &accepted {
                previous.revalidate()?;
            }
            if let Some(file) = config
                .commands
                .persistent_read(&name, COMMAND_BYTES as u64, &[])?
            {
                break file;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let command: Command = serde_json::from_slice(file.bytes())
            .map_err(|_| anyhow::anyhow!("invalid closed egress fixture command"))?;
        ensure!(
            command.version == 1 && command.sequence == sequence,
            "invalid egress fixture command version or sequence"
        );
        file.revalidate()?;
        let stop = matches!(command.action, Action::Stop);
        let mut ack = json!({"version":1,"sequence":sequence,"action":command.action,"command_sha256":file.digest()});
        if stop {
            ack["status"] = json!("stopping");
        } else {
            ensure!(
                accepted.is_none() && receipts.len() == 2,
                "egress fixture revocation was already requested"
            );
            let remaining = deadline.saturating_duration_since(Instant::now());
            let receipt = tokio::time::timeout(remaining, client.egress_revoke_policy(&revoke))
                .await
                .context("egress fixture revocation deadline exceeded")??;
            ensure!(
                receipt.state() == EgressPolicyStateV1::RevokeRequested,
                "egress fixture did not record a revocation request"
            );
            ack["status"] = json!("revocation_requested");
            ack["receipt"] = receipt_summary(&receipt);
            receipts.push(receipt);
        }
        file.revalidate()?;
        publish(&config.acknowledgements, &name, &ack)?;
        if stop {
            return Ok(sequence);
        }
        accepted = Some(file);
    }
    anyhow::bail!("egress fixture command limit reached without stop")
}

fn verify_policy_receipts(path: &Path, receipts: &[EgressPolicyReceiptV1]) -> Result<()> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::ZERO)?;
    let format: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let count: i64 =
        connection.query_row("SELECT count(*) FROM egress_policy_operations", [], |row| {
            row.get(0)
        })?;
    ensure!(
        format == 15
            && (2..=3).contains(&receipts.len())
            && count == i64::try_from(receipts.len())?,
        "egress fixture retained policy operation count differs"
    );
    for receipt in receipts {
        let session = match receipt.selector() {
            EgressPolicySelectorV1::Tenant => None,
            EgressPolicySelectorV1::Session(id) => Some(id.as_str()),
        };
        let kind = match receipt.state() {
            EgressPolicyStateV1::Active => REPLACED,
            EgressPolicyStateV1::RevokeRequested => REVOKE_REQUESTED,
        };
        let canonical = receipt.canonical_bytes()?;
        let exact: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM egress_policy_operations p JOIN events e ON e.seq=p.event_seq WHERE p.operation_id=?1 AND p.receipt_json=?2 AND e.id=?3 AND e.seq=?4 AND e.hash=?5 AND e.received_at=?6 AND e.kind=?7 AND json_extract(e.payload,'$.session_id') IS ?8 AND json_extract(e.payload,'$.data.actor.kind')='machine_control' AND json_extract(e.payload,'$.decision') IS NULL)",
            rusqlite::params![receipt.operation_id().to_string(), std::str::from_utf8(&canonical)?, receipt.event_id(), receipt.event_seq(), receipt.event_hash(), receipt.received_at(), kind, session],
            |row| row.get(0),
        )?;
        ensure!(exact, "committed egress policy receipt differs from SQLite");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit Store15 E1 browser QA only; requires private control root and admitted frontend dist"]
async fn serve_owned_egress_browser_fixture() -> Result<()> {
    let started = Instant::now();
    require_native_fixture_account()?;
    let config = Config::load("run-egress-v1")?;
    let deadline = started + Duration::from_secs(config.seconds) - CLOSE_RESERVE;
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: Uuid::new_v4().to_string(),
    };
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(15, |store| {
        // The normal resource grant needs a preceding event for its exact
        // Session. This declares the synthetic resource, never a connection.
        let mut event = Event::new("fixture.egress-journal-session");
        event.session_id = Some(resource.resource_id.clone());
        event.source = "fixture".into();
        event.provenance = "declared".into();
        event.coverage = "generic".into();
        store
            .append(event)
            .expect("append synthetic Session declaration");
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let (owner_bearer, reader_bearer) = provision_humans(&bundle, &resource)?;
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(TENANT))
        .context("egress fixture source absent")?;
    let database = descriptor.storage_binding.path();
    let scope =
        EgressPolicyExpectedScopeV1::new(TENANT, &descriptor.durable_namespace_id.to_string())?;
    let runner = tokens
        .iter()
        .find(|(tenant, _, _)| tenant == TENANT)
        .map(|(_, runner, _)| runner.clone())
        .context("egress fixture runner absent")?;
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )?;
    let mut servers = Servers::new(&state)?;
    let mut policy_receipts = Vec::new();
    let work = async {
        ensure!(Instant::now() < deadline, "egress fixture setup exhausted its budget");
        let gateway = servers.bind(gateway_router(state.clone())).await?;
        let origin = format!("http://{gateway}");
        let client = AuditClient::new(&origin, &runner)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let revoke = tokio::time::timeout(remaining, install_policies(&client, &scope, &resource, &mut policy_receipts)).await
            .context("egress fixture policy installation deadline exceeded")??;
        let dashboard = servers.bind(dashboard_router_with_human_mode(&origin, None, config.dist.clone(), None, true)?).await?;
        publish(&config.root, "READY.json", &json!({
            "version":1,"mode":"egress_journal","dashboard_url":format!("http://{dashboard}"),"tenant_id":TENANT,
            "resource":resource,"owner":{"display_name":OWNER_NAME,"bearer":owner_bearer},
            "reader":{"display_name":READER_NAME,"bearer":reader_bearer},
            "tenant_policy":receipt_summary(&policy_receipts[0]),"session_policy":receipt_summary(&policy_receipts[1]),
            "budget_seconds":config.seconds,"commands":"commands","acks":"acks","command_limit":COMMAND_LIMIT
        }))?;
        control(&config, deadline, &servers, &client, revoke, &mut policy_receipts).await
    }.await;
    let stopped = servers.close().await;
    drop(servers);
    drop(state);
    let read_receipts = export_receipts(&config.root, &database, TENANT);
    let policy_proof = verify_policy_receipts(&database, &policy_receipts);
    publish(
        &config.root,
        "CLOSED.json",
        &json!({
            "version":1,"completed":work.is_ok() && stopped.is_ok() && read_receipts.is_ok() && policy_proof.is_ok(),
            "commands_processed":work.as_ref().ok().copied(),"servers_and_owner_drained":stopped.is_ok(),
            "receipts_exported":read_receipts.is_ok(),"policy_receipts_verified":policy_proof.is_ok(),
            "stop_reason":if work.is_ok() {"requested"} else {"error_or_deadline"}
        }),
    )?;
    work?;
    stopped?;
    read_receipts?;
    policy_proof
}

#[tokio::test]
async fn owned_event_detail_format15_returns_metadata_and_commits_receipt() {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    async fn get(router: &Router, bearer: &str, path: &str, context: Option<&str>) -> Value {
        let mut request = Request::get(path).header("authorization", format!("Bearer {bearer}"));
        if let Some(context) = context {
            request = request.header("x-aiguard-context", context);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 262_144)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: Uuid::new_v4().to_string(),
    };
    let mut event = Event::new("fixture.event-detail");
    event.session_id = Some(resource.resource_id.clone());
    event.data = json!({"fixture_note": "synthetic-format15-event-body"});
    let event_id = event.id.clone();
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(15, |store| {
        store.append(event).unwrap();
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let (bearer, _) = provision_humans(&bundle, &resource).unwrap();
    let database = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(TENANT) && source.required_format == 15)
        .unwrap()
        .storage_binding
        .path();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let router = gateway_router(state);
    let identity = get(&router, &bearer, "/api/v1/human/identity", None).await;
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == TENANT && context["mode"] == "personal")
        .unwrap();
    let detail = get(
        &router,
        &bearer,
        &format!("/api/v1/human/events/{event_id}?view=metadata"),
        context["id"].as_str(),
    )
    .await;
    assert_eq!(detail["context_id"], context["id"]);
    assert_eq!(
        detail["authorization_version"],
        context["authorization_version"]
    );
    assert_eq!(detail["view"], "metadata");
    assert_eq!(detail["has_more"], false);
    assert!(detail["next_cursor"].is_null());
    let items: Vec<crate::identity::HumanEventItemV1> =
        serde_json::from_value(detail["items"].clone()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].metadata.id, event_id);
    assert_eq!(items[0].metadata.resource, resource);
    assert!(items[0].content.is_none());
    assert!(!detail.to_string().contains("synthetic-format15-event-body"));

    let reader = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt = &detail["read_receipt"];
    let (payload, received_at): (String, String) = reader
        .query_row(
            "SELECT payload,received_at FROM events WHERE kind='control.identity.access' AND id=?1",
            [receipt["audit_event_id"].as_str().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let audit: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(audit["data"]["action"], "event");
    assert_eq!(audit["data"]["result_count"], 1);
    assert_eq!(audit["data"]["principal"], identity["principal"]);
    assert_eq!(audit["data"]["mode"], context["mode"]);
    assert_eq!(audit["data"]["request_id"], receipt["request_id"]);
    assert_eq!(received_at, receipt["received_at"].as_str().unwrap());
}
