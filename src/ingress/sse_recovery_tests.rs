//! D3 exchange-pool discriminator, independent of the human read lane.
//! Real authenticated human response Bodies are parked by a test-only HTTP
//! scheduling barrier before their first poll. Their real SSE leases remain
//! held, but this does not reproduce 64 naturally polling/heartbeat-live SSEs.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_control_router, gateway_router},
    audit::Store,
    gateway_server::GatewayServer,
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1, LocalCredentialV1,
        LocalHumanProvisionV1, MembershipV1, PrincipalRefV1, ResourceGrantV1, ResourceKindV1,
        ResourceRefV1, generate_local_credential,
    },
    model::{Event, StoredEvent},
    policy::Policy,
    redaction::Redactor,
};
use anyhow::{Result, anyhow, ensure};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, net::SocketAddr, os::unix::fs::PermissionsExt};
use tokio::{
    net::TcpListener,
    sync::{oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

const CONTEXT_HEADER: &str = "x-aiguard-context";
const STREAM_COUNT: usize = 64;
const JSON_LIMIT: usize = 64 * 1024;
const PARK_FRAME: &[u8] = b": fixture-body-parked\n\n";

#[derive(Clone)]
struct BodyBarrier {
    entered: tokio::sync::mpsc::Sender<()>,
    release: watch::Receiver<bool>,
}
impl BodyBarrier {
    async fn park(self, request: Request, next: axum::middleware::Next) -> Response {
        let is_stream = request.uri().path() == "/api/v1/human/stream";
        let response = next.run(request).await;
        if !is_stream || response.status() != StatusCode::OK {
            return response;
        }
        let (parts, body) = response.into_parts();
        let held = async_stream::stream! {
            // This acknowledges a real HTTP response-body poll after the
            // authenticated handler acquired its actual stream lease.
            let _ = self.entered.send(()).await;
            let mut release = self.release;
            let mut pulse = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    biased;
                    changed = release.changed() => {
                        if changed.is_err() { return; }
                        if *release.borrow() { break; }
                    }
                    _ = pulse.tick() => {
                        // Explicit fixture scheduling marker, never counted as
                        // a product heartbeat or native audit observation.
                        yield Ok::<_,axum::Error>(axum::body::Bytes::from_static(PARK_FRAME));
                    }
                }
            }
            let mut data = body.into_data_stream();
            while let Some(chunk) = data.next().await {
                yield chunk;
                }
        };
        Response::from_parts(parts, Body::from_stream(held))
    }
}
struct Reader {
    principal: PrincipalRefV1,
    tenant: String,
    token: String,
    context: String,
}
struct Fixture {
    files: Arc<tempfile::TempDir>,
    state: GatewayState,
    stores: Vec<Arc<Store>>,
    pressure: Vec<Reader>,
    victim: Reader,
    runner: String,
}
fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-control-operator".into(),
            role: crate::identity::LegacyControlRoleV1::Runner,
        },
        change,
    }
}
fn reader(store: &Store) -> Result<Reader> {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "840f3353-1dd5-43ac-96d6-b9765383fca2".into(),
        account_id: Uuid::new_v4().to_string(),
    };
    let token = generate_local_credential()?;
    store.provision_local_human(&LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "D3 synthetic reader".into(),
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
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
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
                    tenant_id: store.tenant_id().into(),
                    organization_scope: "d3-fixture-organization".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            }),
        ],
    })?;
    Ok(Reader {
        principal,
        tenant: store.tenant_id().into(),
        token,
        context: String::new(),
    })
}
impl Fixture {
    fn new() -> Result<Self> {
        let files = Arc::new(
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()?,
        );
        let mut tenants = Vec::new();
        let mut stores = Vec::new();
        let mut pressure = Vec::new();
        let mut victim = None;
        let mut runner = String::new();
        for (index, name) in ["tenant-a", "tenant-b", "tenant-c", "tenant-d"]
            .iter()
            .enumerate()
        {
            let path = files.path().join(format!("{name}.sqlite"));
            let store = Arc::new(Store::open_for_tenant(
                &path,
                name,
                Redactor::new(vec![]),
                2048,
            )?);
            store.migrate_control_v3(&path.with_extension("before3"))?;
            store.migrate_identity_v4(&path.with_extension("before4"))?;
            store.migrate_human_access_v5(&path.with_extension("before5"))?;
            // 6*4 + 6*4 + 4*4 = 64. Human limits are 24/tenant and
            // 4/principal, inside the separate total32 PER TenantState.
            if index < 3 {
                for _ in 0..if index == 2 { 4 } else { 6 } {
                    pressure.push(reader(&store)?);
                }
            } else {
                victim = Some(reader(&store)?);
            }
            let token = format!("d3-fixture-runner-{name}-").repeat(3);
            if index == 3 {
                runner = token.clone();
            }
            tenants.push(TenantGateway {
                tenant_id: (*name).into(),
                store: Arc::clone(&store),
                runner_token: token,
                viewer_token: format!("d3-fixture-viewer-{name}-").repeat(3),
            });
            stores.push(store);
        }
        let state =
            GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![]))?;
        Ok(Self {
            files,
            state,
            stores,
            pressure,
            victim: victim.ok_or_else(|| anyhow!("fixture victim absent"))?,
            runner,
        })
    }
}

