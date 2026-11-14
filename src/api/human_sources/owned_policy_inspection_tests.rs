//! Test-first operator inspection over actual owned15 sources. No future DTO.
//! Execution, including formatting, belongs to the separately admitted QA.
use crate::{
    api::{GatewayState, gateway_router},
    audit::{HumanSourceAccess, Store},
    control::egress_policy::{
        EgressPolicyExpectedScopeV1, EgressPolicyInputV1, EgressPolicySelectorV1,
        EgressPolicyTenantCeilingV1, EgressReplacePolicyRequestV1,
        EgressRevokePolicyRequestV1,
    },
    control_fs::PrivateFile,
    egress::{Authority, EgressTransport, PolicyLimits},
    human_authority::startup::HumanOwnedStartupBundle,
    identity::*,
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runner::AuditClient,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}, mpsc},
    thread,
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};
use tower::ServiceExt;

pub(super) const RUNNER: &str = "synthetic-A-runner";
pub(super) const INSPECT: &str = "/control/api/v1/egress/policies/inspect";
pub(super) const PROTOCOL: &str = "/control/api/v1/control/protocol";
const OBSERVE: Duration = Duration::from_secs(120);
const ACTIVE: &str = "11000000-0000-4000-8000-000000000101";
const INACTIVE: &str = "11000000-0000-4000-8000-000000000102";
const ABSENT: &str = "11000000-0000-4000-8000-000000000103";

pub(super) struct Fixture {
    // The existing fixture owns every synthetic file until real work drains.
    _files: Box<dyn Send>,
    pub(super) state: GatewayState,
    pub(super) credentials: PrivateFile,
    pub(super) namespace: String,
    pub(super) database: PathBuf,
}

pub(super) fn tenant(state: &GatewayState, id: &str) -> usize {
    state.tenants.iter().position(|entry| entry.tenant_id == id).unwrap()
}

fn identity_change(bundle: &HumanOwnedStartupBundle, change: IdentityChangeV1) {
    let descriptor = bundle.owner.closure().sources.iter()
        .find(|source| source.tenant_id.as_deref() == Some("A")).unwrap();
    let source = bundle.owner.source_token(descriptor.source_id).unwrap();
    let epoch = bundle.owner.try_write_epoch().unwrap();
    bundle.stores.get(&descriptor.source_id).unwrap().mutate_identity_scoped(
        &epoch, &source, &IdentityMutationV1 {
            version: 1, operation_id: uuid::Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "synthetic-inspection-offline-setup".into(),
                role: LegacyControlRoleV1::Runner,
            }, change,
        },
    ).unwrap();
    epoch.check().unwrap();
}

impl Fixture {
    pub(super) fn new(format: u32) -> Self {
        let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
            // Genuine causal events precede grants and owned migration. The
            // account/membership are installed by the existing owned fixture.
            for session in [ACTIVE, INACTIVE] {
                let mut event = Event::new("fixture.inspection-session-created");
                event.session_id = Some(session.into());
                legacy.append(event).unwrap();
            }
        });
        let (bundle, credentials, tokens) = files.take_startup();
        let binding = crate::control::review_binding::tests::provision();
        for session in [ACTIVE, INACTIVE] {
            let mut grant = ResourceGrantV1 {
                version: 1, tenant_id: "A".into(),
                organization_scope: binding.policy.organization_scope.clone(),
                resource: ResourceRefV1 { kind: ResourceKindV1::Session, resource_id: session.into() },
                owner: binding.policy.owner.clone(), revision: 1, active: true, shares: vec![],
            };
            identity_change(&bundle, IdentityChangeV1::ResourceGrant { expected_revision: 0, grant: grant.clone() });
            if session == INACTIVE {
                grant.revision = 2;
                grant.active = false;
                identity_change(&bundle, IdentityChangeV1::ResourceGrant { expected_revision: 1, grant });
            }
        }
        // The complete collision index is created only after all identities,
        // grants and memberships. No stale generation is a positive fixture.
        Self::from_parts(Box::new(files), bundle, credentials, tokens, format)
    }

    fn from_parts(
        files: Box<dyn Send>, bundle: HumanOwnedStartupBundle, credentials: PrivateFile,
        tokens: Vec<(String, String, String)>, format: u32,
    ) -> Self {
        let state = GatewayState::for_owned_human_sources(
            bundle, credentials.clone(), tokens, Policy::default(), None, Redactor::new(vec![]),
        ).unwrap();
        let owned = state.owned_human.as_ref().unwrap();
        let a = tenant(&state, "A");
        let credential = crate::api::authenticate_review_runner_fixture(
            credentials.clone(), owned.owner.clone(),
            state.tenants.iter().map(|entry| (entry.tenant_id.clone(), Arc::clone(&entry.store))).collect(),
            RUNNER,
        ).unwrap();
        let (namespace, _) = {
            let epoch = owned.owner.try_read_epoch().unwrap();
            state.tenants[a].store.assert_owned_control_fixture_binding_for_test(
                &epoch, &owned.sources[a], &credential, format,
            )
        };
        let database = owned.sources[a].descriptor().storage_binding.path();
        drop(credential);
        Self { _files: files, state, credentials, namespace, database }
    }

    pub(super) fn scope(&self) -> EgressPolicyExpectedScopeV1 {
        EgressPolicyExpectedScopeV1::new("A", &self.namespace).unwrap()
    }

    pub(super) async fn drain(&self) -> Result<()> {
        let retirement = self.state.human_retirement().context("owned retirement absent")?;
        retirement.retire();
        tokio::time::timeout(OBSERVE, retirement.drain()).await.context("real owned drain watchdog")??;
        Ok(())
    }
}

