use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    policy::Policy,
};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use std::{net::SocketAddr, os::unix::fs::PermissionsExt};
use tokio::{net::TcpListener, task::JoinHandle};

#[derive(Clone, Copy, Debug)]
enum Cut {
    BeforeCommit,
    AfterCommit,
}

struct Gateway(JoinHandle<()>);

impl Drop for Gateway {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Gateway {
    async fn stop(&mut self) -> Result<()> {
        self.0.abort();
        let error = (&mut self.0).await.expect_err("gateway task remained live");
        anyhow::ensure!(error.is_cancelled(), "gateway task failed");
        Ok(())
    }
}

fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    runner: String,
    viewer: String,
    cut: Option<Cut>,
    requests: Arc<Mutex<Vec<WorkerMutationV1>>>,
) -> Result<Gateway> {
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "local".into(),
            store,
            runner_token: runner.clone(),
            viewer_token: viewer.clone(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![runner, viewer]),
    )?;
    let app =
        gateway_router(state).layer(middleware::from_fn(move |request: Request, next: Next| {
            let requests = requests.clone();
            async move {
                if request.method() != axum::http::Method::POST
                    || !request.uri().path().ends_with("/transitions")
                {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 64 * 1024).await.unwrap();
                let mutation: WorkerMutationV1 = serde_json::from_slice(&bytes).unwrap();
                assert!(matches!(
                    mutation.transition,
                    WorkerTransitionV1::CancelReserved
                ));
                requests.lock().unwrap().push(mutation);
                if matches!(cut, Some(Cut::BeforeCommit)) {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                let response = next
                    .run(Request::from_parts(parts, Body::from(bytes)))
                    .await;
                if matches!(cut, Some(Cut::AfterCommit)) {
                    assert!(response.status().is_success());
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                response
            }
        }));
    Ok(Gateway(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    })))
}

