//! Real loopback gateway and schema-9 Store coverage for the broker HTTP client.
//! No cluster, model or runtime engine is started by these tests.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
    control::{HostClockMethodV1, HostClockSampleV1, QuotaPolicyV1},
    control_fs::{Directory, PrivateFile},
    policy::Policy,
    redaction::Redactor,
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesPreflightResourcesV1,
        admission::{
            self,
            journal::{AdmissionJournal, AdmissionJournalProtection, JournalError, RecoveryPhase},
            transport_tests::TlsMaterial,
        },
    },
    tls::ServerIdentity,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{sync::oneshot, task::JoinHandle};
use uuid::Uuid;

const TENANT: &str = "held-client-test-tenant";
const TOKEN: &str = "public-held-client-token-aaaaaaaaaaaaaaaaaaaaaaaa";
const EXPIRY: &str = "2100-01-01T00:00:00.000Z";

pub(crate) struct Server {
    pub(crate) url: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Server {
    pub(crate) async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(crate) struct Fixture {
    pub(crate) directory: TempDir,
    store: Option<Arc<Store>>,
    tenant: String,
    token: String,
    credential_id: String,
    token_path: PathBuf,
    pub(crate) intent: KubernetesCreateIntentV1,
    binding_revision: u64,
    grant_revision: u64,
}

impl Fixture {
    pub(crate) fn new(tenant: &str) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap())
            .expect("private held-client fixture directory");
        let store_path = directory.path().join("tenant.sqlite");
        let store = Store::open_for_tenant(&store_path, tenant, Redactor::new(vec![]), 10_000)
            .expect("real tenant Store");
        store
            .migrate_control_v3(&directory.path().join("before3"))
            .unwrap();
        store
            .migrate_identity_v4(&directory.path().join("before4"))
            .unwrap();
        store
            .migrate_human_access_v5(&directory.path().join("before5"))
            .unwrap();
        store
            .migrate_workload_v6(&directory.path().join("before6"))
            .unwrap();
        store
            .migrate_human_brokers_v7(&directory.path().join("before7"))
            .unwrap();
        store
            .migrate_kubernetes_create_v8(&directory.path().join("before8"))
            .unwrap();
        store
            .migrate_kubernetes_policy_v9(&directory.path().join("before9"))
            .unwrap();

        let broker_id = Uuid::new_v4().to_string();
        let incarnation_id = Uuid::new_v4().to_string();
        let credential_id = Uuid::new_v4().to_string();
        let token = format!("{TOKEN}-{}", tenant.replace('-', ""));
        let authority = KubernetesAuthorityV1::new(
            Uuid::new_v4(),
            "https://127.0.0.1:6443",
            "11111111-1111-4111-8111-111111111111",
            tenant,
            "22222222-2222-4222-8222-222222222222",
            &"a".repeat(64),
        )
        .unwrap();
        let binding = store
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: credential_id.clone(),
                expected_revision: 0,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: broker_id.clone(),
                    incarnation_id: incarnation_id.clone(),
                },
                sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                expires_at: EXPIRY.into(),
            })
            .unwrap();
        let grant = store
            .mutate_broker_grant(&BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: broker_id.clone(),
                expected_revision: None,
                credential_id: credential_id.clone(),
                incarnation_id: incarnation_id.clone(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: authority.authority_id().to_string(),
                    runtime_kind: BrokerRuntimeKindV1::Kubernetes,
                }],
                lease_seconds: 120,
                not_after: EXPIRY.into(),
            })
            .unwrap();
        let namespace = store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap();
        let intent = KubernetesCreateIntentV1 {
            version: 1,
            create_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4().to_string(),
            control_namespace_uuid: namespace,
            broker_id,
            incarnation_id,
            authority,
            profile: KUBERNETES_HELD_PROFILE_V1.into(),
            image: format!("worker@sha256:{}", "b".repeat(64)),
            resources: KubernetesPreflightResourcesV1 {
                cpu_millis: 500,
                memory_mib: 128,
                workspace_mib: 8,
                temporary_mib: 8,
                deadline_seconds: 60,
            },
        };
        intent.validate().unwrap();

        let token_path = directory.path().join("broker-token");
        write_private(&token_path, token.as_bytes());
        Self {
            directory,
            store: Some(Arc::new(store)),
            tenant: tenant.into(),
            token,
            credential_id,
            token_path,
            intent,
            binding_revision: binding.revision,
            grant_revision: grant.receipt.grant_revision,
        }
    }

    pub(crate) fn store(&self) -> &Arc<Store> {
        self.store.as_ref().expect("fixture Store is open")
    }

    fn policy(&self) -> KubernetesHeldCreatePolicyV1 {
        KubernetesHeldCreatePolicyV1 {
            version: 1,
            decision_ttl_seconds: 30,
            approvals: vec![KubernetesHeldCreateApprovalV1 {
                authority_fingerprint: self.intent.authority.fingerprint(),
                image: self.intent.image.clone(),
                profile: self.intent.profile.clone(),
            }],
        }
    }

    fn scope(&self, url: &str) -> HeldCreateClientScope {
        HeldCreateClientScope {
            gateway: url.into(),
            tenant_id: self.tenant.clone(),
            control_namespace_uuid: self.intent.control_namespace_uuid.clone(),
            broker_id: self.intent.broker_id.clone(),
            credential_id: self.credential_id(),
            incarnation_id: self.intent.incarnation_id.clone(),
        }
    }

    fn credential_id(&self) -> String {
        self.credential_id.clone()
    }

    fn tenant_gateway(&self) -> TenantGateway {
        TenantGateway {
            tenant_id: self.tenant.clone(),
            store: Arc::clone(self.store()),
            runner_token: format!("public-control-runner-{}-{}", self.tenant, "r".repeat(40)),
            viewer_token: format!("public-held-viewer-{}", self.tenant),
        }
    }

    pub(crate) fn gateway(&self, policy_enabled: bool, quota: QuotaPolicyV1) -> GatewayState {
        let tenant_gateway = self.tenant_gateway();
        let mut state = GatewayState::for_tenants(
            vec![tenant_gateway],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
        .with_worker_quotas(vec![(self.tenant.clone(), quota)])
        .unwrap();
        if policy_enabled {
            state = state
                .with_kubernetes_held_create_policies(vec![(self.tenant.clone(), self.policy())])
                .unwrap();
        }
        state
    }

    pub(crate) fn gateway_with_redaction_secret(
        &self,
        policy_enabled: bool,
        quota: QuotaPolicyV1,
        secret: &str,
    ) -> GatewayState {
        let mut state = GatewayState::for_tenants(
            vec![self.tenant_gateway()],
            Policy::default(),
            None,
            Redactor::new(vec![secret.to_owned()]),
        )
        .unwrap()
        .with_worker_quotas(vec![(self.tenant.clone(), quota)])
        .unwrap();
        if policy_enabled {
            state = state
                .with_kubernetes_held_create_policies(vec![(self.tenant.clone(), self.policy())])
                .unwrap();
        }
        state
    }

    pub(crate) async fn serve(&self, policy_enabled: bool, quota: QuotaPolicyV1) -> Server {
        self.serve_with_fault(policy_enabled, quota, None).await
    }

    pub(crate) async fn serve_with_redaction_secret(
        &self,
        secret: &str,
    ) -> (Server, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let result_requests = Arc::new(AtomicUsize::new(0));
        let router = gateway_router(self.gateway_with_redaction_secret(
            true,
            QuotaPolicyV1::default(),
            secret,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&result_requests),
            count_result_request,
        ));
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        (
            Server {
                url,
                stop: Some(stop),
                task: Some(task),
            },
            result_requests,
        )
    }

    pub(crate) async fn serve_with_lost_result_ack_once(
        &self,
    ) -> (Server, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let result_requests = Arc::new(AtomicUsize::new(0));
        let hidden_committed_ack = Arc::new(AtomicBool::new(false));
        let router = gateway_router(self.gateway(true, QuotaPolicyV1::default())).layer(
            middleware::from_fn_with_state(
                LostResultAckState {
                    result_requests: Arc::clone(&result_requests),
                    hidden_committed_ack: Arc::clone(&hidden_committed_ack),
                },
                hide_first_committed_result_ack,
            ),
        );
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        (
            Server {
                url,
                stop: Some(stop),
                task: Some(task),
            },
            result_requests,
            hidden_committed_ack,
        )
    }

    async fn serve_tls(&self, material: &TlsMaterial, requests: Arc<AtomicUsize>) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        let identity = ServerIdentity::from_files(&material.certificate, &material.key).unwrap();
        let router = gateway_router(self.gateway(true, QuotaPolicyV1::default())).layer(
            middleware::from_fn_with_state(Arc::clone(&requests), count_gateway_request),
        );
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            crate::tls::serve(listener, router, identity, async move {
                let _ = stopped.await;
            })
            .await
            .expect("serve the real tenant gateway over the owned TLS listener");
        });
        Server {
            url,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn serve_with_result_mutation(
        &self,
        mode: Arc<Mutex<Option<ResultSnapshotMutation>>>,
    ) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = gateway_router(self.gateway(true, QuotaPolicyV1::default())).layer(
            middleware::from_fn_with_state(ResultMutationState { mode }, mutate_result_snapshot),
        );
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        Server {
            url,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn serve_counted(&self) -> (Server, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let router = gateway_router(self.gateway(true, QuotaPolicyV1::default())).layer(
            middleware::from_fn_with_state(Arc::clone(&requests), count_gateway_request),
        );
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        (
            Server {
                url,
                stop: Some(stop),
                task: Some(task),
            },
            requests,
        )
    }

    async fn serve_with_fault(
        &self,
        policy_enabled: bool,
        quota: QuotaPolicyV1,
        fault: Option<(ResponseFault, Arc<AtomicUsize>)>,
    ) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mut router = gateway_router(self.gateway(policy_enabled, quota));
        if let Some((fault, followed_redirects)) = fault {
            router = router.layer(middleware::from_fn_with_state(
                FaultState {
                    fault,
                    followed_redirects,
                    secret: self.token.clone(),
                },
                fault_response,
            ));
        }
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        Server {
            url,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn serve_with_claim_guard(
        &self,
        mode: ClaimGuardMode,
        observed_201: Arc<AtomicUsize>,
        renewed_revision: Arc<AtomicU64>,
        quota: QuotaPolicyV1,
    ) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router =
            gateway_router(self.gateway(true, quota)).layer(middleware::from_fn_with_state(
                ClaimGuardState {
                    mode,
                    store: Arc::clone(self.store()),
                    credential_id: self.credential_id(),
                    broker_id: self.intent.broker_id.clone(),
                    incarnation_id: self.intent.incarnation_id.clone(),
                    token: self.token.clone(),
                    binding_revision: self.binding_revision,
                    observed_201,
                    renewed_revision,
                },
                guard_after_claim,
            ));
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        Server {
            url,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn serve_with_peer(&self, peer: &Fixture, quota: QuotaPolicyV1) -> Server {
        let state = GatewayState::for_tenants(
            vec![self.tenant_gateway(), peer.tenant_gateway()],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
        .with_worker_quotas(vec![
            (self.tenant.clone(), quota.clone()),
            (peer.tenant.clone(), quota),
        ])
        .unwrap()
        .with_kubernetes_held_create_policies(vec![
            (self.tenant.clone(), self.policy()),
            (peer.tenant.clone(), peer.policy()),
        ])
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = gateway_router(state);
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        Server {
            url,
            stop: Some(stop),
            task: Some(task),
        }
    }

    pub(crate) fn client(&self, url: &str) -> HeldCreateControlClient {
        self.client_with_known_secrets(url, Vec::new())
    }

    fn client_with_ca(&self, url: &str, ca_path: &Path) -> HeldCreateControlClient {
        let token = PrivateFile::open(&self.token_path, 4096).unwrap();
        let ca = PrivateFile::open(ca_path, 65_536).unwrap();
        HeldCreateControlClient::new(self.scope(url), token, Some(ca), vec![self.token.clone()])
            .expect("construct client with explicitly retained private CA and bearer files")
    }

    fn client_with_known_secrets(
        &self,
        url: &str,
        additional: Vec<String>,
    ) -> HeldCreateControlClient {
        let token = PrivateFile::open(&self.token_path, 4096).unwrap();
        let mut known_secrets = vec![self.token.clone()];
        known_secrets.extend(additional);
        HeldCreateControlClient::new(self.scope(url), token, None, known_secrets)
            .expect("construct client from protected token and explicit loopback gateway")
    }

    fn wrong_token_client(&self, url: &str) -> HeldCreateControlClient {
        let path = self.directory.path().join("wrong-token");
        let wrong = format!("wrong-held-client-token-{}", "x".repeat(40));
        write_private(&path, wrong.as_bytes());
        let token = PrivateFile::open(&path, 4096).unwrap();
        HeldCreateControlClient::new(self.scope(url), token, None, vec![wrong])
            .expect("construct test client with a protected wrong token")
    }

    pub(crate) fn prepared_journal(
        &self,
        reservation: &KubernetesCreateMutationResponseV1,
        claim: &KubernetesCreateClaimV1,
    ) -> AdmissionJournal {
        let root = Directory::open_existing(self.directory.path())
            .unwrap()
            .persistent_child("held-create-journal", true)
            .unwrap()
            .unwrap();
        AdmissionJournal::prepare(
            root,
            self.journal_protection(),
            &self.tenant,
            &self.intent,
            admission::prepare(&self.intent).unwrap(),
            &reservation.receipt,
            &reservation.snapshot,
            claim,
        )
        .unwrap()
    }

    pub(crate) fn journal_protection(&self) -> AdmissionJournalProtection {
        AdmissionJournalProtection::new(Vec::new(), vec![self.token.clone()])
    }

    pub(crate) fn claim(&self) -> KubernetesCreateClaimV1 {
        KubernetesCreateClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            create_id: self.intent.create_id.clone(),
            intent_hash: self.intent.intent_hash().unwrap(),
            expected_revision: 1,
            expected_create_epoch: 1,
            emitter_id: Uuid::new_v4().to_string(),
        }
    }

    pub(crate) fn recreate_store_inactive(&mut self) {
        let previous = self.store.take().unwrap();
        let store = Arc::try_unwrap(previous)
            .unwrap_or_else(|_| panic!("all old gateway Store references were closed"));
        drop(store);
        let reopened = Store::open_for_tenant(
            &self.directory.path().join("tenant.sqlite"),
            &self.tenant,
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap();
        assert!(!reopened.supervision_state().unwrap().active);
        self.store = Some(Arc::new(reopened));
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn client_from_config_secrets(
    root: &TempDir,
    known_secrets: Vec<String>,
) -> ClientResult<HeldCreateControlClient> {
    let token_path = root.path().join("config-token");
    if !token_path.exists() {
        write_private(&token_path, TOKEN.as_bytes());
    }
    let token = PrivateFile::open(&token_path, 4096)
        .map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
    HeldCreateControlClient::new(
        HeldCreateClientScope {
            gateway: "http://127.0.0.1:9".into(),
            tenant_id: "known-secret-config-tenant".into(),
            control_namespace_uuid: "11111111-1111-4111-8111-111111111111".into(),
            broker_id: "22222222-2222-4222-8222-222222222222".into(),
            credential_id: "33333333-3333-4333-8333-333333333333".into(),
            incarnation_id: "44444444-4444-4444-8444-444444444444".into(),
        },
        token,
        None,
        known_secrets,
    )
}

async fn count_gateway_request(
    axum::extract::State(requests): axum::extract::State<Arc<AtomicUsize>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    requests.fetch_add(1, Ordering::SeqCst);
    next.run(request).await
}

async fn count_result_request(
    axum::extract::State(requests): axum::extract::State<Arc<AtomicUsize>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() == axum::http::Method::POST && request.uri().path().ends_with("/results") {
        requests.fetch_add(1, Ordering::SeqCst);
    }
    next.run(request).await
}

#[derive(Clone)]
struct LostResultAckState {
    result_requests: Arc<AtomicUsize>,
    hidden_committed_ack: Arc<AtomicBool>,
}

async fn hide_first_committed_result_ack(
    axum::extract::State(state): axum::extract::State<LostResultAckState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let is_result =
        request.method() == axum::http::Method::POST && request.uri().path().ends_with("/results");
    let result_index = is_result.then(|| state.result_requests.fetch_add(1, Ordering::SeqCst));
    let response = next.run(request).await;
    if result_index == Some(0) && response.status() == StatusCode::OK {
        state.hidden_committed_ack.store(true, Ordering::SeqCst);
        let (mut parts, committed_ack_body) = response.into_parts();
        let committed_ack_bytes = to_bytes(committed_ack_body, 128 * 1024)
            .await
            .expect("the committed Result ACK body is valid before the read fault");
        assert!(!committed_ack_bytes.is_empty());
        parts.headers.insert(
            header::CONTENT_LENGTH,
            axum::http::HeaderValue::from_str(&committed_ack_bytes.len().to_string())
                .expect("the finite ACK length is a valid HTTP header"),
        );
        let truncated = futures_util::stream::once(async {
            Err::<axum::body::Bytes, std::io::Error>(std::io::Error::other(
                "synthetic Result ACK body read failure",
            ))
        });
        return Response::from_parts(parts, Body::from_stream(truncated));
    }
    response
}

#[derive(Clone, Copy)]
enum ResultSnapshotMutation {
    ClaimEmitter,
    EmissionWindow,
}

#[derive(Clone)]
struct ResultMutationState {
    mode: Arc<Mutex<Option<ResultSnapshotMutation>>>,
}

async fn mutate_result_snapshot(
    axum::extract::State(state): axum::extract::State<ResultMutationState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let is_result =
        request.method() == axum::http::Method::POST && request.uri().path().ends_with("/results");
    let response = next.run(request).await;
    if !is_result {
        return response;
    }
    assert_eq!(response.status(), StatusCode::OK);
    let Some(mode) = *state.mode.lock().unwrap() else {
        return response;
    };
    let (mut parts, body) = response.into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    let bytes = to_bytes(body, 256 * 1024).await.unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    match mode {
        ResultSnapshotMutation::ClaimEmitter => {
            let old = value["snapshot"]["claim"]["emitter_id"]
                .as_str()
                .expect("result snapshot carries the exact historical claim emitter")
                .to_owned();
            let mut replacement = Uuid::new_v4().to_string();
            while replacement == old {
                replacement = Uuid::new_v4().to_string();
            }
            value["snapshot"]["claim"]["emitter_id"] = serde_json::json!(replacement);
        }
        ResultSnapshotMutation::EmissionWindow => {
            let window = value["snapshot"]["emission_window"]
                .as_object_mut()
                .expect("result snapshot retains the original claim emission window");
            let old_issued = chrono::DateTime::parse_from_rfc3339(
                window["issued_at"].as_str().expect("issued_at is text"),
            )
            .expect("issued_at is RFC3339");
            let old_not_after = chrono::DateTime::parse_from_rfc3339(
                window["not_after"].as_str().expect("not_after is text"),
            )
            .expect("not_after is RFC3339");
            let old_duration = old_not_after - old_issued;
            assert_eq!(old_duration, chrono::Duration::seconds(8));
            for field in ["issued_at", "not_after"] {
                let timestamp = window[field]
                    .as_str()
                    .expect("emission window fields are strings");
                let parsed = chrono::DateTime::parse_from_rfc3339(timestamp)
                    .expect("emission window timestamp is RFC3339");
                window.insert(
                    field.to_owned(),
                    serde_json::json!(
                        (parsed + chrono::Duration::milliseconds(1))
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    ),
                );
            }
            let new_issued = chrono::DateTime::parse_from_rfc3339(
                window["issued_at"]
                    .as_str()
                    .expect("shifted issued_at is text"),
            )
            .expect("shifted issued_at remains RFC3339");
            let new_not_after = chrono::DateTime::parse_from_rfc3339(
                window["not_after"]
                    .as_str()
                    .expect("shifted not_after is text"),
            )
            .expect("shifted not_after remains RFC3339");
            assert_eq!(new_not_after - new_issued, old_duration);
        }
    }
    Response::from_parts(parts, Body::from(serde_json::to_vec(&value).unwrap()))
}

fn quota_zero() -> QuotaPolicyV1 {
    QuotaPolicyV1 {
        max_active_workers: 0,
        max_cpu_millis: 0,
        max_memory_bytes: 0,
        ..QuotaPolicyV1::default()
    }
}

#[cfg(target_os = "macos")]
const TEST_CLOCK_METHOD: HostClockMethodV1 = HostClockMethodV1::DarwinMachContinuousV1;
#[cfg(not(target_os = "macos"))]
const TEST_CLOCK_METHOD: HostClockMethodV1 = HostClockMethodV1::LinuxBoottimeV1;

struct TestClock {
    next_ns: AtomicU64,
    samples: AtomicUsize,
    step_ns: u64,
}
impl TestClock {
    fn new() -> Self {
        Self {
            next_ns: AtomicU64::new(1_000_000_000),
            samples: AtomicUsize::new(0),
            step_ns: 1_000_000,
        }
    }

    fn sample_count(&self) -> usize {
        self.samples.load(Ordering::SeqCst)
    }

    fn advance_nine_seconds(&self) {
        self.next_ns.fetch_add(9_000_000_000, Ordering::SeqCst);
    }
}
impl HostClock for TestClock {
    fn sample(&self) -> crate::broker::clock::ClockResult<HostClockSampleV1> {
        self.samples.fetch_add(1, Ordering::SeqCst);
        Ok(HostClockSampleV1 {
            version: 1,
            method: TEST_CLOCK_METHOD,
            boot_id: "33333333-3333-4333-8333-333333333333".into(),
            continuous_ns_decimal: self
                .next_ns
                .fetch_add(self.step_ns, Ordering::SeqCst)
                .to_string(),
            observed_utc: None,
        })
    }
}

#[derive(Clone)]
enum ClaimGuardMode {
    RenewSameBinding,
    AdvanceClock(Arc<TestClock>),
}

#[derive(Clone)]
struct ClaimGuardState {
    mode: ClaimGuardMode,
    store: Arc<Store>,
    credential_id: String,
    broker_id: String,
    incarnation_id: String,
    token: String,
    binding_revision: u64,
    observed_201: Arc<AtomicUsize>,
    renewed_revision: Arc<AtomicU64>,
}

async fn guard_after_claim(
    axum::extract::State(state): axum::extract::State<ClaimGuardState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let is_claim_post =
        request.method() == axum::http::Method::POST && request.uri().path().ends_with("/claims");
    let response = next.run(request).await;
    if !is_claim_post || response.status() != StatusCode::CREATED {
        return response;
    }

    state.observed_201.fetch_add(1, Ordering::SeqCst);
    match &state.mode {
        ClaimGuardMode::RenewSameBinding => {
            let renewal = state
                .store
                .issue_workload_binding(&WorkloadBindingIssueV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    credential_id: state.credential_id.clone(),
                    expected_revision: state.binding_revision,
                    subject: WorkloadSubjectV1::Broker {
                        broker_id: state.broker_id.clone(),
                        incarnation_id: state.incarnation_id.clone(),
                    },
                    sha256_digest: hex::encode(Sha256::digest(state.token.as_bytes())),
                    expires_at: EXPIRY.into(),
                })
                .expect("the same binding can be renewed at its current revision");
            state
                .renewed_revision
                .store(renewal.revision, Ordering::SeqCst);
        }
        ClaimGuardMode::AdvanceClock(clock) => clock.advance_nine_seconds(),
    }
    response
}

#[tokio::test]
async fn real_gateway_protocol_and_reservation_are_exact_and_tenant_scoped() {
    let a = Fixture::new("tenant-a");
    let b = Fixture::new("tenant-b");
    let server = a.serve_with_peer(&b, QuotaPolicyV1::default()).await;
    let client = a.client(&server.url);

    let protocol = client.protocol().await.unwrap();
    assert_eq!(protocol.version, 1);
    assert_eq!(protocol.schema_version, 9);
    assert_eq!(protocol.tenant_id, a.tenant);
    assert_eq!(
        protocol.control_namespace_uuid,
        a.intent.control_namespace_uuid
    );
    assert_eq!(protocol.broker_id, a.intent.broker_id);
    assert_eq!(protocol.incarnation_id, a.intent.incarnation_id);
    assert_eq!(protocol.binding_revision, a.binding_revision);
    assert_eq!(protocol.grant_revision, a.grant_revision);
    assert!(protocol.admission_policy_configured);
    assert_eq!(
        protocol.operations,
        vec![
            KubernetesHeldCreateOperationV1::Reserve,
            KubernetesHeldCreateOperationV1::Claim,
            KubernetesHeldCreateOperationV1::ObserveClaim,
            KubernetesHeldCreateOperationV1::Result,
            KubernetesHeldCreateOperationV1::Cancel,
        ]
    );

    let identity_protocol_head = a.store().head().unwrap();
    let wrong_token = a.wrong_token_client(&server.url);
    let auth_error = wrong_token.protocol().await.err().unwrap();
    assert_eq!(auth_error, HeldCreateClientError::Authentication);
    assert!(!auth_error.to_string().contains(&a.token));
    assert_eq!(a.store().head().unwrap(), identity_protocol_head);

    let reservation = client.reserve(&a.intent).await.unwrap();
    assert!(!reservation.replayed);
    assert_eq!(reservation.receipt.operation_id, a.intent.create_id);
    assert_eq!(
        reservation.receipt.input_hash,
        a.intent.intent_hash().unwrap()
    );
    let head = a.store().head().unwrap();
    let replay = client.reserve(&a.intent).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, reservation.receipt);
    assert_eq!(replay.snapshot, reservation.snapshot);
    assert_eq!(a.store().head().unwrap(), head);

    let cancel = KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: a.intent.create_id.clone(),
        intent_hash: a.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    };
    let cancelled = client.cancel(&cancel).await.unwrap();
    assert!(!cancelled.replayed);
    assert_eq!(
        cancelled.snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert!(!cancelled.snapshot.quota_held);
    let head_after_cancel = a.store().head().unwrap();
    let cancel_replay = client.cancel(&cancel).await.unwrap();
    assert!(cancel_replay.replayed);
    assert_eq!(cancel_replay.receipt, cancelled.receipt);
    assert_eq!(cancel_replay.snapshot, cancelled.snapshot);
    assert_eq!(a.store().head().unwrap(), head_after_cancel);

    let b_head = b.store().head().unwrap();
    let b_client = b.client(&server.url);
    b_client.protocol().await.unwrap();
    let scope_error = b_client.reserve(&a.intent).await.err().unwrap();
    assert!(matches!(
        scope_error,
        HeldCreateClientError::InvalidContext | HeldCreateClientError::Forbidden
    ));
    assert_eq!(a.store().head().unwrap(), head_after_cancel);
    assert_eq!(b.store().head().unwrap(), b_head);
}

#[tokio::test]
async fn only_a_new_201_claim_with_unchanged_identity_yields_one_live_ack() {
    let f = Fixture::new(TENANT);
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let client = f.client(&server.url);
    let protocol = client.protocol().await.unwrap();
    let reservation = client.reserve(&f.intent).await.unwrap();
    assert!(!reservation.replayed);
    let claim = f.claim();
    let mut journal = f.prepared_journal(&reservation, &claim);
    let owned_clock = Arc::new(TestClock::new());
    let clock: Arc<dyn HostClock> = owned_clock.clone();
    let attempt = HeldCreateClaimAttempt::begin_for_test(&journal, Arc::clone(&clock)).unwrap();
    assert_eq!(
        owned_clock.sample_count(),
        1,
        "the origin is sampled before HTTP"
    );
    let replay_attempt =
        HeldCreateClaimAttempt::begin_for_test(&journal, Arc::clone(&clock)).unwrap();
    assert_eq!(
        owned_clock.sample_count(),
        2,
        "each owned attempt gets one origin"
    );

    let first = client.claim_once(attempt).await.unwrap();
    assert_eq!(
        first.response().disposition,
        KubernetesCreateDispositionV1::EmitOnce
    );
    let first_response = first.response().clone();
    let live = first.into_live_ack().expect("only the first 201 is live");
    assert_eq!(live.tenant_id(), TENANT);
    assert_eq!(live.claim(), &claim);
    assert_eq!(live.receipt(), &first_response.receipt);
    assert_eq!(live.snapshot(), &first_response.snapshot);
    assert!(Arc::ptr_eq(&live.clock(), &clock));
    assert_eq!(live.before_claim().method, TEST_CLOCK_METHOD);
    assert_eq!(live.before_claim().continuous_ns_decimal, "1000000000");
    live.revalidate_material().unwrap();
    let checkpoint = journal
        .commit_remote_emission_possible(&live)
        .expect("the exact live 201 can be committed to the private journal");
    journal.revalidate_emission(&checkpoint).unwrap();
    assert!(
        f.directory
            .path()
            .join("held-create-journal/emission.json")
            .exists()
    );

    // A previously acknowledged 201 followed by a same-operation historical
    // replay is descriptive only; this does not simulate loss of the first ACK.
    let head = f.store().head().unwrap();
    let protocol_after_claim = client.protocol().await.unwrap();
    assert_eq!(
        protocol_after_claim.binding_revision,
        protocol.binding_revision
    );
    assert_eq!(protocol_after_claim.grant_revision, protocol.grant_revision);
    let replay = client.claim_once(replay_attempt).await.unwrap();
    assert_eq!(
        replay.response().disposition,
        KubernetesCreateDispositionV1::ObserveOnly
    );
    assert!(replay.into_live_ack().is_none());
    let observed = client
        .observe_claim(&f.intent.create_id, &claim.operation_id)
        .await
        .unwrap();
    assert_eq!(
        observed.disposition,
        KubernetesCreateDispositionV1::ObserveOnly
    );
    assert_eq!(observed.receipt, live.receipt().clone());
    assert_eq!(observed.snapshot, live.snapshot().clone());
    assert_eq!(f.store().head().unwrap(), head);
}

#[tokio::test]
async fn post_claim_binding_renewal_and_clock_expiry_never_mint_a_live_ack() {
    for mode_kind in [0, 1] {
        let f = Fixture::new(TENANT);
        let observed_201 = Arc::new(AtomicUsize::new(0));
        let renewed_revision = Arc::new(AtomicU64::new(0));
        let clock = Arc::new(TestClock::new());
        let mode = if mode_kind == 0 {
            ClaimGuardMode::RenewSameBinding
        } else {
            ClaimGuardMode::AdvanceClock(Arc::clone(&clock))
        };
        let server = f
            .serve_with_claim_guard(
                mode,
                Arc::clone(&observed_201),
                Arc::clone(&renewed_revision),
                QuotaPolicyV1::default(),
            )
            .await;
        let client = f.client(&server.url);
        client.protocol().await.unwrap();
        let reservation = client.reserve(&f.intent).await.unwrap();
        let claim = f.claim();
        let journal = f.prepared_journal(&reservation, &claim);
        let clock_trait: Arc<dyn HostClock> = clock.clone();
        let attempt = HeldCreateClaimAttempt::begin_for_test(&journal, clock_trait).unwrap();

        let exchange = client.claim_once(attempt).await.unwrap();
        assert_eq!(
            exchange.response().disposition,
            KubernetesCreateDispositionV1::EmitOnce,
            "the gateway really committed a new claim before post-response checks"
        );
        assert_eq!(observed_201.load(Ordering::SeqCst), 1);
        assert!(exchange.into_live_ack().is_none());

        if mode_kind == 0 {
            assert_eq!(
                renewed_revision.load(Ordering::SeqCst),
                f.binding_revision + 1
            );
            let digest: [u8; 32] = Sha256::digest(f.token.as_bytes()).into();
            let owner = f.store().workload_digest_binding(&digest).unwrap().unwrap();
            assert_eq!(owner.credential_id, f.credential_id());
            assert_eq!(
                owner.subject,
                WorkloadSubjectV1::Broker {
                    broker_id: f.intent.broker_id.clone(),
                    incarnation_id: f.intent.incarnation_id.clone(),
                }
            );
        } else {
            assert!(clock.sample_count() >= 2);
        }
        assert!(
            !f.directory
                .path()
                .join("held-create-journal/emission.json")
                .exists(),
            "a descriptive response after a failed guard cannot be committed as live"
        );
        server.stop().await;
    }
}

#[tokio::test]
async fn exact_result_replay_survives_policy_withdrawal_quota_zero_and_inactive_reopen() {
    let mut f = Fixture::new(TENANT);
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let client = f.client(&server.url);
    let reservation = client.reserve(&f.intent).await.unwrap();
    let claim = f.claim();
    let mut journal = f.prepared_journal(&reservation, &claim);
    let clock: Arc<dyn HostClock> = Arc::new(TestClock::new());
    let exchange = client
        .claim_once(HeldCreateClaimAttempt::begin_for_test(&journal, clock).unwrap())
        .await
        .unwrap();
    assert_eq!(
        exchange.response().disposition,
        KubernetesCreateDispositionV1::EmitOnce
    );
    let live = exchange.into_live_ack().unwrap();
    assert_eq!(live.claim(), &claim);
    journal
        .commit_remote_emission_possible(&live)
        .expect("the exact live claim ACK is retained before a result");

    let result = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        claim_operation_id: claim.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        },
    };
    journal.retain_candidate(&result, None).unwrap();
    let first = client.result(&result).await.unwrap();
    assert_eq!(first.tenant_id(), TENANT);
    assert_eq!(first.result(), &result);
    assert_eq!(first.snapshot().revision, 3);
    assert_eq!(
        first.snapshot().state,
        KubernetesCreateStateV1::OwnershipUnknown
    );
    assert!(first.snapshot().quota_held);
    let receipt = first.receipt().clone();
    let snapshot = first.snapshot().clone();
    let head_after_commit = f.store().head().unwrap();
    let ack_handle = journal.acknowledge_remote_result(first).unwrap();
    assert!(
        ack_handle.is_none(),
        "an unknown result never publishes a Pod handle"
    );
    let ack_bytes = fs::read(f.directory.path().join("held-create-journal/ack.json")).unwrap();

    // Close the serving gateway and every SQLite handle before reopening; no
    // stale in-process Store instance can satisfy the replay.
    drop(live);
    drop(client);
    drop(journal);
    server.stop().await;
    let journal_root = Directory::open_existing(f.directory.path())
        .unwrap()
        .persistent_child("held-create-journal", false)
        .unwrap()
        .unwrap();
    let recovered = AdmissionJournal::recover(
        journal_root,
        AdmissionJournalProtection::new(Vec::new(), vec![f.token.clone()]),
    )
    .unwrap()
    .expect("the exact result ACK remains in the private journal");
    assert_eq!(recovered.phase(), RecoveryPhase::ResultAcknowledged);
    let recovered_journal = recovered.into_journal();
    assert_eq!(recovered_journal.candidate(), Some(&result));
    assert_eq!(
        fs::read(f.directory.path().join("held-create-journal/ack.json")).unwrap(),
        ack_bytes
    );
    f.recreate_store_inactive();
    let reopened = f.store();
    assert_eq!(reopened.head().unwrap(), head_after_commit);
    assert!(!reopened.supervision_state().unwrap().active);

    let inactive = f.serve(false, quota_zero()).await;
    let replay_client = f.client(&inactive.url);
    let replay = replay_client.result(&result).await.unwrap();
    assert_eq!(replay.result(), &result);
    assert_eq!(replay.receipt(), &receipt);
    assert_eq!(replay.snapshot(), &snapshot);
    assert_eq!(reopened.head().unwrap(), head_after_commit);
    assert_eq!(
        fs::read(f.directory.path().join("held-create-journal/ack.json")).unwrap(),
        ack_bytes
    );
    drop(recovered_journal);
}

#[tokio::test]
async fn remote_result_ack_requires_the_exact_retained_claim_and_emission_window() {
    let mut every_mutation_was_rejected = true;
    let mut every_candidate_remained_exact = true;
    let mut every_historical_replay_was_acknowledged = true;

    for mutation in [
        ResultSnapshotMutation::ClaimEmitter,
        ResultSnapshotMutation::EmissionWindow,
    ] {
        let f = Fixture::new(TENANT);
        let mutation_mode = Arc::new(Mutex::new(Some(mutation)));
        let server = f
            .serve_with_result_mutation(Arc::clone(&mutation_mode))
            .await;
        let client = f.client(&server.url);
        let reservation = client.reserve(&f.intent).await.unwrap();
        let claim = f.claim();
        let mut journal = f.prepared_journal(&reservation, &claim);
        let clock: Arc<dyn HostClock> = Arc::new(TestClock::new());
        let exchange = client
            .claim_once(HeldCreateClaimAttempt::begin_for_test(&journal, clock).unwrap())
            .await
            .unwrap();
        assert_eq!(
            exchange.response().disposition,
            KubernetesCreateDispositionV1::EmitOnce
        );
        let live = exchange.into_live_ack().unwrap();
        journal.commit_remote_emission_possible(&live).unwrap();

        let result = KubernetesCreateResultV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            create_id: f.intent.create_id.clone(),
            intent_hash: f.intent.intent_hash().unwrap(),
            claim_operation_id: claim.operation_id.clone(),
            expected_revision: 2,
            expected_create_epoch: 1,
            outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
                reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
                http_status: None,
            },
        };
        journal.retain_candidate(&result, None).unwrap();
        let candidate_path = f
            .directory
            .path()
            .join("held-create-journal/candidate.json");
        let candidate_bytes = fs::read(&candidate_path).unwrap();
        let mutated_result_is_rejected = match client.result(&result).await {
            Err(HeldCreateClientError::Protocol) => true,
            Err(_) => false,
            Ok(ack) => journal
                .acknowledge_remote_result(ack)
                .err()
                .is_some_and(|error| error == JournalError::InvalidInput),
        };
        let mutation_preserved_candidate = fs::read(&candidate_path).unwrap() == candidate_bytes;
        let mutation_left_no_ack = !f
            .directory
            .path()
            .join("held-create-journal/ack.json")
            .exists();

        *mutation_mode.lock().unwrap() = None;
        let committed_head = f.store().head().unwrap();
        let historical_ack = client.result(&result).await;
        let replay_acknowledged = match historical_ack {
            Ok(ack) => journal
                .acknowledge_remote_result(ack)
                .is_ok_and(|handle| handle.is_none()),
            Err(_) => false,
        };
        let ack_exists = f
            .directory
            .path()
            .join("held-create-journal/ack.json")
            .is_file();
        let candidate_still_exact = fs::read(&candidate_path).unwrap() == candidate_bytes;
        let store_head_stable = f.store().head().unwrap() == committed_head;

        every_mutation_was_rejected &=
            mutated_result_is_rejected && mutation_left_no_ack && mutation_preserved_candidate;
        every_candidate_remained_exact &= candidate_still_exact;
        every_historical_replay_was_acknowledged &=
            replay_acknowledged && ack_exists && store_head_stable;
        drop(live);
        server.stop().await;
    }

    assert!(
        every_mutation_was_rejected,
        "changed historical claim/window must be rejected before ack.json"
    );
    assert!(every_candidate_remained_exact);
    assert!(
        every_historical_replay_was_acknowledged,
        "the exact unmodified historical result remains acknowledgeable"
    );
}

