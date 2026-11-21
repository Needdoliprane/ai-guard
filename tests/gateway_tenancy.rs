//! Real HTTP and process restart evidence for the tenant registry. This is not
//! evidence of worker/container isolation, which requires the runtime scenario.
mod support;

use agentsandbox::{
    control::{AdmissionRefusalV1, AdmissionResultV1, AdmissionV1, QuotaPolicyV1, WorkerStateV1},
    model::Event,
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, Command},
};

struct Gateway {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    base: String,
}

async fn start(config: &Path, max_events: u64) -> Gateway {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .current_dir(config.parent().unwrap())
        .args(["gateway", "--listen", "127.0.0.1:0", "--tenants-file"])
        .arg(config)
        .arg("--max-events")
        .arg(max_events.to_string())
        .arg("--policy")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    let received = tokio::time::timeout(Duration::from_secs(15), stderr.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(received > 0, "gateway did not report readiness");
    let address = line
        .strip_prefix("Gateway ")
        .and_then(|line| line.split_whitespace().next())
        .expect("gateway did not start successfully");
    let address: std::net::SocketAddr = address.parse().unwrap();
    assert!(address.ip().is_loopback());
    Gateway {
        child,
        _stderr: stderr,
        base: format!("http://{address}"),
    }
}

fn tenant_registry(root: &Path, a_quota: Option<&QuotaPolicyV1>) -> std::path::PathBuf {
    let config = root.join("tenants.json");
    let mut tenants = Vec::new();
    for (tenant, runner, viewer) in [("a", "r", "v"), ("b", "s", "w")] {
        std::fs::write(root.join(format!("{tenant}-runner")), runner.repeat(48)).unwrap();
        std::fs::write(root.join(format!("{tenant}-viewer")), viewer.repeat(48)).unwrap();
        let mut settings = json!({"tenant_id":format!("tenant-{tenant}"),"data":format!("{tenant}.sqlite"),
            "runner_token_file":format!("{tenant}-runner"),"viewer_token_file":format!("{tenant}-viewer")});
        if tenant == "a"
            && let Some(quota) = a_quota
        {
            settings["worker_quota"] = json!(quota);
        }
        tenants.push(settings);
    }
    std::fs::write(&config, json!({"version":1,"tenants":tenants}).to_string()).unwrap();
    config
}

#[tokio::test]
async fn configured_tenants_remain_isolated_over_http_and_after_control_plane_crash() {
    let dir = support::private_tempdir();
    let config = tenant_registry(dir.path(), None);
    let mut gateway = start(&config, 100).await;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .unwrap();
    for (tenant, runner, viewer) in [("a", "r", "v"), ("b", "s", "w")] {
        let identity: Value = http
            .get(format!("{}/api/v1/identity", gateway.base))
            .bearer_auth(viewer.repeat(48))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            identity,
            json!({"tenant_id":format!("tenant-{tenant}"),"role":"viewer"})
        );
        let mut event = Event::new("test.tenant");
        event.data = json!({"marker":format!("private-to-{tenant}")});
        let response = http
            .post(format!("{}/api/v1/events", gateway.base))
            .bearer_auth(runner.repeat(48))
            .json(&event)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let response = http
            .post(format!("{}/api/v1/events", gateway.base))
            .bearer_auth(viewer.repeat(48))
            .json(&event)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    // Killing only the child created by this test exercises WAL durability;
    // no preexisting process, database or container is touched.
    gateway.child.kill().await.unwrap();
    drop(gateway);
    let mut gateway = start(&config, 100).await;
    for (tenant, viewer, other) in [("a", "v", "b"), ("b", "w", "a")] {
        for route in ["events", "export?format=json"] {
            let response = http
                .get(format!("{}/api/v1/{route}", gateway.base))
                .bearer_auth(viewer.repeat(48))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(response.contains(&format!("private-to-{tenant}")));
            assert!(!response.contains(&format!("private-to-{other}")));
        }
        let integrity: Value = http
            .get(format!("{}/api/v1/integrity", gateway.base))
            .bearer_auth(viewer.repeat(48))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(integrity["valid"], true);
        assert_eq!(integrity["event_count"], 2); // tenant genesis + private event
    }
    let response = http
        .get(format!(
            "{}/api/v1/identity?tenant_id=tenant-b",
            gateway.base
        ))
        .header("x-tenant-id", "tenant-b")
        .bearer_auth("v".repeat(48))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(response["tenant_id"], "tenant-a");
    let response = http
        .get(format!("{}/api/v1/events", gateway.base))
        .bearer_auth("worker-unregistered".repeat(3))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    gateway.child.kill().await.unwrap();
}

#[tokio::test]
async fn new_decisions_resume_after_audit_capacity_is_raised_and_the_gateway_reopens() {
    let dir = support::private_tempdir();
    let config = tenant_registry(dir.path(), None);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .unwrap();
    let request = || {
        json!({
            "request_id": uuid::Uuid::new_v4(),
            "profile": "strict",
            "command": {"argv": ["/usr/bin/id"], "cwd": "/workspace"}
        })
    };
    let first = request();
    let later = request();
    let mut gateway = start(&config, 2).await;
    let decide = |gateway: &Gateway, runner: &str, payload: &Value| {
        http.post(format!("{}/api/v1/decide", gateway.base))
            .bearer_auth(runner.repeat(48))
            .json(payload)
            .send()
    };
    let integrity = |gateway: &Gateway, viewer: &str| {
        http.get(format!("{}/api/v1/integrity", gateway.base))
            .bearer_auth(viewer.repeat(48))
            .send()
    };
    let original: Value = decide(&gateway, "r", &first)
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(original["decision"]["outcome"], "allow");
    assert_eq!(original["receipt"]["request_id"], first["request_id"]);
    assert_eq!(original["audit_seq"], 2);
    let full = decide(&gateway, "r", &later).await.unwrap();
    assert_eq!(full.status(), 503);
    assert_eq!(
        full.json::<Value>().await.unwrap(),
        json!({"error":"Audit indisponible, incohérent ou plein : opération refusée."})
    );
    let isolated: Value = decide(&gateway, "s", &request())
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(isolated["decision"]["outcome"], "allow");
    assert_eq!(isolated["audit_seq"], 2);
    assert_ne!(isolated["audit_hash"], original["audit_hash"]);
    let before: Value = integrity(&gateway, "v")
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["valid"], true);
    assert_eq!(before["event_count"], 2);
    assert_eq!(before["head_hash"], original["audit_hash"]);
    gateway.child.kill().await.unwrap();
    drop(gateway);

    let mut gateway = start(&config, 100).await;
    let retried: Value = decide(&gateway, "r", &later)
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retried["decision"]["outcome"], "allow");
    assert_eq!(retried["receipt"]["request_id"], later["request_id"]);
    assert_eq!(retried["audit_seq"], 3);
    for (payload, expected) in [(&first, &original), (&later, &retried)] {
        let replay: Value = decide(&gateway, "r", payload)
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(&replay, expected);
    }
    for (viewer, expected) in [("v", &retried), ("w", &isolated)] {
        let verified: Value = integrity(&gateway, viewer)
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(verified["valid"], true);
        assert_eq!(verified["event_count"], expected["audit_seq"]);
        assert_eq!(verified["head_hash"], expected["audit_hash"]);
    }
    gateway.child.kill().await.unwrap();
}

