//! Integration control oracles only. Real current CLI/stdin/service/native IPC;
//! no worker, engine, Claim, SyntheticReady, retirement or quota proof.
#![cfg(unix)]

#[path = "support/d7_legacy_cli.rs"]
mod fixture;
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{
        journal::{RejectionCodeV1, RejectionPhaseV1},
        service::protocol::{BrokerCommandV1, BrokerErrorCodeV1, BrokerReplyV1},
        service::{self, RequestedExecutionV1, SubmitV1},
    },
    policy::Policy,
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use axum::{
    body::Body,
    http::{Method, Request},
    middleware::{self, Next},
    response::Response,
};
use fixture::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    process::{Child, Command},
    sync::oneshot,
    task::JoinHandle,
};
use uuid::Uuid;

struct Gateway {
    url: String,
    namespace: Uuid,
    task: Option<JoinHandle<std::io::Result<()>>>,
    shutdown: Option<oneshot::Sender<()>>,
    gets: Arc<AtomicUsize>,
    posts: Arc<AtomicUsize>,
    store: Option<Arc<Store>>,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if self.task.is_some() || self.store.is_some() {
            // No Drop/notification proves a join. Keep the exact uncollected
            // handle and Store; Fixture's fail-closed Drop retains its root.
            std::mem::forget((self.task.take(), self.store.take()));
        }
    }
}
impl Gateway {
    async fn close(&mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let task = self.task.as_mut().context("gateway join owner missing")?;
        match tokio::time::timeout(WAIT, task)
            .await
            .context("test gateway task join timeout")?
        {
            Ok(Ok(())) => {
                // axum's graceful return has drained its connections. Only
                // this positive result releases the retained task and Store.
                self.task.take();
                self.store.take();
                Ok(())
            }
            Ok(Err(error)) => Err(error.into()),
            Err(error) => Err(error.into()),
        }
    }
    async fn start(f: &Fixture) -> Result<Self> {
        let db = f.root.join("control.sqlite");
        let store = Arc::new(Store::open_for_tenant(
            &db,
            TENANT,
            Redactor::new(vec![TOKEN.into()]),
            10_000,
        )?);
        store.migrate_control_v3(&f.root.join("before3.sqlite"))?;
        let namespace = Uuid::parse_str(
            store
                .supervision_state()?
                .control_namespace_uuid
                .as_deref()
                .context("real namespace")?,
        )?;
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: TENANT.into(),
                store: Arc::clone(&store),
                runner_token: TOKEN.into(),
                viewer_token: "synthetic-d7-unused-viewer-bbbbbbbbb".into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![TOKEN.into()]),
        )?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let gets = Arc::new(AtomicUsize::new(0));
        let posts = Arc::new(AtomicUsize::new(0));
        let (g, p) = (Arc::clone(&gets), Arc::clone(&posts));
        let app = gateway_router(state).layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                let (g, p) = (Arc::clone(&g), Arc::clone(&p));
                async move {
                    if request.method() == Method::POST {
                        p.fetch_add(1, Ordering::SeqCst);
                    }
                    if request.method() == Method::GET
                        && request.uri().path() == "/control/api/v1/control/protocol"
                    {
                        g.fetch_add(1, Ordering::SeqCst);
                    }
                    let response: Response = next.run(request).await;
                    response
                }
            },
        ));
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
        });
        Ok(Self {
            url,
            namespace,
            task: Some(task),
            shutdown: Some(shutdown),
            gets,
            posts,
            store: Some(store),
        })
    }
}

async fn close_broker(broker: &mut Option<Child>) -> Result<()> {
    let Some(child) = broker.as_mut() else {
        return Ok(());
    };
    let normal = match child.try_wait() {
        Ok(None) => normal_broker_stop(child).await,
        Ok(Some(_)) => Ok(()),
        Err(error) => Err(error.into()),
    };
    if normal.is_err() {
        let collected = kill_fixture_child(child).await;
        if collected.is_err() {
            std::mem::forget(broker.take());
        } else {
            broker.take();
        }
        return combine(normal, collected);
    }
    broker.take();
    Ok(())
}