#[derive(Clone, Copy)]
enum ResponseFault {
    Redirect,
    DuplicateKey,
    SecretValue,
    Oversized,
}
#[derive(Clone)]
struct FaultState {
    fault: ResponseFault,
    followed_redirects: Arc<AtomicUsize>,
    secret: String,
}
async fn fault_response(
    axum::extract::State(state): axum::extract::State<FaultState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.uri().path().ends_with("/redirect-target") {
        state.followed_redirects.fetch_add(1, Ordering::SeqCst);
    }
    if request.uri().path() != "/api/v1/brokers/kubernetes/held-create-protocol" {
        return next.run(request).await;
    }
    match state.fault {
        ResponseFault::Redirect => Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(
                header::LOCATION,
                "/api/v1/brokers/kubernetes/redirect-target",
            )
            .body(Body::empty())
            .unwrap(),
        fault => {
            let original = next.run(request).await;
            assert_eq!(
                original.status(),
                StatusCode::OK,
                "the normal protocol response must be valid before fault injection"
            );
            let (mut parts, body) = original.into_parts();
            parts.headers.remove(header::CONTENT_LENGTH);
            let bytes = to_bytes(body, 256 * 1024).await.unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let body = match fault {
                ResponseFault::DuplicateKey => {
                    let mut bytes = serde_json::to_vec(&value).unwrap();
                    bytes.pop();
                    bytes.extend_from_slice(b",\"version\":1}");
                    bytes
                }
                ResponseFault::SecretValue => {
                    value["tenant_id"] = serde_json::json!(state.secret);
                    serde_json::to_vec(&value).unwrap()
                }
                ResponseFault::Oversized => {
                    let mut oversized = bytes.to_vec();
                    assert!(
                        serde_json::from_slice::<KubernetesHeldCreateProtocolV1>(&oversized)
                            .is_ok(),
                        "the unmodified response is valid protocol JSON"
                    );
                    oversized.resize(129 * 1024, b' ');
                    assert!(
                        serde_json::from_slice::<KubernetesHeldCreateProtocolV1>(&oversized)
                            .is_ok(),
                        "whitespace padding preserves a valid JSON protocol response"
                    );
                    oversized
                }
                ResponseFault::Redirect => unreachable!(),
            };
            Response::from_parts(parts, Body::from(body))
        }
    }
}