/// One loopback listener can switch between two actual, separately owned
/// Routers. This is transport composition, never a replacement backend.
pub(super) struct Server {
    pub(super) origin: String,
    router: Arc<Mutex<Router>>,
    pub(super) paths: Arc<Mutex<Vec<(String, String, u16)>>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl Server {
    pub(super) async fn new(state: &GatewayState) -> Result<Self> {
        let router = Arc::new(Mutex::new(gateway_router(state.clone())));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().fallback(|request: Request| async move {
            // This fallback is unreachable: middleware always invokes the
            // currently retained real Router rather than its Next handler.
            drop(request);
            StatusCode::INTERNAL_SERVER_ERROR
        }).layer(middleware::from_fn({
            let router = Arc::clone(&router);
            let paths = Arc::clone(&paths);
            move |request: Request, _next: Next| {
                let actual = router.lock().unwrap().clone();
                let paths = Arc::clone(&paths);
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_owned();
                    let response = actual.oneshot(request).await.unwrap();
                    paths.lock().unwrap().push((method, path, response.status().as_u16()));
                    response
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).with_graceful_shutdown(async { let _ = stopped.await; }).await
        });
        Ok(Self { origin, router, paths, stop: Some(stop), task: Some(task) })
    }
    pub(super) fn replace(&self, state: &GatewayState) {
        *self.router.lock().unwrap() = gateway_router(state.clone());
    }
    pub(super) fn count(&self, method: &str, path: &str) -> usize {
        self.paths.lock().unwrap().iter().filter(|(m, p, _)| m == method && p == path).count()
    }
    pub(super) async fn sdk_witness(&self, fixture: &Fixture, token: &str, id: &str, format: u32) -> Result<()> {
        let client = AuditClient::new(&self.origin, token)?;
        let protocol = tokio::time::timeout(OBSERVE, client.control_supervision_identity())
            .await.context("SDK setup watchdog is not inspection absence")??;
        let expected = fixture.state.owned_human.as_ref().unwrap().sources[tenant(&fixture.state, id)]
            .descriptor().durable_namespace_id.to_string();
        ensure!(protocol.schema_version == format && protocol.tenant_id == id && protocol.role == "runner"
            && protocol.control_namespace_uuid.as_deref() == Some(expected.as_str()), "invalid real SDK setup");
        ensure!(self.paths.lock().unwrap().iter().any(|(m,p,s)| m == "GET" && p == PROTOCOL && *s == 200),
            "SDK witness lacks actual HTTP200");
        Ok(())
    }
    pub(super) async fn close(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(30), &mut task).await {
                Ok(joined) => joined.context("inspection server task")??,
                Err(_) => {
                    task.abort();
                    let aborted = tokio::time::timeout(Duration::from_secs(30), &mut task).await;
                    if aborted.is_err() { self.task = Some(task); }
                    anyhow::bail!("inspection server cleanup watchdog; abort observed={}", aborted.is_ok());
                }
            }
        }
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
        if let Some(task) = self.task.take() { task.abort(); }
    }
}

pub(super) fn policy() -> EgressPolicyInputV1 {
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    EgressPolicyInputV1::new(now.saturating_sub(60_000), now.checked_add(600_000).unwrap(),
        &[Authority::new("example.com", 443, EgressTransport::ConnectTcpOpaque).unwrap()],
        PolicyLimits::new(1, 1).unwrap()).unwrap()
}
pub(super) fn replacement(fixture: &Fixture, revision: u64, epoch: u64) -> EgressReplacePolicyRequestV1 {
    EgressReplacePolicyRequestV1::new(uuid::Uuid::new_v4(), fixture.scope(),
        EgressPolicySelectorV1::Tenant, revision, epoch, policy(), None).unwrap()
}
pub(super) fn selector(session: Option<&str>) -> Value {
    session.map_or_else(|| json!({"kind":"tenant"}), |id| json!({"kind":"session","session_id":id}))
}