#[tokio::test]
async fn actual_service_cli_private_pipe_fresh_then_existing_cold_fence() -> Result<()> {
    let mut f = Fixture::new()?;
    let mut gateway = Gateway::start(&f).await?;
    let mut broker: Option<Child> = None;
    let mut helper: Option<Peer> = None;
    let outcome:Result<()> = async {
        // Every fallible setup step after the gateway is live shares this
        // outcome/cleanup scope, including registry, inventory and spawn.
        let (config,state,_)=registry(&f,&gateway.url,gateway.namespace,1,1)?;
        ensure!(fs::read_dir(state.join("journals"))?.next().is_none(),"fixture journal inventory not empty");
        ensure!(!state.join("enforcer").exists(),"fixture role must genuinely not exist");
        broker=Some(serve(&config,&f.root)?);
        wait_broker(&state.join("broker"),broker.as_mut().context("owned broker missing")?).await?;
        let peer=Peer::read(&state.join("enforcer"))?;helper=Some(peer.clone());
        let first=peer.capabilities().await?;
        ensure!(first["version"]==1&&first["max_slots"]==1&&first["max_recovery_stop_owners"]==1,"actual native configuration differs");
        ensure!(first["recovery_generation"].is_null()&&first["fresh_launch_nonce"].as_str().is_some_and(|s|Uuid::parse_str(s).is_ok_and(|v|!v.is_nil())),"actual spawning parent failed Fresh/native nonce handshake");
        ensure!(first["known_stop_slots"]==0&&first["owned_active"]==0&&!first["recovery_incomplete"].as_bool().unwrap_or(true),"empty installed native role not confirmed");
        ensure!(fs::read_dir(state.join("journals"))?.next().is_none(),"Fresh startup opened/created a previous journal source");
        let lock=fs::metadata(peer.role.join(".owner.lock"))?;let auth=fs::read(peer.role.join("auth"))?;
        let original_announcements=announcements(&peer.role)?;
        // Actual private CLI factory collision, with its own private stdin
        // pipe: EEXIST cannot adopt the first role or replace its auth/epoch.
        let mut collision=fresh_child(&state,"enforcer",1,1,Uuid::new_v4()).await?;
        let status=match tokio::time::timeout(WAIT,collision.wait()).await {
            Ok(Ok(status))=>status,
            Ok(Err(error))=>{
                let cleanup=kill_fixture_child(&mut collision).await;
                if cleanup.is_err(){std::mem::forget(collision);}
                return combine(Err(error.into()),cleanup);
            },
            Err(error)=>{
                let cleanup=kill_fixture_child(&mut collision).await;
                if cleanup.is_err(){std::mem::forget(collision);}
                return combine(Err(error.into()),cleanup);
            },
        };
        ensure!(!status.success(),"colliding real fresh child adopted an existing directory");
        ensure!(announcements(&peer.role)?==original_announcements,"collision published another role");
        ensure!(peer.capabilities().await?["epoch"]==first["epoch"],"collision replaced native epoch");
        normal_broker_stop(broker.as_mut().context("owned broker missing")?).await?;
        peer.idle_shutdown().await?;wait_lease_released(&peer.role).await?;helper=None;
        // Real relaunch on original existing role: source remains truly empty,
        // which still cannot reconstruct Fresh provenance or reopen admission.
        broker=Some(serve(&config,&f.root)?);
        wait_broker_new(&state.join("broker"),broker.as_mut().context("owned broker missing")?,1).await?;
        let cold=Peer::read(&state.join("enforcer"))?;helper=Some(cold.clone());
        let second=cold.capabilities().await?;
        ensure!(second["epoch"]!=first["epoch"]&&second["fresh_launch_nonce"].is_null()&&second["recovery_generation"].as_u64().is_some_and(|g|g>0),"actual existing constructor did not fence before service publication");
        ensure!(fs::read_dir(state.join("journals"))?.next().is_none(),"cold empty-source scan invented journal activity");
        let after=fs::metadata(cold.role.join(".owner.lock"))?;
        ensure!((lock.dev(),lock.ino())==(after.dev(),after.ino())&&auth==fs::read(cold.role.join("auth"))?,"cold relaunch replaced auth/lock");
        let (_,target,_)=target(&cold.role,&gateway.url,gateway.namespace)?;
        let refused=cold.rpc(json!({"kind":"reserve_prospective","prospective":target,"generation":0,"replay_only":false})).await?;
        ensure!(refused["kind"]=="error"&&refused["code"]=="not_ready","cold native role did not apply its admission fence");
        ensure!(gateway.posts.load(Ordering::SeqCst)==0,"control Arm/Claim or other POST occurred");
        Ok(())
    }.await;
    let mut cleanup = close_broker(&mut broker).await;
    if let Some(peer) = helper {
        let stopped = async {
            peer.idle_shutdown().await?;
            wait_lease_released(&peer.role).await
        }
        .await;
        cleanup = combine(cleanup, stopped);
    }
    let gateway_cleanup = gateway.close().await;
    cleanup = combine(cleanup, gateway_cleanup);
    finish(&mut f, outcome, cleanup)
}