async fn fresh_worker_admission(
    gateway: &Gateway,
    http: &Client,
    runner: &str,
    template: &AdmissionV1,
) -> AdmissionV1 {
    let mut admission = template.clone();
    let session_id = uuid::Uuid::new_v4().to_string();
    admission.operation_id = uuid::Uuid::new_v4().to_string();
    admission.decision_request.request_id = uuid::Uuid::new_v4().to_string();
    admission.decision_request.session_id = Some(session_id.clone());
    admission.launch.session_id = session_id;
    let decision: Value = http
        .post(format!("{}/api/v1/decide", gateway.base))
        .bearer_auth(runner.repeat(48))
        .json(&admission.decision_request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(decision["decision"]["outcome"], "allow");
    admission.receipt = serde_json::from_value(decision["receipt"].clone()).unwrap();
    admission
}

async fn reserve_worker(
    gateway: &Gateway,
    http: &Client,
    runner: &str,
    admission: &AdmissionV1,
    expected_status: StatusCode,
) -> AdmissionResultV1 {
    let response = http
        .post(format!("{}/api/v1/workers/reservations", gateway.base))
        .bearer_auth(runner.repeat(48))
        .json(admission)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), expected_status);
    let result: AdmissionResultV1 = response.json().await.unwrap();
    assert_eq!(result.receipt.operation_id, admission.operation_id);
    result
}