pub(super) async fn raw(origin: &str, method: &str, path: &str, token: Option<&str>, body: Value)
    -> Result<(u16, HeaderMap, Vec<u8>)>
{
    let http = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never()).connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(15)).build()?;
    let mut request = http.request(method.parse()?, format!("{origin}{path}"))
        .header(header::CONTENT_TYPE, "application/json").body(serde_json::to_vec(&body)?);
    if let Some(token) = token { request = request.bearer_auth(token); }
    let mut response = request.send().await.context("raw inspection transport is not feature absence")?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(chunk.len() <= 4096 - bytes.len(), "inspection exceeded4096bytes");
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, headers, bytes))
}

pub(super) async fn inspect(server: &Server, selected: Value) -> Result<Value> {
    let (status, headers, bytes) = raw(&server.origin, "POST", INSPECT, Some(RUNNER),
        json!({"version":1,"selector":selected})).await?;
    ensure!(matches!(status,200|404|503),"OPI_UNEXPECTED_INSPECTION_STATUS_NOT_FEATURE_ABSENCE actual={status}");
    ensure!(status == 200, "OPI_INSPECTION_EXPECTED_HTTP200_AFTER_REAL_SETUP actual={status}");
    ensure!(headers.get(header::CONTENT_TYPE).is_some_and(|v| v == "application/json")
        && headers.get(header::CACHE_CONTROL).is_some_and(|v| v == "no-store"), "inspection headers");
    let value: Value = serde_json::from_slice(&bytes)?;
    let fields = value.as_object().context("inspection object absent")?;
    ensure!(fields.len() == 10 && ["version","schema_version","scope","selector","head","tenant_ceiling",
        "session_resource","observed_at","coverage","historical"].iter().all(|key| fields.contains_key(*key)), "inspection field contract");
    ensure!(value["version"] == 1 && value["schema_version"] == 15 && value["selector"] == selected
        && value["coverage"] == "policy_registry_metadata" && value["historical"] == true, "inspection correlation");
    chrono::DateTime::parse_from_rfc3339(value["observed_at"].as_str().context("missing observed_at")?)?;
    for forbidden in [RUNNER,"example.com","destinations","not_before_ms","expires_at_ms","source_id",
        "storage_binding","owner","actor","binding_id","policy_digest"] {
        ensure!(!String::from_utf8_lossy(&bytes).contains(forbidden), "inspection leaked forbidden metadata");
    }
    Ok(value)
}

pub(super) fn snapshot(fixture: &Fixture) -> Result<Vec<(String, Vec<Vec<rusqlite::types::Value>>)>> {
    let db = rusqlite::Connection::open_with_flags(&fixture.database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut result = Vec::new();
    for table in ["egress_policy_meta","egress_policy_revisions","egress_policy_heads","egress_policy_operations",
        "egress_policy_reservations","audit_time_floors","events","quota"] {
        let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))?;
        let columns = statement.column_count();
        let rows = statement.query_map([], |row| (0..columns).map(|i| row.get(i)).collect::<rusqlite::Result<Vec<_>>>())?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.push((table.into(), rows));
    }
    Ok(result)
}

fn no_report(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() { return Ok(()); }
    let value: Value = serde_json::from_slice(bytes)?;
    for key in ["scope","selector","head","tenant_ceiling","session_resource","observed_at","coverage"] {
        ensure!(value.get(key).is_none(), "refusal published usable report field");
    }
    Ok(())
}