struct OwnedGateway {
    public: String,
    control: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl OwnedGateway {
    async fn start(
        state: GatewayState,
        files: Arc<tempfile::TempDir>,
        barrier: BodyBarrier,
    ) -> Result<Self> {
        let owner = GatewayServer::new(state.control_recovery_retirement());
        let (stop, wait) = oneshot::channel();
        let (announce, ready) = oneshot::channel();
        let address: SocketAddr = "127.0.0.1:0".parse()?;
        let mut task = tokio::spawn(async move {
            let _files = files;
            owner
                .serve(
                    (
                        address,
                        gateway_router(state.clone()).layer(axum::middleware::from_fn(
                            move |request: Request, next: axum::middleware::Next| {
                                let barrier = barrier.clone();
                                async move { barrier.park(request, next).await }
                            },
                        )),
                    ),
                    Some((address, gateway_control_router(state))),
                    async {
                        let _ = wait.await;
                    },
                    move |public, control| {
                        announce
                            .send((public, control))
                            .map_err(|_| anyhow!("fixture startup observer lost"))
                    },
                )
                .await
        });
        let ready = tokio::time::timeout(Duration::from_secs(5), ready).await;
        match ready {
            Ok(Ok((public, Some(control)))) => Ok(Self {
                public: format!("http://{public}"),
                control: format!("http://{control}"),
                stop: Some(stop),
                task: Some(task),
            }),
            _ => {
                let _ = stop.send(());
                if tokio::time::timeout(Duration::from_secs(60), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
                Err(anyhow!(
                    "fixture gateway did not bind both owned loopback listeners"
                ))
            }
        }
    }
    async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut task = self
            .task
            .take()
            .ok_or_else(|| anyhow!("fixture gateway owner absent"))?;
        match tokio::time::timeout(Duration::from_secs(60), &mut task).await {
            Ok(joined) => joined.map_err(|_| anyhow!("cleanup: gateway owner failed"))?,
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(anyhow!("cleanup: gateway observation deadline"))
            }
        }
    }
}
impl Drop for OwnedGateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

