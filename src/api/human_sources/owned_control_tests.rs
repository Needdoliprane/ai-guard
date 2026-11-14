//! A0 test-first: real migrated owned sources, registry/binding and HTTP SDK.
//! NOT_RUN until Root admits this exact source. No future production API used.
use super::*;
use crate::{
    audit::Store,
    broker::service::{owned_protocol_precheck_for_test, protocol::BrokerErrorCodeV1},
    control::SupervisionProtocolV1,
    control_fs::Directory,
    runner::AuditClient,
};
use axum::{
    extract::Request,
    middleware::{self, Next},
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};

const RESERVED_PROTOCOL: &str = "/control/api/v1/control/protocol";
const RUNNER: &str = "synthetic-A-runner";

struct HttpServer {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl HttpServer {
    async fn close(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(30), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn diagnostic_and_broker_boundary(format: u32) {
    let mut fixture = Store::owned_control_fixture_for_test(format);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials.clone(),
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let owned = state.owned_human.as_ref().unwrap();
    let credential = crate::api::authenticate_review_runner_fixture(
        credentials,
        owned.owner.clone(),
        state
            .tenants
            .iter()
            .map(|tenant| (tenant.tenant_id.clone(), Arc::clone(&tenant.store)))
            .collect(),
        RUNNER,
    )
    .unwrap();
    assert_eq!(credential.tenant_id(), "A");
    let index = state
        .tenants
        .iter()
        .position(|tenant| tenant.tenant_id == "A")
        .unwrap();
    let (namespace, active) = {
        let epoch = owned.owner.try_read_epoch().unwrap();
        state.tenants[index]
            .store
            .assert_owned_control_fixture_binding_for_test(
                &epoch,
                &owned.sources[index],
                &credential,
                format,
            )
    };
    drop(credential);
    let retirement = state.human_retirement().unwrap();
    let observed = Arc::new(Mutex::new(Vec::<u16>::new()));
    let record = Arc::clone(&observed);
    let app = gateway_router(state.clone()).layer(middleware::from_fn(
        move |request: Request, next: Next| {
            let record = Arc::clone(&record);
            async move {
                let is_protocol = request.method() == axum::http::Method::GET
                    && request.uri().path() == RESERVED_PROTOCOL;
                let response = next.run(request).await;
                if is_protocol {
                    record.lock().unwrap().push(response.status().as_u16());
                }
                response
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut server = HttpServer {
        stop: Some(stop),
        task: Some(task),
    };
    let client = AuditClient::new(&url, RUNNER).unwrap();

    // Valid synthetic operator memory for C3, with every effect-bearing path
    // absent. Registry::load normalizes them but must not open them, and the
    // real VerifiedProfile precheck must refuse before open_context.
    let operator = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let directory = Directory::open_existing(operator.path()).unwrap();
    let absent = operator.path().join("unopened");
    let registry = directory.persistent_create("registry.json", &serde_json::to_vec(&json!({
        "version":1, "installation_id":owned.owner.closure().installation_id,
        "root":absent.join("jobs"), "max_enforcer_slots":1,
        "profiles":[{
            "id":"a0-boundary", "gateway":url.as_str(), "tenant_id":"A", "control_namespace_uuid":namespace.as_str(),
            "config_dir":absent.join("config"), "runner_token_file":absent.join("runner-token"),
            "runtime":"docker", "runtime_program":absent.join("runtime-program"),
            "runtime_endpoint":"unix:///synthetic-unopened-engine.sock", "image":"synthetic:a0",
            "policy_profile":"strict", "network":"none", "allow_rootful_runtime":false,
            "max_timeout_seconds":30, "allow_capture_output":false, "managed_clients":true,
            "providers_file":absent.join("providers.json"), "manual_credential_file":null,
            "source_roots":[absent.join("project")], "export_roots":[absent.join("export")]
        }]
    })).unwrap()).unwrap();
    assert!(!absent.exists());
    // Nonsensitive fixed markers appear only after real migration, W0 startup,
    // retained registry lookup and checked active binding verification succeed.
    match format {
        14 => println!("A0_FIXTURE_VALID_FORMAT14_REAL_MIGRATION_REGISTRY_BINDING"),
        15 => println!("A0_FIXTURE_VALID_FORMAT15_REAL_MIGRATION_REGISTRY_BINDING"),
        _ => unreachable!(),
    }
    let protocol = tokio::time::timeout(
        Duration::from_secs(120),
        client.control_supervision_identity(),
    )
    .await
    .expect("SDK observation budget elapsed; not a product baseline");
    // Baseline503 must fail the SDK-success oracle, never a fabricated broker
    // result. C3 is reached only after an actual successful SDK diagnostic.
    let broker = if protocol.is_ok() {
        Some(
            tokio::time::timeout(
                Duration::from_secs(120),
                owned_protocol_precheck_for_test(registry.path(), "a0-boundary", RUNNER),
            )
            .await
            .expect("broker precheck observation budget elapsed"),
        )
    } else {
        None
    };
    let effect_paths_absent = !absent.exists();
    drop(client);
    server.close().await;
    retirement.retire();
    tokio::time::timeout(Duration::from_secs(30), retirement.drain())
        .await
        .unwrap()
        .unwrap();
    let statuses = observed.lock().unwrap().clone();
    // Expected baseline oracle on120b: SDKUnavailable and real HTTP503 here.
    assert!(
        protocol.is_ok(),
        "A0_DIAGNOSTIC_EXPECTED_SUCCESS format={format} sdk_error={:?} actual_http_statuses={statuses:?}",
        protocol.as_ref().err()
    );
    let protocol = protocol.unwrap();
    assert_eq!(protocol.schema_version, format);
    assert_eq!(
        protocol.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert_eq!(protocol.tenant_id, "A");
    assert_eq!(protocol.role, "runner");
    assert_eq!(protocol.active, active);
    assert_eq!(
        protocol.protocols,
        vec![SupervisionProtocolV1::BrokerSupervisionV1]
    );
    assert_eq!(statuses, vec![200, 200]);
    assert_eq!(broker.unwrap(), Err(BrokerErrorCodeV1::IncompatibleControl));
    assert!(
        effect_paths_absent,
        "diagnostic compatibility must not admit broker effects"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_protocol_format14_reaches_real_sdk_and_keeps_broker_incompatible() {
    diagnostic_and_broker_boundary(14).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_protocol_format15_reaches_real_sdk_and_keeps_broker_incompatible() {
    diagnostic_and_broker_boundary(15).await;
}

// Negative regressions are deliberately outside the frozen positive prefix.
pub(super) mod negative;

// Separate canary for the composed A0 postcommit rendez-vous.
#[cfg(test)]
mod cancellation;