async fn wait_broker_new(
    role: &std::path::Path,
    child: &mut tokio::process::Child,
    old_count: usize,
) -> Result<()> {
    let until = tokio::time::Instant::now() + WAIT;
    loop {
        ensure!(
            child.try_wait()?.is_none(),
            "reconnected broker exited before publication"
        );
        if announcements(role)?.len() > old_count && latest_endpoint(role).is_ok() {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "new broker publication missing"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn actual_child_cli_stdin_reserve_installs_native_context_before_any_arm() -> Result<()> {
    let mut f = Fixture::new()?;
    let nonce = Uuid::new_v4();
    let mut child = match fresh_child(&f.root, "r", 1, 1, nonce).await {
        Ok(child) => child,
        Err(error) => {
            f.preserve();
            return Err(error);
        }
    };
    let outcome:Result<()> = async {
        let peer=wait_peer(&f.root.join("r"),&mut child).await?;
        let initial=peer.capabilities().await?;
        ensure!(initial["fresh_launch_nonce"]==json!(nonce)&&initial["recovery_generation"].is_null(),"actual child did not attest the private stdin nonce");
        let (_,prospective,_)=target(&peer.role,"http://127.0.0.1:1",Uuid::new_v4())?;
        let reserved=peer.rpc(json!({"kind":"reserve_prospective","prospective":prospective,"generation":0,"replay_only":false})).await?;
        ensure!(reserved["kind"]=="prospective_reserved","actual native reservation was not installed");
        let observed=peer.capabilities().await?;
        ensure!(observed["known_stop_slots"]==1&&observed["owned_active"]==1,"actual installed native context not observed");
        let (_,second,_)=target(&peer.role,"http://127.0.0.1:1",Uuid::new_v4())?;
        let full=peer.rpc(json!({"kind":"reserve_prospective","prospective":second,"generation":0,"replay_only":false})).await?;
        ensure!(full["kind"]=="error"&&full["code"]=="capacity","W provided hidden fresh admission capacity");
        let status=peer.rpc(json!({"kind":"status"})).await?;ensure!(status["value"]["slot_count"]==1,"status omitted prospective entry");
        let busy=peer.rpc(json!({"kind":"shutdown_if_idle"})).await?;ensure!(busy["kind"]=="error"&&busy["code"]=="not_idle","nonempty helper reported idle");
        // No Arm request or HTTP client is created in this target-only case.
        // The endpoint above is an explicitly unused address, not a probe.
        Ok(())
    }.await;
    let cleanup = kill_fixture_child(&mut child).await;
    if cleanup.is_err() {
        std::mem::forget(child);
    }
    finish(&mut f, outcome, cleanup)
}

const OLD_SHA: &str = "9fe59fc815660c5df96611da824734023424e8b0b01fb2bb3aaf3d9708270c59";
fn old_binary() -> Result<PathBuf> {
    ensure!(
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "pinned old artifact requires Darwin aarch64; absence/platform mismatch is not PASS"
    );
    let path = PathBuf::from(
        std::env::var_os("AI_GUARD_TEST_LEGACY_V1_BIN")
            .context("set pinned old V1 binary explicitly")?,
    );
    let expected = std::env::var("AI_GUARD_TEST_LEGACY_V1_SHA256")
        .context("set mandatory admitted old V1 SHA256")?;
    ensure!(
        path.is_absolute() && expected == OLD_SHA,
        "old binary source pin differs from admitted R4/public e794 artifact"
    );
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    // SAFETY: read-only effective UID of this test control process.
    let uid = unsafe { libc::geteuid() };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.len() == 246466400
            && meta.mode() & 0o7022 == 0
            && meta.mode() & 0o111 != 0
            && meta.uid() == uid,
        "old binary metadata differs"
    );
    let mut hash = Sha256::new();
    let mut file = file.take(246466401);
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hash.update(&buffer[..n]);
    }
    ensure!(
        bytes == 246466400,
        "old artifact changed during bounded hash"
    );
    ensure!(
        hex::encode(hash.finalize()) == expected,
        "old binary byte hash differs"
    );
    Ok(path)
}

#[tokio::test]
#[ignore = "requires explicit pinned pre-Legacy binary and SHA256; Root/Luna run with --ignored, missing prerequisites must fail"]
async fn actual_old_v1_peer_refuses_current_service_arm_and_retains_exact_stop() -> Result<()> {
    let old = old_binary()?;
    let mut f = Fixture::new()?;
    let mut gateway = Gateway::start(&f).await?;
    let mut peer_child: Option<Child> = None;
    let mut broker: Option<Child> = None;
    let outcome:Result<()> = async {
        // Even mkdir/auth/old-peer spawn failures must reach gateway join and
        // fail-closed fixture preservation, rather than bypassing teardown.
        let (config,state,source)=registry(&f,&gateway.url,gateway.namespace,2,0)?;
        let role=state.join("enforcer");
        fs::create_dir(&role)?;
        fs::set_permissions(&role,fs::Permissions::from_mode(0o700))?;
        private(&role.join("auth"),&[0x5a;32])?;
        peer_child=Some(Command::new(&old)
            .args(["__broker-enforcer","--root"]).arg(&role)
            .args(["--max-slots","2"])
            .env_clear().current_dir(&f.root)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .kill_on_drop(true).spawn()?);
        let peer=wait_peer(&role,peer_child.as_mut().context("owned old peer missing")?).await?;
        let (registration,prospective,digest)=target(&role,&gateway.url,gateway.namespace)?;
        let arm=prospective["arm"]["operation_id"].as_str().context("synthetic exact arm")?.to_owned();
        let retained=peer.rpc(json!({"kind":"register","registration":registration,"generation":0,"observe_only":true})).await?;
        ensure!(retained["kind"]=="registered"||(retained["kind"]=="error"&&retained["code"]=="not_ready"),"old V1 did not retain the exact synthetic STOP target");
        let before=peer.rpc(json!({"kind":"slots","arm_ids":[arm]})).await?;
        ensure!(before["kind"]=="slots"&&before["values"][0]["registration_hash"]==digest,"old exact stop identity not retained");
        let failure_before=before["values"][0]["stop_failures"].as_u64().context("old native reopen progress counter")?;
        ensure!(peer.capabilities().await.is_err(),"selected artifact is not an unsupported old peer");
        let epoch=peer.endpoint.epoch();let old_announcements=announcements(&role)?;let lock=fs::metadata(role.join(".owner.lock"))?;let auth=fs::read(role.join("auth"))?;
        broker=Some(serve(&config,&f.root)?);wait_broker(&state.join("broker"),broker.as_mut().unwrap()).await?;
        let submitted=Uuid::new_v4();
        let payload=service::encode_submit(&SubmitV1{version:1,operation_id:submitted,project:source,execution:RequestedExecutionV1::Command{argv:vec!["/usr/bin/id".into()]},timeout_seconds:Some(30),capture_output:false})?;
        let reply=service::request(&state,PROFILE,TOKEN,BrokerCommandV1::Submit{payload}).await?;
        match reply {
            BrokerReplyV1::Rejected{operation_id,phase:RejectionPhaseV1::SubmissionPreparation,code:RejectionCodeV1::PreparationFailed,..} if operation_id==submitted => {
                let detail=service::request(&state,PROFILE,TOKEN,BrokerCommandV1::Status{operation_id:submitted}).await?;
                ensure!(matches!(detail,BrokerReplyV1::Status{status} if status.executor_error==Some(BrokerErrorCodeV1::NeedsReconciliation)),"refusal was not the actual reconciliation/admission gate");
            }
            BrokerReplyV1::Error{code:BrokerErrorCodeV1::NeedsReconciliation}=>{}
            _=>anyhow::bail!("old-peer request did not reach the actual admission refusal"),
        }
        ensure!(gateway.gets.load(Ordering::SeqCst)>0&&gateway.posts.load(Ordering::SeqCst)==0,"request did not authenticate or emitted Arm/Claim/control POST");
        let after=Peer::read(&role)?;ensure!(after.endpoint.epoch()==epoch&&announcements(&role)?==old_announcements,"current broker replaced/took over old peer");
        let cancelled=after.rpc(json!({"kind":"cancel","arm_id":arm,"registration_hash":digest,"generation":9})).await?;ensure!(cancelled["kind"]=="acknowledged","old exact Cancel lost");
        let slots=after.rpc(json!({"kind":"slots","arm_ids":[arm]})).await?;
        ensure!(slots["values"][0]["generation"]==9&&slots["values"][0]["registration_hash"]==digest,"old STOP/Cancel state was discarded");
        let failure_after=slots["values"][0]["stop_failures"].as_u64().context("old post-Cancel native progress counter")?.max(failure_before);
        let progress_deadline=tokio::time::Instant::now()+Duration::from_secs(5);
        loop {
            let live=after.rpc(json!({"kind":"slots","arm_ids":[arm]})).await?;
            ensure!(live["values"][0]["generation"]==9&&live["values"][0]["registration_hash"]==digest,"old exact target changed during native progress");
            if live["values"][0]["stop_failures"].as_u64().is_some_and(|n|n>failure_after){break;}
            ensure!(tokio::time::Instant::now()<progress_deadline,"old native target context stopped attempting reopen after new-peer refusal");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let status=after.rpc(json!({"kind":"status"})).await?;
        ensure!(status["kind"]=="status"&&status["value"]["slot_count"]==1&&status["value"]["max_slots"]==2&&status["value"]["healthy"]==true,"old V1 Status stopped reporting its retained target");
        let now=fs::metadata(role.join(".owner.lock"))?;ensure!(auth==fs::read(role.join("auth"))?&&(now.dev(),now.ino())==(lock.dev(),lock.ino()),"old anchors were rewritten");
        ensure!(peer_child.as_mut().context("owned old peer missing")?.try_wait()?.is_none(),"old peer was terminated by the refusal");
        // NotReady is explicit unknown engine readiness, not an active STOP
        // coverage/effect claim. Exact retained target+Cancel/epoch/anchors are
        // the old-peer compatibility oracles; there is no actual container.
        Ok(())
    }.await;
    let mut cleanup = close_broker(&mut broker).await;
    if let Some(child) = peer_child.as_mut() {
        let peer_cleanup = kill_fixture_child(child).await;
        if peer_cleanup.is_err() {
            std::mem::forget(peer_child.take());
        } else {
            peer_child.take();
        }
        cleanup = combine(cleanup, peer_cleanup);
    }
    let gateway_cleanup = gateway.close().await;
    cleanup = combine(cleanup, gateway_cleanup);
    finish(&mut f, outcome, cleanup)
}