#[tokio::test]
async fn client_rejects_redirects_duplicate_response_keys_secrets_and_oversize() {
    for fault in [
        ResponseFault::Redirect,
        ResponseFault::DuplicateKey,
        ResponseFault::SecretValue,
        ResponseFault::Oversized,
    ] {
        let f = Fixture::new(TENANT);
        let head = f.store().head().unwrap();
        let followed_redirects = Arc::new(AtomicUsize::new(0));
        let server = f
            .serve_with_fault(
                true,
                QuotaPolicyV1::default(),
                Some((fault, Arc::clone(&followed_redirects))),
            )
            .await;
        let client = f.client(&server.url);
        let error = client.protocol().await.unwrap_err();
        assert_ne!(error, HeldCreateClientError::Unavailable);
        assert_eq!(
            followed_redirects.load(Ordering::SeqCst),
            0,
            "redirects must not be followed"
        );
        assert!(
            !error.to_string().contains(&f.token),
            "client errors must not reveal protected request credentials"
        );
        assert_eq!(f.store().head().unwrap(), head);
    }

    let f = Fixture::new(TENANT);
    let head = f.store().head().unwrap();
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let client = f.client_with_known_secrets(&server.url, vec![f.intent.image.clone()]);
    let error = client
        .reserve(&f.intent)
        .await
        .err()
        .expect("known-secret request is refused locally");
    assert_eq!(error, HeldCreateClientError::InvalidRequest);
    assert!(!error.to_string().contains(&f.intent.image));
    assert_eq!(f.store().head().unwrap(), head);
}