fn prepare_reservation(root: &Path, origin: &str, store: &Store) -> Result<(RunPlan, WorkerV1)> {
    let mut plan = export_and_mediation_tests::build_fixture_plan(root, false);
    plan.options.gateway = origin.into();
    // A runtime invocation would fail even under a valid control account.
    plan.options.runtime_program = Some(root.join("absent-runtime-program"));
    let policy = Policy::default();
    let decision = crate::policy::evaluate(&policy, &plan.request);
    let stored = store
        .decision(&plan.request, Some((decision.clone(), json!(null))))?
        .context("fixture decision missing")?;
    let before = Manifest::default();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: plan.operation_id.to_string(),
        decision_request: plan.request.clone(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: plan.request.request_id.clone(),
            request_hash: crate::policy::request_hash(&plan.request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id: plan.session.to_string(),
            command: plan.request.command.clone(),
            profile: plan.options.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: plan.options.image.clone(),
            image_id: format!("sha256:{}", "d".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1024 * 1024 * 1024,
            pids_limit: 256,
            timeout_seconds: u32::try_from(plan.options.timeout)?,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: content_hash("ai-guard:workspace-manifest:v1", &before)?,
            workspace_import_policy_version: 1,
            provider: plan.options.provider.clone(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        },
    };
    let admitted = store.admit_worker(
        &admission,
        &policy,
        decision,
        json!(null),
        &QuotaPolicyV1::default(),
    )?;
    let worker = admitted.worker.context("fixture reservation missing")?;
    plan.managed = Some(ManagedCheckpoint {
        tenant_id: "local".into(),
        gateway_origin: reqwest::Url::parse(origin)?.to_string(),
        config_root: plan.options.config_dir.clone(),
        binding: None,
        authority_file: "unused.json".into(),
        admission,
        worker: Some(worker.clone()),
        owned: None,
        pending: None,
        uncertain: vec![],
        lock_identity: None,
        before,
        import: None,
        decision: json!(null),
        output_event: None,
        output_acknowledged: false,
        result: None,
        archive_name: "unused.tar".into(),
        archive_digest: "0".repeat(64),
        credential_digest: None,
        workspace_control_exclusion_version: Some(1),
        invocation_event: None,
    });
    PlanState::new(plan.clone())?;
    Ok((plan, worker))
}

#[tokio::test]
async fn reserved_stop_replays_the_same_intent_across_gateway_restart() -> Result<()> {
    for cut in [Cut::AfterCommit, Cut::BeforeCommit] {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let fixture = root.path().canonicalize()?;
        let config = fixture.join("config");
        crate::operations::initialize_config(&config)?;
        let runner = read_token(&config.join("secrets/runner"))?;
        let viewer = read_token(&config.join("secrets/viewer"))?;
        let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
        let database = fixture.join("audit.sqlite");
        let store = Arc::new(Store::open(&database, redactor.clone(), 1000)?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address: SocketAddr = listener.local_addr()?;
        let origin = format!("http://{address}");
        let (plan, worker) = prepare_reservation(&fixture, &origin, &store)?;
        let baseline = store.integrity()?;
        assert_eq!(worker.state, WorkerStateV1::Reserved);
        assert_eq!(
            store.worker_quota(&QuotaPolicyV1::default())?.held_workers,
            1
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut gateway = serve(
            listener,
            store.clone(),
            runner.clone(),
            viewer.clone(),
            Some(cut),
            requests.clone(),
        )?;
        let error = stop_worker(plan.session, &config, &origin, None, false)
            .await
            .expect_err("the first cancellation response was interrupted");
        assert!(error.to_string().contains("503 Service Unavailable"));
        let request = requests.lock().unwrap()[0].clone();
        let committed = store.worker_operation(&request.operation_id)?;
        let legacy = store
            .worker(&worker.worker_id)?
            .context("reservation disappeared")?;
        let held = store.worker_quota(&QuotaPolicyV1::default())?.held_workers;
        let intermediate = store.integrity()?;
        match cut {
            Cut::BeforeCommit => {
                assert!(committed.is_none());
                assert_eq!(legacy.state, WorkerStateV1::Reserved);
                assert_eq!(held, 1);
                assert_eq!(intermediate.head_hash, baseline.head_hash);
            }
            Cut::AfterCommit => {
                assert!(committed.is_some());
                assert_eq!(legacy.state, WorkerStateV1::Cancelled);
                assert_eq!(held, 0);
                assert_eq!(intermediate.event_count, baseline.event_count + 1);
            }
        }
        gateway.stop().await?;
        drop(store);

        let store = Arc::new(Store::open_existing_for_tenant(
            &database, "local", redactor, 1000,
        )?);
        let listener = TcpListener::bind(address).await?;
        let _gateway = serve(
            listener,
            store.clone(),
            runner,
            viewer,
            None,
            requests.clone(),
        )?;
        let cancelled = stop_worker(plan.session, &config, &origin, None, false).await?;
        assert_eq!(cancelled["worker"]["state"], "cancelled");
        assert_eq!(cancelled["worker"]["worker_id"], worker.worker_id);
        assert_eq!(
            cancelled["worker"]["attempt"]["attempt_id"],
            worker.attempt.attempt_id
        );
        assert_eq!(cancelled["worker"]["revision"], worker.revision + 1);
        assert!(cancelled["runtime_observation"].is_null());
        assert!(cancelled["worker"]["attempt"]["container_id"].is_null());
        {
            let requests = requests.lock().unwrap();
            let received: Vec<_> = requests
                .iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<_, _>>()?;
            let expected = serde_json::to_value(&request)?;
            assert_eq!(
                received,
                vec![expected.clone(), expected],
                "{cut:?}: exact retry of the durable cancellation"
            );
        }
        let receipt = store
            .worker_operation(&request.operation_id)?
            .context("cancellation ACK missing")?;
        if let Some(committed) = committed {
            assert_eq!(receipt, committed);
        }
        let acknowledgement: MutationReceiptV1 = serde_json::from_slice(&fs::read(
            config
                .join("worker-stop-intents")
                .join(&request.attempt_id)
                .join("ack.json"),
        )?)?;
        assert_eq!(acknowledgement, receipt);
        assert_eq!(
            store.worker_quota(&QuotaPolicyV1::default())?.held_workers,
            0
        );
        assert!(store.worker_usage()?.is_empty());
        let integrity = store.integrity()?;
        assert!(integrity.valid);
        assert_eq!(integrity.event_count, baseline.event_count + 1);
        assert_eq!(integrity.head_hash, receipt.audit_hash);
        assert_eq!(
            stop_worker(plan.session, &config, &origin, None, false).await?["worker"],
            cancelled["worker"]
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(store.integrity()?.head_hash, integrity.head_hash);
    }
    Ok(())
}