#[tokio::test]
async fn configured_worker_quota_at_hundred_survives_gateway_restart() {
    let dir = support::private_tempdir();
    let quota = QuotaPolicyV1 {
        version: 1,
        max_active_workers: 100,
        max_cpu_millis: 200_000,
        max_memory_bytes: 100 * 1024 * 1024 * 1024,
        max_timeout_seconds: 3600,
    };
    let config = tenant_registry(dir.path(), Some(&quota));
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap();
    // Reuse the supported launch DTO. Only the reservation route is exercised.
    let template: AdmissionV1 = serde_json::from_str(include_str!(
        "fixtures/control/admission-fixed-sandbox-v1.json"
    ))
    .unwrap();
    let get = |gateway: &Gateway, viewer: &str, route: &str| {
        http.get(format!("{}/api/v1/{route}", gateway.base))
            .bearer_auth(viewer.repeat(48))
    };
    let empty_page = json!({"items":[],"next_cursor":null});
    let full_quota = json!({"policy":quota,"held_workers":100,"held_cpu_millis":200_000,
        "held_memory_bytes":107_374_182_400_u64});
    let mut gateway = start(&config, 1000).await;
    let mut reserved_ids = BTreeSet::new();
    for _ in 0..100 {
        let admission = fresh_worker_admission(&gateway, &http, "r", &template).await;
        let result = reserve_worker(&gateway, &http, "r", &admission, StatusCode::CREATED).await;
        assert!(result.accepted && !result.replayed && result.refusal.is_none());
        let worker = result.worker.unwrap();
        assert_eq!(worker.state, WorkerStateV1::Reserved);
        assert!(worker.quota_held && worker.attempt.container_id.is_none());
        assert!(reserved_ids.insert(worker.worker_id));
    }
    // The first 100 decisions and reservations can fill the 200 writes/s window.
    // Keep the quota refusal independent of the HTTP write-rate limit.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let excess = fresh_worker_admission(&gateway, &http, "r", &template).await;
    let denied = reserve_worker(&gateway, &http, "r", &excess, StatusCode::TOO_MANY_REQUESTS).await;
    assert_eq!(denied.refusal, Some(AdmissionRefusalV1::Quota));
    assert!(!denied.accepted && !denied.replayed && denied.worker.is_none());
    let before: Value = read_json(get(&gateway, "v", "workers?limit=60")).await;
    let first = before["items"].as_array().unwrap();
    assert_eq!(first.len(), 60);
    let cursor = before["next_cursor"].as_str().unwrap();
    assert_eq!(first[59]["worker_id"], cursor);
    assert_eq!(read_json(get(&gateway, "v", "quota")).await, full_quota);
    assert_eq!(read_json(get(&gateway, "v", "usage")).await, empty_page);

    // Kill only the child started by this test and reopen the same tenant DBs.
    gateway.child.kill().await.unwrap();
    drop(gateway);
    let mut gateway = start(&config, 1000).await;
    assert_eq!(
        read_json(get(&gateway, "v", "workers?limit=60")).await,
        before
    );
    let after = read_json(get(
        &gateway,
        "v",
        &format!("workers?limit=60&cursor={cursor}"),
    ))
    .await;
    let second = after["items"].as_array().unwrap();
    assert_eq!(second.len(), 40);
    assert!(after["next_cursor"].is_null());
    let listed: Vec<_> = first
        .iter()
        .chain(second)
        .map(|worker| worker["worker_id"].as_str().unwrap().to_owned())
        .collect();
    assert!(listed.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(listed.into_iter().collect::<BTreeSet<_>>(), reserved_ids);
    assert_eq!(read_json(get(&gateway, "v", "quota")).await, full_quota);
    assert_eq!(read_json(get(&gateway, "v", "usage")).await, empty_page);
    let replay = reserve_worker(&gateway, &http, "r", &excess, StatusCode::TOO_MANY_REQUESTS).await;
    assert_eq!(replay.refusal, Some(AdmissionRefusalV1::Quota));
    assert!(!replay.accepted && replay.replayed && replay.worker.is_none());
    assert_eq!(replay.receipt, denied.receipt);
    let integrity = read_json(get(&gateway, "v", "integrity")).await;
    assert_eq!(integrity["valid"], true);
    assert_eq!(integrity["event_count"], denied.receipt.audit_seq);
    assert_eq!(integrity["head_hash"], denied.receipt.audit_hash);

    assert_eq!(read_json(get(&gateway, "w", "workers")).await, empty_page);
    let b_admission = fresh_worker_admission(&gateway, &http, "s", &template).await;
    let b_result = reserve_worker(&gateway, &http, "s", &b_admission, StatusCode::CREATED).await;
    let b_worker = b_result.worker.unwrap();
    let b_page = read_json(get(&gateway, "w", "workers")).await;
    assert_eq!(b_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(b_page["items"][0]["worker_id"], b_worker.worker_id);
    assert_eq!(
        read_json(get(&gateway, "w", "quota")).await["held_workers"],
        1
    );
    assert_eq!(read_json(get(&gateway, "v", "quota")).await, full_quota);
    gateway.child.kill().await.unwrap();
}

async fn read_json(request: reqwest::RequestBuilder) -> Value {
    request
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
