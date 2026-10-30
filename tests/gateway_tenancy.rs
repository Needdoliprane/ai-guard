//! Real HTTP and process restart evidence for the tenant registry. This is not
//! evidence of worker/container isolation, which requires the runtime scenario.
mod support;

use agentsandbox::model::Event;
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, Command},
};

struct Gateway {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    base: String,
}

async fn start(config: &Path) -> Gateway {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["gateway", "--listen", "127.0.0.1:0", "--tenants-file"])
        .arg(config)
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

#[tokio::test]
async fn configured_tenants_remain_isolated_over_http_and_after_control_plane_crash() {
    let dir = support::private_tempdir();
    let config = dir.path().join("tenants.json");
    let mut tenants = Vec::new();
    for (tenant, runner, viewer) in [("a", "r", "v"), ("b", "s", "w")] {
        std::fs::write(
            dir.path().join(format!("{tenant}-runner")),
            runner.repeat(48),
        )
        .unwrap();
        std::fs::write(
            dir.path().join(format!("{tenant}-viewer")),
            viewer.repeat(48),
        )
        .unwrap();
        tenants.push(json!({"tenant_id":format!("tenant-{tenant}"),"data":format!("{tenant}.sqlite"),
            "runner_token_file":format!("{tenant}-runner"),"viewer_token_file":format!("{tenant}-viewer")}));
    }
    std::fs::write(&config, json!({"version":1,"tenants":tenants}).to_string()).unwrap();
    let mut gateway = start(&config).await;
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
    let mut gateway = start(&config).await;
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