#[tokio::test]
async fn explicit_ca_trust_is_required_for_a_tls_control_gateway() {
    let f = Fixture::new(TENANT);
    let server_material = TlsMaterial::create().await;
    let wrong_ca_material = TlsMaterial::create().await;
    let request_count = Arc::new(AtomicUsize::new(0));
    let server = f
        .serve_tls(&server_material, Arc::clone(&request_count))
        .await;

    let trusted_client = f.client_with_ca(&server.url, &server_material.ca);
    let protocol = trusted_client.protocol().await.unwrap();
    assert_eq!(protocol.schema_version, 9);
    assert_eq!(protocol.tenant_id, f.tenant);
    let after_trusted_request = request_count.load(Ordering::SeqCst);
    assert!(after_trusted_request > 0);

    let untrusted_client = f.client_with_ca(&server.url, &wrong_ca_material.ca);
    let error = untrusted_client.protocol().await.err().unwrap();
    assert_eq!(error, HeldCreateClientError::Transport);
    assert!(!error.to_string().contains(&f.token));
    assert_eq!(
        request_count.load(Ordering::SeqCst),
        after_trusted_request,
        "a certificate from an unconfigured CA must fail before HTTP reaches the gateway"
    );
    server.stop().await;
}

#[tokio::test]
async fn dropping_a_waiter_keeps_the_material_slot_until_owned_revalidation_finishes() {
    let f = Fixture::new(TENANT);
    let (server, request_count) = f.serve_counted().await;
    let client = Arc::new(f.client(&server.url));
    let (entered_tx, entered_rx) = oneshot::channel();
    let (finished_tx, finished_rx) = oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    client
        .pause_next_material_validation_for_test(entered_tx, release_rx, finished_tx)
        .expect("install a one-shot test barrier on this client instance");

    let waiting_client = Arc::clone(&client);
    let waiter = tokio::spawn(async move { waiting_client.protocol().await });
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("owned validation closure reaches the barrier")
        .expect("barrier sender remains owned by the validation closure");
    waiter.abort();
    let _ = waiter.await;

    let requests_before_capacity = request_count.load(Ordering::SeqCst);
    let capacity_error = client
        .protocol()
        .await
        .err()
        .expect("the held material slot rejects a concurrent client operation");
    assert_eq!(capacity_error, HeldCreateClientError::Capacity);
    assert_eq!(
        request_count.load(Ordering::SeqCst),
        requests_before_capacity,
        "capacity refusal occurs before another HTTP request"
    );

    release_tx
        .send(())
        .expect("the validation closure is still waiting at its barrier");
    tokio::time::timeout(Duration::from_secs(5), finished_rx)
        .await
        .expect("owned validation closure finishes within the harness watchdog")
        .expect("closure reports completion after releasing its slot");
    assert_eq!(
        request_count.load(Ordering::SeqCst),
        requests_before_capacity,
        "the abandoned waiter never sends its HTTP request"
    );

    client.protocol().await.unwrap();
    assert!(
        request_count.load(Ordering::SeqCst) > requests_before_capacity,
        "a new operation can acquire the released slot and reach the gateway"
    );
    server.stop().await;
}