struct OwnedIngress {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl OwnedIngress {
    async fn start(router: Router) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (stop, mut wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let outcome: Result<()> = async {
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut wait => break,
                        joined = connections.join_next(), if !connections.is_empty() => {
                            if let Some(joined) = joined {
                                joined.map_err(|_| anyhow!("fixture ingress connection task failed"))?;
                            }
                        },
                        accepted = listener.accept() => {
                            let (stream, _) = accepted?;
                            let service = TowerToHyperService::new(router.clone());
                            connections.spawn(async move {
                                let builder = Builder::new(TokioExecutor::new());
                                let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                            });
                        }
                    }
                }
                Ok(())
            }
            .await;
            drop(listener);
            connections.abort_all();
            while connections.join_next().await.is_some() {}
            outcome
        });
        Ok(Self {
            url,
            stop: Some(stop),
            task: Some(task),
        })
    }
    async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut task = self
            .task
            .take()
            .ok_or_else(|| anyhow!("fixture ingress owner absent"))?;
        match tokio::time::timeout(Duration::from_secs(60), &mut task).await {
            Ok(joined) => joined.map_err(|_| anyhow!("cleanup: ingress owner failed"))?,
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(anyhow!("cleanup: ingress observation deadline"))
            }
        }
    }
}
impl Drop for OwnedIngress {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn bounded_json(mut response: reqwest::Response) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow!("fixture JSON body failed"))?
        {
            ensure!(
                chunk.len() <= JSON_LIMIT.saturating_sub(bytes.len()),
                "fixture JSON byte bound exceeded"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow!("fixture JSON deadline elapsed"))??;
    Ok(serde_json::from_slice(&bytes)?)
}
async fn request(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    tokio::time::timeout(Duration::from_secs(5), request.send())
        .await
        .map_err(|_| anyhow!("fixture response headers timed out"))?
        .map_err(|_| anyhow!("fixture HTTP transport failed"))
}
async fn context(client: &reqwest::Client, origin: &str, reader: &mut Reader) -> Result<()> {
    let response = request(
        client
            .get(format!("{origin}/api/v1/human/identity"))
            .bearer_auth(&reader.token),
    )
    .await?;
    ensure!(
        response.status() == StatusCode::OK,
        "setup: real human authentication failed"
    );
    let identity = bounded_json(response).await?;
    let entries = identity["contexts"]
        .as_array()
        .ok_or_else(|| anyhow!("setup: real context response absent"))?;
    ensure!(
        entries.len() == 1 && entries[0]["tenant_id"] == reader.tenant,
        "setup: authenticated tenant context mismatch"
    );
    reader.context = entries[0]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("setup: real context ID absent"))?
        .into();
    Ok(())
}
#[derive(Default)]
struct StreamOwners {
    entries: Vec<(watch::Receiver<(u64, bool)>, JoinHandle<()>)>,
}
impl StreamOwners {
    fn push(&mut self, mut response: reqwest::Response) {
        let (observed, receiver) = watch::channel((0u64, false));
        let task = tokio::spawn(async move {
            let result: Result<()> = async {
                let mut bytes = Vec::new();
                loop {
                    let chunk = response
                        .chunk()
                        .await
                        .map_err(|_| anyhow!("fixture body transport failed"))?
                        .ok_or_else(|| anyhow!("fixture body ended"))?;
                    ensure!(
                        chunk.len() <= 8192usize.saturating_sub(bytes.len()),
                        "fixture frame bound"
                    );
                    bytes.extend_from_slice(&chunk);
                    while let Some(end) = bytes.windows(2).position(|part| part == b"\n\n") {
                        ensure!(
                            &bytes[..end + 2] == PARK_FRAME,
                            "fixture body scheduling marker absent"
                        );
                        let next = observed
                            .borrow()
                            .0
                            .checked_add(1)
                            .ok_or_else(|| anyhow!("fixture observation overflow"))?;
                        observed.send_replace((next, false));
                        bytes.drain(..end + 2);
                    }
                }
            }
            .await;
            let _ = result;
            observed.send_modify(|entry| entry.1 = true);
        });
        self.entries.push((receiver, task));
    }
    async fn barrier(&self, stage: &'static str) -> Result<()> {
        ensure!(
            !self.entries.is_empty() && self.entries.len() <= STREAM_COUNT,
            "setup: admitted body owners required"
        );
        let mut waits = futures_util::stream::FuturesUnordered::new();
        for (receiver, task) in &self.entries {
            let mut receiver = receiver.clone();
            let before = *receiver.borrow();
            ensure!(
                !before.1 && !task.is_finished(),
                "setup: parked body ended; stage={stage}"
            );
            waits.push(async move {
                let after = *receiver
                    .wait_for(|entry| entry.1 || entry.0 > before.0)
                    .await
                    .map_err(|_| anyhow!("setup: parked body observer lost; stage={stage}"))?;
                ensure!(!after.1, "setup: parked body failed; stage={stage}");
                Ok::<(), anyhow::Error>(())
            });
        }
        tokio::time::timeout(Duration::from_secs(12), async {
            while let Some(result) = waits.next().await {
                result?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow!("setup: body scheduling barrier deadline; stage={stage}"))??;
        ensure!(
            self.entries
                .iter()
                .all(|(entry, task)| !entry.borrow().1 && !task.is_finished()),
            "setup: parked body failed while siblings reached barrier; stage={stage}"
        );
        Ok(())
    }
    async fn finish(&mut self) -> Result<()> {
        for (_, task) in &self.entries {
            task.abort();
        }
        let mut panicked = false;
        for (_, task) in self.entries.drain(..) {
            match task.await {
                Ok(()) => (),
                Err(error) if error.is_cancelled() => (),
                Err(_) => panicked = true,
            }
        }
        ensure!(!panicked, "cleanup: body observer panicked");
        Ok(())
    }
}
impl Drop for StreamOwners {
    fn drop(&mut self) {
        for (_, task) in &self.entries {
            task.abort();
        }
    }
}
async fn control_canary(
    client: &reqwest::Client,
    origin: &str,
    runner: &str,
    store: &Store,
) -> Result<()> {
    let mut event = Event::new("observer.coverage");
    event.source = "d3-synthetic-control-canary".into();
    event.provenance = "declared".into();
    event.coverage = "partial".into();
    event.data = json!({"fixture":"bounded control audit delivery"});
    let response = request(
        client
            .post(format!("{origin}/control/api/v1/control/events"))
            .bearer_auth(runner)
            .json(&event),
    )
    .await?;
    ensure!(
        response.status() == StatusCode::OK,
        "setup: separate authenticated control channel failed"
    );
    let stored: StoredEvent = serde_json::from_value(bounded_json(response).await?)?;
    ensure!(
        stored.seq > 0 && stored.event.id == event.id,
        "setup: control canary lacks its real receipt"
    );
    let actual = store.events(&crate::audit::EventQuery {
        kind: Some("observer.coverage".into()),
        limit: Some(8),
        ..Default::default()
    })?;
    ensure!(
        actual.iter().any(|entry| entry.event.id == event.id
            && entry.hash == stored.hash
            && entry.seq == stored.seq),
        "setup: control canary receipt was not actually durable in its authenticated tenant"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_authenticated_human_bodies_leave_unary_ingress_available_and_recover_after_drop() {
    let mut fixture = Fixture::new().unwrap();
    let (entered, mut arrivals) = tokio::sync::mpsc::channel(STREAM_COUNT);
    let (_release, release) = watch::channel(false);
    let gateway = OwnedGateway::start(
        fixture.state.clone(),
        Arc::clone(&fixture.files),
        BodyBarrier { entered, release },
    )
    .await
    .unwrap();
    let ingress = OwnedIngress::start(
        ingress_router_with_control(&gateway.public, &gateway.control).unwrap(),
    )
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(2))
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let mut held = StreamOwners::default();
    let mut declined_streams = 0usize;
    let outcome: Result<()> = async {
        for reader in &mut fixture.pressure {
            context(&client, &ingress.url, reader).await?;
        }
        context(&client, &ingress.url, &mut fixture.victim).await?;
        let probe = |origin: &str| {
            client
                .get(format!("{origin}/api/v1/human/resources"))
                .bearer_auth(&fixture.victim.token)
                .header(CONTEXT_HEADER, &fixture.victim.context)
        };
        ensure!(
            request(probe(&ingress.url)).await?.status() == StatusCode::OK,
            "setup: victim read failed before pressure"
        );
        control_canary(&client, &ingress.url, &fixture.runner, &fixture.stores[3]).await?;
        tokio::time::timeout(Duration::from_secs(30), async {
            for reader in &fixture.pressure {
                for _ in 0..4 {
                    let response = request(
                        client
                            .get(format!("{}/api/v1/human/stream", ingress.url))
                            .bearer_auth(&reader.token)
                            .header(CONTEXT_HEADER, &reader.context),
                    )
                    .await?;
                    if response.status() == StatusCode::TOO_MANY_REQUESTS {
                        ensure!(bounded_json(response).await? == json!({"error":"Trop de flux simultanés."}),
                            "setup: stream declined outside the dedicated ingress stream budget");
                        declined_streams += 1;
                        continue;
                    }
                    ensure!(response.status() == StatusCode::OK, "setup: human SSE admission failed");
                    ensure!(
                        response.headers().get(header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| value.split(';').next() == Some("text/event-stream")),
                        "setup: admitted response was not SSE"
                    );
                    held.push(response);
                    arrivals.recv().await.ok_or_else(|| anyhow!("setup: HTTP body barrier owner lost"))?;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow!("setup: bounded 64-stream opening deadline elapsed"))??;
        ensure!(held.entries.len() + declined_streams == STREAM_COUNT,
            "setup: all 64 stream requests must be accounted for");
        held.barrier("pre-probe").await?;
        // Direct gateway and distinct control remain actual, separate canaries.
        // They do not stand in for the public ingress availability oracle.
        ensure!(
            request(probe(&gateway.public)).await?.status() == StatusCode::OK,
            "setup: victim gateway source unavailable"
        );
        control_canary(&client, &ingress.url, &fixture.runner, &fixture.stores[3]).await?;
        let response = request(probe(&ingress.url)).await?;
        let status = response.status();
        let body = bounded_json(response).await?;
        held.barrier("post-probe").await?;
        if status == StatusCode::TOO_MANY_REQUESTS {
            ensure!(body == json!({"error":"Trop de requêtes simultanées."}),
                "setup: 429 did not identify the ingress exchange pool");
        } else {
            ensure!(status == StatusCode::OK, "setup: unexpected probe status");
        }
        // Capture the product oracle, then free all downstream owners and
        // require an actual recovered HTTP read before surfacing a RED.
        held.finish().await?;
        let recovered = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response = request(probe(&ingress.url)).await?;
                let status = response.status();
                let body = bounded_json(response).await?;
                if status == StatusCode::OK { return Ok::<(), anyhow::Error>(()); }
                ensure!(status == StatusCode::TOO_MANY_REQUESTS
                    && body == json!({"error":"Trop de requêtes simultanées."}),
                    "cleanup: recovered probe failed outside ingress capacity");
                tokio::task::yield_now().await;
            }
        }).await.map_err(|_| anyhow!("cleanup: ingress recovery observation deadline elapsed"))?;
        recovered?;
        for store in &fixture.stores {
            ensure!(
                store.workers()?.is_empty() && store.worker_usage()?.is_empty(),
                "fixture unexpectedly created worker/usage"
            );
        }
        ensure!(
            status == StatusCode::OK,
            "D3_POOL: 64 authenticated parked response bodies exhausted the unary ingress pool; status={status}"
        );
        Ok(())
    }
    .await;
    let stream_cleanup = held.finish().await;
    drop(client);
    let ingress_cleanup = ingress.finish().await;
    let gateway_cleanup = gateway.finish().await;
    stream_cleanup.unwrap();
    ingress_cleanup.unwrap();
    gateway_cleanup.unwrap();
    outcome.unwrap();
}

async fn next_audit_frame(
    response: &mut reqwest::Response,
    pending: &mut Vec<u8>,
) -> Result<(String, Vec<String>)> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(end) = pending.windows(2).position(|bytes| bytes == b"\n\n") {
                let frame: Vec<_> = pending.drain(..end + 2).collect();
                let frame = std::str::from_utf8(&frame)?;
                if frame.starts_with(':') {
                    continue;
                }
                ensure!(
                    frame.lines().any(|line| line == "event: human_audit"),
                    "unexpected human SSE frame"
                );
                let field = |prefix| {
                    frame
                        .lines()
                        .find_map(|line| line.strip_prefix(prefix))
                        .ok_or_else(|| anyhow!("missing SSE field {prefix}"))
                };
                let page: serde_json::Value = serde_json::from_str(field("data: ")?)?;
                let ids = page["items"]
                    .as_array()
                    .ok_or_else(|| anyhow!("missing audit items"))?
                    .iter()
                    .map(|item| {
                        item["metadata"]["id"]
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| anyhow!("missing audit event ID"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                return Ok((field("id: ")?.to_owned(), ids));
            }
            let chunk = response
                .chunk()
                .await?
                .ok_or_else(|| anyhow!("SSE ended before audit frame"))?;
            ensure!(
                chunk.len() <= JSON_LIMIT.saturating_sub(pending.len()),
                "SSE fixture frame exceeds bound"
            );
            pending.extend_from_slice(&chunk);
        }
    })
    .await
    .map_err(|_| anyhow!("audit frame deadline elapsed"))?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_sse_reader_releases_permits_and_resumes_from_last_read_cursor() -> Result<()> {
    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let path = files.path().join("reader.sqlite");
    let store = Arc::new(Store::open_for_tenant(
        &path,
        "tenant-a",
        Redactor::new(vec![]),
        2048,
    )?);
    store.migrate_control_v3(&path.with_extension("before3"))?;
    store.migrate_identity_v4(&path.with_extension("before4"))?;
    store.migrate_human_access_v5(&path.with_extension("before5"))?;
    let mut reader = reader(&store)?;
    let session = Uuid::new_v4().to_string();
    let publish = || -> Result<String> {
        let mut event = Event::new("observer.coverage");
        event.session_id = Some(session.clone());
        Ok(store.append(event)?.event.id)
    };
    let initial = publish()?;
    store.mutate_identity(&mutation(IdentityChangeV1::ResourceGrant {
        expected_revision: 0,
        grant: ResourceGrantV1 {
            version: 1,
            tenant_id: reader.tenant.clone(),
            organization_scope: "d3-fixture-organization".into(),
            resource: ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: session.clone(),
            },
            owner: reader.principal.clone(),
            revision: 1,
            active: true,
            shares: Vec::new(),
        },
    }))?;
    let gateway_state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: reader.tenant.clone(),
            store: Arc::clone(&store),
            runner_token: "local-sse-fixture-runner-token".into(),
            viewer_token: "local-sse-fixture-viewer-token".into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )?;
    let gateway_slots = gateway_state.fixture_stream_slots();
    let baseline = gateway_slots.available_permits();
    let (produced, mut observed) = watch::channel(0usize);
    let public = gateway_router(gateway_state).layer(axum::middleware::from_fn(
        move |request: Request, next: axum::middleware::Next| {
            let produced = produced.clone();
            async move {
                let stream = request.uri().path() == "/api/v1/human/stream";
                let response = next.run(request).await;
                if !stream || response.status() != StatusCode::OK {
                    return response;
                }
                let (parts, body) = response.into_parts();
                let body = body.into_data_stream().inspect(move |chunk| {
                    if chunk
                        .as_ref()
                        .is_ok_and(|bytes| bytes.starts_with(b"event: human_audit\n"))
                    {
                        produced.send_modify(|count| *count += 1);
                    }
                });
                Response::from_parts(parts, Body::from_stream(body))
            }
        },
    ));
    let gateway = OwnedIngress::start(public).await?;
    let upstream = Ingress {
        origin: upstream_origin(&gateway.url)?,
        client: reqwest::Client::builder().no_proxy().build()?,
        uploads: Arc::new(tokio::sync::Semaphore::new(MAX_UPLOADS)),
        concurrency: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT)),
        streams: Arc::new(tokio::sync::Semaphore::new(MAX_SSE_EXCHANGES)),
        timeouts: Timeouts::default(),
    };
    let ingress = OwnedIngress::start(
        Router::new()
            .fallback(any(forward))
            .with_state(upstream.clone()),
    )
    .await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .build()?;
    let outcome: Result<()> = async {
        context(&client, &ingress.url, &mut reader).await?;
        let get = |path: &str| {
            client
                .get(format!("{}{path}", ingress.url))
                .bearer_auth(&reader.token)
                .header(CONTEXT_HEADER, &reader.context)
        };
        let stream = || {
            get("/api/v1/human/stream").query(&[
                ("view", "metadata"),
                ("limit", "1"),
                ("resource_kind", "session"),
                ("resource_id", &session),
            ])
        };
        let mut response = request(stream()).await?;
        ensure!(response.status() == StatusCode::OK, "initial SSE refused");
        let (cursor, ids) = next_audit_frame(&mut response, &mut Vec::new()).await?;
        ensure!(ids == [initial], "unexpected initial events");
        ensure!(
            upstream.concurrency.available_permits() == MAX_CONCURRENT - 1
                && upstream.streams.available_permits() == MAX_SSE_EXCHANGES - 1
                && gateway_slots.available_permits() == baseline - 1,
            "stream did not own all expected permits"
        );

        // Stop reading this response while ordinary events are committed and a unary read completes.
        let expected = [publish()?, publish()?, publish()?];
        let unary = request(get("/api/v1/human/resources")).await?;
        ensure!(
            unary.status() == StatusCode::OK,
            "ordinary read failed during pause"
        );
        bounded_json(unary).await?;
        tokio::time::timeout(
            Duration::from_secs(5),
            observed.wait_for(|count| *count >= 2),
        )
        .await
        .map_err(|_| anyhow!("gateway did not produce a frame during the pause"))??;
        drop(response);
        tokio::time::timeout(Duration::from_secs(5), async {
            while upstream.concurrency.available_permits() != MAX_CONCURRENT
                || upstream.streams.available_permits() != MAX_SSE_EXCHANGES
                || gateway_slots.available_permits() != baseline
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("closing the client transport did not free the stream permits"))?;

        let sentinel = publish()?;
        let mut resumed = request(stream().query(&[("cursor", cursor)])).await?;
        ensure!(resumed.status() == StatusCode::OK, "resumed SSE refused");
        let mut pending = Vec::new();
        let mut actual = Vec::new();
        let expected: Vec<_> = expected.into_iter().chain([sentinel]).collect();
        while actual.len() < expected.len() {
            let (_, ids) = next_audit_frame(&mut resumed, &mut pending).await?;
            ensure!(!ids.is_empty(), "empty audit frame after resume");
            actual.extend(ids);
        }
        ensure!(
            actual == expected,
            "resumed events have a gap, duplicate or ordering error"
        );
        Ok(())
    }
    .await;
    let ingress_cleanup = ingress.finish().await;
    let gateway_cleanup = gateway.finish().await;
    outcome?;
    ingress_cleanup?;
    gateway_cleanup
}