fn audit_counts(fixture: &Fixture) -> Result<(i64, i64, i64)> {
    let db=rusqlite::Connection::open_with_flags(&fixture.database,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(db.query_row("SELECT count(*),coalesce(max(seq),0),coalesce(sum(CASE WHEN kind='control.identity.access' AND json_extract(payload,'$.data.action')='identity' THEN 1 ELSE 0 END),0) FROM events",
        [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?)
}

// Break caught: empty/guessed heads, wrong authenticated namespace or full
// policy disclosure; a read that writes an event/reserve/floor also fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_inspection_observes_actual_heads_ceiling_and_resource_metadata() -> Result<()> {
    let fixture = Fixture::new(15);
    let mut server = Server::new(&fixture.state).await?;
    let outcome: Result<()> = async {
        server.sdk_witness(&fixture, RUNNER, "A", 15).await?;
        let neighbor = crate::audit::read_test_observer::observe(&fixture.state.tenants[tenant(&fixture.state,"B")].store);
        let before = snapshot(&fixture)?;
        println!("OPI_METADATA_SETUP_REAL_OWNED15_REGISTRY_BINDING_SDK200");
        let first = inspect(&server, selector(None)).await?;
        ensure!(first["scope"] == json!({"tenant_id":"A","control_namespace_uuid":fixture.namespace})
            && first["head"] == json!({"presence":"absent"})
            && first["tenant_ceiling"] == json!({"status":"not_applicable"})
            && first["session_resource"] == "not_applicable", "actual absent tenant metadata");
        for (session, presence) in [(ACTIVE,"active"),(INACTIVE,"inactive"),(ABSENT,"absent")] {
            let read = inspect(&server, selector(Some(session))).await?;
            ensure!(read["head"] == json!({"presence":"absent"}) && read["tenant_ceiling"] == json!({"status":"absent"})
                && read["session_resource"] == presence, "actual resource presence");
        }
        ensure!(snapshot(&fixture)? == before, "inspection changed registry/audit/reserve/clock");
        let sdk = AuditClient::new(&server.origin, RUNNER)?;
        let request = replacement(&fixture, 0, 0);
        let receipt = sdk.egress_replace_policy(&request).await?;
        let receipt_value = serde_json::to_value(&receipt)?;
        let digest = request.policy().to_policy(1)?.digest();
        ensure!(receipt_value["revision"] == 1 && receipt_value["after_epoch"] == 1, "real tenant write");
        let stable = snapshot(&fixture)?;
        let read = inspect(&server, selector(None)).await?;
        ensure!(read["head"] == json!({"presence":"present","revision":1,"epoch":1,"state":"active"}), "actual tenant head");
        let read = inspect(&server, selector(Some(ACTIVE))).await?;
        ensure!(read["tenant_ceiling"] == json!({"status":"active","revision":1,"epoch":1,"digest":hex::encode(digest)}), "exact ceiling");
        ensure!(snapshot(&fixture)? == stable, "metadata read mutated store");
        let session = EgressReplacePolicyRequestV1::new(uuid::Uuid::new_v4(), fixture.scope(),
            EgressPolicySelectorV1::Session(ACTIVE.into()), 0, 0, request.policy().clone(),
            Some(EgressPolicyTenantCeilingV1::new(1,1,digest)?))?;
        sdk.egress_replace_policy(&session).await?;
        let stable = snapshot(&fixture)?;
        let read = inspect(&server, selector(Some(ACTIVE))).await?;
        ensure!(read["head"] == json!({"presence":"present","revision":1,"epoch":1,"state":"active"})
            && read["session_resource"] == "active", "actual session head");
        ensure!(snapshot(&fixture)? == stable, "session read mutated store");
        let revoke = EgressRevokePolicyRequestV1::new(uuid::Uuid::new_v4(), fixture.scope(), EgressPolicySelectorV1::Tenant, 1, 1)?;
        sdk.egress_revoke_policy(&revoke).await?;
        let stable = snapshot(&fixture)?;
        let read = inspect(&server, selector(Some(ACTIVE))).await?;
        ensure!(read["tenant_ceiling"] == json!({"status":"inactive","revision":1,"epoch":2}), "revoked ceiling must not be absent");
        ensure!(snapshot(&fixture)? == stable && neighbor.try_recv().is_err(), "read mutated or reached neighbor");
        Ok(())
    }.await;
    let close = server.close().await;
    let drain = fixture.drain().await;
    close?; drain?; outcome
}

async fn pending_refusal(state: &GatewayState, token: Option<&str>, expected: u16) -> Result<()> {
    let polls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&polls);
    let stream = futures_util::stream::poll_fn(move |_| {
        count.fetch_add(1, Ordering::AcqRel);
        std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
    });
    let mut request = axum::http::Request::builder().method("POST").uri(INSPECT)
        .header(header::CONTENT_TYPE,"application/json").header(header::CONTENT_LENGTH,"1024");
    if let Some(token) = token { request = request.header(header::AUTHORIZATION,format!("Bearer {token}")); }
    let response = tokio::time::timeout(OBSERVE, gateway_router(state.clone()).oneshot(request.body(Body::from_stream(stream))?))
        .await.context("before-body observation watchdog is not refusal")??;
    ensure!(response.status().as_u16() == expected && polls.load(Ordering::Acquire) == 0, "refusal polled body or wrong status");
    no_report(&to_bytes(response.into_body(),4096).await?)
}

// Break caught: granting reader authority to another credential class, polling
// an unauthorized upload, or admitting public/malformed inspection aliases.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_refuses_role_collisions_and_unreserved_methods_before_body() -> Result<()> {
    let fixture = Fixture::new(15);
    let mut server = Server::new(&fixture.state).await?;
    let outcome: Result<()> = async {
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;
        println!("OPI_REFUSALS_SETUP_REAL_OWNED15_REGISTRY_BINDING_SDK200");
        inspect(&server,selector(None)).await?;
        let before = snapshot(&fixture)?;
        let neighbor = crate::audit::read_test_observer::observe(&fixture.state.tenants[tenant(&fixture.state,"B")].store);
        pending_refusal(&fixture.state,None,401).await?;
        pending_refusal(&fixture.state,Some("synthetic-A-viewer"),403).await?;
        for (method,path) in [("POST","/api/v1/egress/policies/inspect"),("HEAD",INSPECT),("GET",INSPECT),
            ("POST","/control/api/v1/egress/policies/inspect?tenant_id=B"),("POST","/control/api/v1/egress/policies/unknown")] {
            let (status,_,bytes) = raw(&server.origin,method,path,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await?;
            ensure!(matches!(status,400|404|405|503), "unreserved method/path was admitted"); no_report(&bytes)?;
        }
        for field in ["tenant_id","expected_scope","source_id","principal","actor","clock","operation_id","unknown"] {
            let mut body = json!({"version":1,"selector":selector(None)});
            body[field] = json!("synthetic-untrusted");
            let (status,_,bytes) = raw(&server.origin,"POST",INSPECT,Some(RUNNER),body).await?;
            ensure!(status == 400,"unknown authority/body field accepted"); no_report(&bytes)?;
        }
        ensure!(snapshot(&fixture)? == before && neighbor.try_recv().is_err(),"refusal changed or reached neighbor");
        Ok(())
    }.await;
    let close=server.close().await; let drain=fixture.drain().await;
    close?; drain?; outcome?;

    let old = Fixture::new(14);
    let mut old_server = Server::new(&old.state).await?;
    let old_outcome: Result<()> = async {
        old_server.sdk_witness(&old,RUNNER,"A",14).await?;
        pending_refusal(&old.state,Some(RUNNER),409).await?;
        let (status,_,bytes)=raw(&old_server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await?;
        ensure!(status == 409 && serde_json::from_slice::<Value>(&bytes)?["error"]["code"] == "egress_policy_migration_required","format14 migrated or misreported");
        Ok(())
    }.await;
    let close=old_server.close().await; let drain=old.drain().await;
    close?; drain?; old_outcome?;
    collision_refusals().await?;
    legacy_refusal().await
}

async fn legacy_refusal()->Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let files=tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir())?)?;
    let path=files.path().join("legacy.sqlite");
    let store=Store::open_for_tenant(&path,"A",Redactor::new(vec![]),10_000)?;
    store.owned_control_migrate_legacy_fixture_for_test(files.path())?;
    let state=GatewayState::for_tenants(vec![crate::api::TenantGateway {
        tenant_id:"A".into(),runner_token:RUNNER.into(),viewer_token:"synthetic-A-viewer".into(),store:Arc::new(store),
    }],Policy::default(),None,Redactor::new(vec![]))?;
    ensure!(state.owned_human.is_none(),"legacy witness accidentally owned");
    let mut server=Server::new(&state).await?;
    let outcome:Result<()>=async {
        let (status,_,bytes)=raw(&server.origin,"GET","/api/v1/control/protocol",Some(RUNNER),json!({})).await?;
        ensure!(status==200&&serde_json::from_slice::<Value>(&bytes)?["schema_version"]==13,"legacy positive control invalid");
        let db=rusqlite::Connection::open_with_flags(&path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let before:i64=db.query_row("SELECT count(*) FROM events",[],|r|r.get(0))?;
        let (status,_,bytes)=raw(&server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await?;
        ensure!(matches!(status,404|405|503),"legacy control admitted inspection");no_report(&bytes)?;
        let after:i64=db.query_row("SELECT count(*) FROM events",[],|r|r.get(0))?;
        ensure!(before==after,"legacy refusal mutated audit");Ok(())
    }.await;
    let close=server.close().await;close?;outcome
}

async fn collision_refusals() -> Result<()> {
    use super::owned_control_tests::negative::{local_credential, provision_b, reserve_workload_fixture_worker};
    use sha2::{Digest,Sha256};
    for workload in [false,true] {
        let mut worker=None;
        let mut files=Store::owned_control_fixture_with_legacy_setup_for_test(15,|legacy| {
            if workload { worker=Some(reserve_workload_fixture_worker(legacy)); }
        });
        let (bundle,credentials,tokens)=files.take_startup();
        provision_b(&bundle);
        if let Some(worker)=worker {
            let descriptor=bundle.owner.closure().sources.iter().find(|s|s.tenant_id.as_deref()==Some("A")).unwrap();
            let source=bundle.owner.source_token(descriptor.source_id)?;
            let epoch=bundle.owner.try_write_epoch()?;
            let access=HumanSourceAccess::write(&epoch,&source)?;
            bundle.stores.get(&descriptor.source_id).unwrap().issue_workload_binding_scoped(&access,&crate::workload::WorkloadBindingIssueV1 {
                version:1,operation_id:uuid::Uuid::new_v4().to_string(),credential_id:uuid::Uuid::new_v4().to_string(),expected_revision:0,
                subject:crate::workload::WorkloadSubjectV1::Worker { worker_id:worker.worker_id,attempt_id:worker.attempt.attempt_id,generation:1 },
                sha256_digest:hex::encode(Sha256::digest(RUNNER.as_bytes())),
                expires_at:(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339(),
            })?;
        } else { local_credential(&bundle,"B",RUNNER,false); }
        let fixture=Fixture::from_parts(Box::new(files),bundle,credentials,tokens,15);
        let mut server=Server::new(&fixture.state).await?;
        let outcome:Result<()>=async {
            server.sdk_witness(&fixture,"synthetic-B-runner","B",15).await?;
            let observers=fixture.state.tenants.iter().map(|t|crate::audit::read_test_observer::observe(&t.store)).collect::<Vec<_>>();
            pending_refusal(&fixture.state,Some(RUNNER),503).await?;
            ensure!(observers.iter().all(|o|o.try_recv().is_err()),"collision reached either Store");
            Ok(())
        }.await;
        let close=server.close().await; let drain=fixture.drain().await;
        close?; drain?; outcome?;
    }
    Ok(())
}

struct HeldMutex { release: Option<mpsc::Sender<()>>, worker: Option<thread::JoinHandle<()>> }
impl HeldMutex {
    fn new(store:Arc<Store>)->Self {
        let (ready,entered)=mpsc::channel(); let (release,released)=mpsc::channel();
        let worker=thread::spawn(move||store.modern_test_hold_mutex(ready,released));
        let held=Self {release:Some(release),worker:Some(worker)};
        entered.recv_timeout(OBSERVE).unwrap(); held
    }
    fn close(mut self) {self.release.take().unwrap().send(()).unwrap();self.worker.take().unwrap().join().unwrap();}
}
impl Drop for HeldMutex {
    fn drop(&mut self) {
        if let Some(release)=self.release.take(){let _=release.send(());}
        if let Some(worker)=self.worker.take(){let _=worker.join();}
    }
}

// Break caught: treating a busy/unhealthy source as absence, leaking the
// precommit report after authority changes, or releasing abandoned work early.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_never_publishes_unhealthy_busy_or_stale_observations() -> Result<()> {
    let fixture=Fixture::new(15);
    let mut server=Server::new(&fixture.state).await?;
    let outcome:Result<()>=async {
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;
        println!("OPI_HEALTH_SETUP_REAL_OWNED15_REGISTRY_BINDING_SDK200");
        inspect(&server,selector(None)).await?;
        let store=Arc::clone(&fixture.state.tenants[tenant(&fixture.state,"A")].store);
        store.owned_control_set_busy_timeout_for_test(713)?;
        let neighbor=crate::audit::read_test_observer::observe(&fixture.state.tenants[tenant(&fixture.state,"B")].store);
        let held=HeldMutex::new(Arc::clone(&store));
        let refusal=raw(&server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await;
        held.close();
        let (status,_,bytes)=refusal?; ensure!(status==503,"busy mutex accepted");no_report(&bytes)?;
        ensure!(store.healthy()&&store.owned_control_busy_timeout_for_test()?==713,"mutex changed health/timeout");
        inspect(&server,selector(None)).await?;
        let lock=rusqlite::Connection::open(&fixture.database)?;
        lock.execute_batch("BEGIN IMMEDIATE")?;
        let refusal=raw(&server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await;
        let timeout=store.owned_control_busy_timeout_for_test()?;
        lock.execute_batch("ROLLBACK")?;
        let (status,_,bytes)=refusal?;ensure!(status==503&&timeout==713,"SQLite busy accepted/timeout lost");no_report(&bytes)?;
        inspect(&server,selector(None)).await?;
        ensure!(neighbor.try_recv().is_err(),"busy reached neighbor");

        // A different real audited read on the SAME retained Store produces
        // actual SQLITE_FULL. No trigger/schema bypass or physical disk claim.
        let owned=fixture.state.owned_human.as_ref().unwrap();
        let a=tenant(&fixture.state,"A");
        let principal=crate::control::review_binding::tests::provision().policy.owner;
        let audited=||->Result<_>{
            let epoch=owned.owner.try_read_epoch()?;
            let access=HumanSourceAccess::read(&epoch,&owned.sources[a])?;
            store.modern_human_profile(&access,&principal,&Redactor::new(vec![]))
        };
        let positive_before=audit_counts(&fixture)?;
        ensure!(audited()?.account.is_some(),"audited positive control absent");
        let positive_after=audit_counts(&fixture)?;
        ensure!(positive_after==(positive_before.0+1,positive_before.1+1,positive_before.2+1),
            "positive profile control did not commit a genuine identity-read audit event");
        let page_limit=store.modern_test_limit_audit_pages()?;
        let mut rollback=None;
        for _ in 0..128 {
            let before=snapshot(&fixture)?;
            let usage=store.modern_test_human_usage();
            let counts=audit_counts(&fixture)?;
            match audited() {
                Ok(profile)=>{
                    ensure!(profile.account.is_some()&&store.healthy(),"successful audited control invalid");
                    ensure!(audit_counts(&fixture)?==(counts.0+1,counts.1+1,counts.2+1),
                        "bounded successful profile read did not actually audit");
                }
                Err(error)=>{
                    ensure!(error.chain().any(|cause|matches!(cause.downcast_ref::<rusqlite::Error>(),
                        Some(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DiskFull
                            && code.extended_code==rusqlite::ffi::SQLITE_FULL)),"page cap did not cause actual SQLITE_FULL");
                    // Audit floors may advance independently in this audited
                    // read; every other table and cached usage must roll back.
                    let after=snapshot(&fixture)?;
                    ensure!(before.iter().filter(|(t,_)|t!="audit_time_floors").eq(after.iter().filter(|(t,_)|t!="audit_time_floors"))
                        && store.modern_test_human_usage()==usage&&!store.healthy(),"SQLITE_FULL leaked audit data/usage or failed to latch");
                    rollback=Some(after);break;
                }
            }
            ensure!(store.owned_control_busy_timeout_for_test()?==713,"audited timeout restoration");
        }
        let rollback=rollback.context("bounded page cap did not reach SQLITE_FULL")?;
        ensure!(store.owned_control_busy_timeout_for_test()?==713,"FULL timeout restoration");
        let (status,_,bytes)=raw(&server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await?;
        ensure!(status==503,"unhealthy inspection accepted");no_report(&bytes)?;
        page_limit.restore()?;
        ensure!(!store.healthy(),"fixture capacity restoration cleared unhealthy latch");
        let (status,_,bytes)=raw(&server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await?;
        ensure!(status==503&&snapshot(&fixture)?==rollback&&store.owned_control_busy_timeout_for_test()?==713,"latched refusal changed evidence");
        no_report(&bytes)?;Ok(())
    }.await;
    let close=server.close().await;let drain=fixture.drain().await;
    close?;drain?;outcome?;
    postcommit_authority_change(false).await?;
    postcommit_authority_change(true).await?;
    abandoned_inspection_drains().await?;
    damaged_source_refuses().await
}

fn inspection_request() -> axum::http::Request<Body> {
    axum::http::Request::builder().method("POST").uri(INSPECT)
        .header(header::AUTHORIZATION,format!("Bearer {RUNNER}"))
        .header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(serde_json::to_vec(&json!({"version":1,"selector":selector(None)})).unwrap())).unwrap()
}

async fn postcommit_authority_change(revoke:bool)->Result<()> {
    use crate::api::auth::owned_control::test_hook;
    let mut files=Store::owned_control_fixture_for_test(15);
    let (bundle,credentials,tokens)=files.take_startup();
    let command=super::owned_control_tests::negative::command(&bundle,"A");
    let fixture=Fixture::from_parts(Box::new(files),bundle,credentials,tokens,15);
    let mut server=Server::new(&fixture.state).await?;
    let outcome:Result<()>=async {
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;inspect(&server,selector(None)).await?;
        let (reached,release)=test_hook::install(&fixture.state);
        let router=gateway_router(fixture.state.clone());
        let handler=tokio::spawn(async move{router.oneshot(inspection_request()).await});
        let reached_result=tokio::task::spawn_blocking(move||reached.recv_timeout(Duration::from_secs(10))).await?;
        if reached_result.is_err(){handler.abort();let _=handler.await;anyhow::bail!("inspection did not reach actual postcommit hook");}
        let owned=fixture.state.owned_human.as_ref().unwrap();let a=tenant(&fixture.state,"A");
        let change_result:Result<()>= (||{
            let epoch=owned.owner.try_write_epoch()?;
            if revoke {
                fixture.state.tenants[a].store.revoke_review_binding(&epoch,&owned.sources[a],
                    &crate::control::review_binding::RevokeRunnerReviewBindingV1 {
                        version:1,operation_id:uuid::Uuid::new_v4().to_string(),
                        operator_delegation_id:command.operator_delegation_id.clone(),binding_id:command.binding.binding_id.clone(),
                        expected_revision:1,expected_binding_digest:command.binding.digest()?,
                        reason:crate::control::review_binding::ReviewBindingRevocationReasonV1::OperatorRevoked,
                    })?;
            }else{
                fixture.state.tenants[a].store.mutate_identity_scoped(&epoch,&owned.sources[a],&IdentityMutationV1 {
                    version:1,operation_id:uuid::Uuid::new_v4().to_string(),
                    actor:PrincipalRefV1::LegacyControl{credential_id:"synthetic-generation-control".into(),role:LegacyControlRoleV1::Runner},
                    change:IdentityChangeV1::Credential{expected_revision:0,binding:LocalCredentialV1 {
                        version:1,credential_id:uuid::Uuid::new_v4().to_string(),principal:command.policy.owner.clone(),
                        sha256_digest:"ab".repeat(32),revision:1,active:false,expires_at:None,
                    }},
                })?;
            }Ok(())
        })();
        let ack=release.release_with_ack()?;
        let acknowledged=tokio::task::spawn_blocking(move||ack.recv_timeout(Duration::from_secs(10))).await?;
        let response=tokio::time::timeout(OBSERVE,handler).await.context("inspection publication watchdog")???;
        change_result?;acknowledged.context("actual job did not acknowledge successful release receive")?;
        ensure!(response.status()==StatusCode::SERVICE_UNAVAILABLE,"postcommit authority change published report");
        no_report(&to_bytes(response.into_body(),4096).await?)
    }.await;
    let close=server.close().await;let drain=fixture.drain().await;
    close?;drain?;outcome
}

async fn abandoned_inspection_drains()->Result<()> {
    use crate::api::{auth::owned_control::test_hook,store_io};
    let fixture=Fixture::new(15);let mut server=Server::new(&fixture.state).await?;
    let outcome:Result<()>=async {
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;inspect(&server,selector(None)).await?;
        let a=tenant(&fixture.state,"A");let owned=fixture.state.owned_human.as_ref().unwrap();
        let semaphore=|gate:&Arc<store_io::Gate>|{let permit=gate.admit(store_io::Lane::Control,false).unwrap();Arc::clone(permit.semaphore())};
        let shared=semaphore(&fixture.state.io);let local=semaphore(&fixture.state.tenants[a].io);
        let operation=fixture.state.tenants[a].control_capacity.runner_semaphore_for_test();
        let mut reserved=Vec::new();
        for sem in [&shared,&local,&operation] {
            reserved.push(Arc::clone(sem).try_acquire_many_owned(u32::try_from(sem.available_permits()-1)?)?);
        }
        let (reached,release)=test_hook::install(&fixture.state);
        let router=gateway_router(fixture.state.clone());let handler=tokio::spawn(async move{router.oneshot(inspection_request()).await});
        let reached_result=tokio::task::spawn_blocking(move||reached.recv_timeout(Duration::from_secs(10))).await?;
        if reached_result.is_err(){handler.abort();let _=handler.await;anyhow::bail!("abandoned inspection did not reach committed hook");}
        ensure!(owned.usages.load(Ordering::Acquire)==1&&[&shared,&local,&operation].iter().all(|s|s.available_permits()==0),"accepted work did not retain permits");
        handler.abort();ensure!(handler.await.unwrap_err().is_cancelled(),"actual Router observer not dropped");
        ensure!(owned.usages.load(Ordering::Acquire)==1,"observer abandonment released accepted job");
        let retirement=fixture.state.human_retirement().unwrap();retirement.retire();let drained=retirement.drain();tokio::pin!(drained);
        ensure!(futures_util::poll!(drained.as_mut()).is_pending(),"owner drained while accepted job held");
        let ack=release.release_with_ack()?;
        tokio::task::spawn_blocking(move||ack.recv_timeout(Duration::from_secs(10))).await??;
        tokio::time::timeout(OBSERVE,drained.as_mut()).await.context("accepted inspection did not really drain")??;
        ensure!(owned.usages.load(Ordering::Acquire)==super::CLOSED,"retired usage did not drain");
        ensure!([&shared,&local,&operation].iter().all(|s|s.available_permits()==1),"accepted permits not restored at actual drain");
        drop(reserved);Ok(())
    }.await;
    let close=server.close().await;let drain=fixture.drain().await;
    close?;drain?;outcome
}

async fn damaged_source_refuses()->Result<()> {
    use std::io::Write;
    for configuration in [false,true] {
        let fixture=Fixture::new(15);let mut server=Server::new(&fixture.state).await?;
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;inspect(&server,selector(None)).await?;
        let held=fixture.database.with_extension("inspection-private-held");
        if configuration {
            let mut file=std::fs::OpenOptions::new().append(true).open(fixture.credentials.path())?;
            file.write_all(b"\n")?;file.sync_all()?;
        }else{std::fs::rename(&fixture.database,&held)?;}
        let result=raw(&server.origin,"POST",INSPECT,Some(RUNNER),json!({"version":1,"selector":selector(None)})).await;
        if !configuration{std::fs::rename(&held,&fixture.database)?;}
        let close=server.close().await;
        let retirement=fixture.state.human_retirement().unwrap();retirement.retire();
        let drained=tokio::time::timeout(OBSERVE,retirement.drain()).await?;
        close?;
        ensure!(drained.as_ref().err().and_then(|e|e.downcast_ref::<crate::human_authority::HumanAuthorityFailure>())
            ==Some(&crate::human_authority::HumanAuthorityFailure::OwnerInvalid),"damaged source did not retain real invalid owner");
        let (status,_,bytes)=result?;ensure!(status==503,"damaged source published report");no_report(&bytes)?;
    }Ok(())
}