#[test]
fn constructor_bounds_known_secrets_and_includes_the_bearer_in_its_distinct_count() {
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap())
        .expect("private constructor-only fixture");

    let max_value_accepted = client_from_config_secrets(&root, vec!["z".repeat(4096)]).is_ok();

    let distinct = (0..63)
        .map(|index| format!("private-known-{index:02}-{}", "q".repeat(4075)))
        .collect::<Vec<_>>();
    assert!(distinct.iter().all(|value| value.len() == 4092));
    let mut max_distinct = distinct.clone();
    max_distinct.push(distinct[0].clone());
    max_distinct.push(TOKEN.into());
    let max_distinct_accepted = client_from_config_secrets(&root, max_distinct).is_ok();

    let empty_secret_refused = client_from_config_secrets(&root, vec![String::new()])
        .err()
        .is_some_and(|error| error == HeldCreateClientError::InvalidConfiguration);
    let oversized_secret_refused = client_from_config_secrets(&root, vec!["z".repeat(4097)])
        .err()
        .is_some_and(|error| error == HeldCreateClientError::InvalidConfiguration);
    let over_count_refused = client_from_config_secrets(
        &root,
        (0..64)
            .map(|index| format!("distinct-private-secret-{index:02}"))
            .collect(),
    )
    .err()
    .is_some_and(|error| error == HeldCreateClientError::InvalidConfiguration);

    assert!(max_value_accepted, "a 4096-byte secret is within the bound");
    assert!(
        max_distinct_accepted,
        "63 distinct configured secrets plus the bearer make the 64-entry limit"
    );
    assert_eq!(
        (
            empty_secret_refused,
            oversized_secret_refused,
            over_count_refused,
        ),
        (true, true, true),
        "invalid secret configuration refusal results: empty={empty_secret_refused}, oversized={oversized_secret_refused}, over_count={over_count_refused}"
    );
}
