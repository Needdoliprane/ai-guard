#![cfg(unix)]

mod support;

use agentsandbox::model::Event;
use serde_json::json;
use std::{net::SocketAddr, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, Command},
};

fn viewer_token() -> String {
    "v".repeat(48)
}

fn runner_token() -> String {
    "r".repeat(48)
}

struct Service {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    address: SocketAddr,
}

async fn readiness(http: &reqwest::Client, address: SocketAddr) -> Result<(), String> {
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        http.get(format!("http://{address}/healthz")).send(),
    )
    .await
    .map_err(|_| "health request timed out".to_owned())?
    .map_err(|error| format!("health request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("health returned {}", response.status()));
    }
    Ok(())
}

fn child_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    // Keep these child services independent from any developer shell credentials.
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn start_gateway(dir: &tempfile::TempDir) -> Service {
    let runner_file = dir.path().join("runner-token");
    let viewer_file = dir.path().join("viewer-token");
    std::fs::write(&runner_file, runner_token()).unwrap();
    std::fs::write(&viewer_file, viewer_token()).unwrap();
    let policy = Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json");
    let mut child = child_command();
    child
        .args(["gateway", "--listen", "127.0.0.1:0", "--data"])
        .arg(dir.path().join("audit.sqlite"))
        .arg("--runner-token-file")
        .arg(runner_file)
        .arg("--viewer-token-file")
        .arg(viewer_file)
        .arg("--policy")
        .arg(policy);
    let mut child = child.spawn().expect("spawn gateway child");
    let mut stderr = BufReader::new(child.stderr.take().expect("gateway stderr"));
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(8), stderr.read_line(&mut line))
        .await
        .expect("gateway startup timed out")
        .expect("read gateway startup");
    assert!(read > 0, "gateway exited before readiness: {line:?}");
    let address = line
        .strip_prefix("Gateway ")
        .and_then(|rest| rest.split_whitespace().next())
        .expect("gateway did not report its address")
        .parse::<SocketAddr>()
        .expect("gateway reported an invalid address");
    assert!(address.ip().is_loopback());
    Service {
        child,
        _stderr: stderr,
        address,
    }
}

async fn start_dashboard(dir: &tempfile::TempDir, gateway: SocketAddr) -> Service {
    let viewer_file = dir.path().join("dashboard-viewer-token");
    let front = dir.path().join("front");
    std::fs::create_dir(&front).unwrap();
    std::fs::write(
        front.join("index.html"),
        b"<!doctype html><title>test</title>",
    )
    .unwrap();
    std::fs::write(&viewer_file, viewer_token()).unwrap();
    let mut child = child_command();
    child
        .args(["dashboard", "--listen", "127.0.0.1:0", "--upstream"])
        .arg(format!("http://{gateway}"))
        .arg("--viewer-token-file")
        .arg(viewer_file)
        .arg("--front-dir")
        .arg(front);
    let mut child = child.spawn().expect("spawn dashboard child");
    let mut stderr = BufReader::new(child.stderr.take().expect("dashboard stderr"));
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(8), stderr.read_line(&mut line))
        .await
        .expect("dashboard startup timed out")
        .expect("read dashboard startup");
    assert!(read > 0, "dashboard exited before readiness: {line:?}");
    let address = line
        .strip_prefix("Dashboard ")
        .and_then(|rest| rest.split_whitespace().next())
        .expect("dashboard did not report its address")
        .parse::<SocketAddr>()
        .expect("dashboard reported an invalid address");
    assert!(address.ip().is_loopback());
    Service {
        child,
        _stderr: stderr,
        address,
    }
}

async fn open_sse(http: &reqwest::Client, address: SocketAddr, route: &str) -> reqwest::Response {
    let response = tokio::time::timeout(
        Duration::from_secs(4),
        http.get(format!("http://{address}{route}"))
            .bearer_auth(viewer_token())
            .send(),
    )
    .await
    .expect("SSE response headers timed out")
    .expect("SSE request failed");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"))
    );
    response
}

async fn next_sse_text(response: &mut reqwest::Response) -> String {
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let chunk = response
                .chunk()
                .await
                .expect("SSE stream failed")
                .expect("SSE stream ended unexpectedly");
            text.push_str(&String::from_utf8_lossy(&chunk));
            if !text.is_empty() {
                break;
            }
        }
    })
    .await
    .expect("SSE stream produced no initial event");
    text
}

async fn next_sse_containing(response: &mut reqwest::Response, marker: &str) -> bool {
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let Some(chunk) = response.chunk().await? else {
                return Ok::<bool, reqwest::Error>(false);
            };
            text.push_str(&String::from_utf8_lossy(&chunk));
            if text.contains(marker) {
                return Ok(true);
            }
        }
    })
    .await
    .is_ok_and(|result| result.unwrap_or(false))
}

async fn append_event(
    http: &reqwest::Client,
    address: SocketAddr,
    marker: &str,
) -> Result<(), String> {
    let mut event = Event::new("shutdown.probe");
    event.data = json!({"marker": marker});
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        http.post(format!("http://{address}/api/v1/events"))
            .bearer_auth(runner_token())
            .json(&event)
            .send(),
    )
    .await
    .map_err(|_| "request timed out after 3s".to_owned())?
    .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("append returned {}", response.status()));
    }
    Ok(())
}

fn signal_child(child: &Child, signal: libc::c_int) -> Result<(), String> {
    let pid = child.id().ok_or_else(|| "child has no pid".to_owned())?;
    let pid = i32::try_from(pid).map_err(|_| "child pid is out of range".to_owned())?;
    // SAFETY: the child handle owns this live process and is reaped before it is dropped.
    let result = unsafe { libc::kill(pid, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

async fn await_bounded(
    service: &mut Service,
    label: &str,
    signal_at: tokio::time::Instant,
    max: Duration,
) -> Result<(), String> {
    let deadline = signal_at + max;
    match tokio::time::timeout_at(deadline, service.child.wait()).await {
        Ok(Ok(status)) if status.success() => {
            let elapsed = signal_at.elapsed();
            println!("{label} signal-to-exit: {} ms", elapsed.as_millis());
            if elapsed <= max {
                Ok(())
            } else {
                Err(format!(
                    "{label} exited after {elapsed:?}, beyond the {max:?} signal-to-exit limit"
                ))
            }
        }
        Ok(Ok(status)) => Err(format!("{label} exited with {status}")),
        Ok(Err(error)) => Err(format!("{label} wait failed: {error}")),
        Err(_) => {
            let cleanup = async {
                service.child.start_kill().map_err(|e| e.to_string())?;
                tokio::time::timeout(Duration::from_secs(2), service.child.wait())
                    .await
                    .map_err(|_| "forced child cleanup timed out".to_owned())?
                    .map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            }
            .await;
            Err(format!(
                "{label} did not exit within {max:?} from SIGTERM; elapsed={} ms; targeted kill cleanup: {cleanup:?}",
                signal_at.elapsed().as_millis()
            ))
        }
    }
}

#[tokio::test]
async fn gateway_and_dashboard_drain_open_sse_then_exit_within_eight_seconds() {
    let dir = support::private_tempdir();
    let http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut gateway = start_gateway(&dir).await;
    let mut dashboard = start_dashboard(&dir, gateway.address).await;
    readiness(&http, gateway.address)
        .await
        .expect("gateway readiness");
    readiness(&http, dashboard.address)
        .await
        .expect("dashboard readiness");

    let mut gateway_stream = open_sse(&http, gateway.address, "/api/v1/stream?after=0").await;
    let mut dashboard_stream = open_sse(&http, dashboard.address, "/api/stream?after=0").await;
    for (label, response) in [
        ("gateway", &mut gateway_stream),
        ("dashboard", &mut dashboard_stream),
    ] {
        let first = next_sse_text(response).await;
        assert!(
            first.contains("event: audit") || first.contains("data:"),
            "{label} stream did not deliver an initial SSE event: {first:?}"
        );
    }

    let mut failures = Vec::new();
    tokio::time::sleep(Duration::from_secs(6)).await;
    for (label, service) in [("gateway", &mut gateway), ("dashboard", &mut dashboard)] {
        if let Err(error) = readiness(&http, service.address).await {
            failures.push(format!("{label} was not healthy before SIGTERM: {error}"));
        }
        match service.child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => failures.push(format!(
                "{label} exited before SIGTERM after the 6s pre-signal phase: {status}"
            )),
            Err(error) => failures.push(format!("{label} liveness check failed: {error}")),
        }
    }
    let dashboard_marker = "dashboard-drain-still-live";
    if let Err(error) = append_event(&http, gateway.address, dashboard_marker).await {
        failures.push(format!("prepare dashboard drain event: {error}"));
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    let dashboard_signal_at = tokio::time::Instant::now();
    if let Err(error) = signal_child(&dashboard.child, libc::SIGTERM) {
        failures.push(format!("dashboard SIGTERM: {error}"));
    }
    if !next_sse_containing(&mut gateway_stream, dashboard_marker).await {
        failures.push("direct gateway SSE stopped before its drain window".into());
    }
    if !next_sse_containing(&mut dashboard_stream, dashboard_marker).await {
        failures.push("dashboard SSE stopped before its drain window".into());
    }
    if let Err(error) = await_bounded(
        &mut dashboard,
        "dashboard",
        dashboard_signal_at,
        Duration::from_secs(8),
    )
    .await
    {
        failures.push(error);
    }

    let mut final_stream = open_sse(&http, gateway.address, "/api/v1/stream?after=0").await;
    let gateway_marker = "gateway-drain-still-live";
    if let Err(error) = append_event(&http, gateway.address, gateway_marker).await {
        failures.push(format!("prepare gateway drain event: {error}"));
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    let gateway_signal_at = tokio::time::Instant::now();
    if let Err(error) = signal_child(&gateway.child, libc::SIGTERM) {
        failures.push(format!("gateway SIGTERM: {error}"));
    }
    if !next_sse_containing(&mut final_stream, gateway_marker).await {
        failures.push("gateway SSE stopped before its drain window".into());
    }
    if let Err(error) = await_bounded(
        &mut gateway,
        "gateway",
        gateway_signal_at,
        Duration::from_secs(8),
    )
    .await
    {
        failures.push(error);
    }
    drop((gateway_stream, dashboard_stream, final_stream));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn gateway_and_dashboard_without_streams_exit_promptly_after_sigterm() {
    let dir = support::private_tempdir();
    let http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut gateway = start_gateway(&dir).await;
    let mut dashboard = start_dashboard(&dir, gateway.address).await;
    readiness(&http, gateway.address)
        .await
        .expect("gateway readiness");
    readiness(&http, dashboard.address)
        .await
        .expect("dashboard readiness");

    let mut failures = Vec::new();
    let dashboard_signal_at = tokio::time::Instant::now();
    if let Err(error) = signal_child(&dashboard.child, libc::SIGTERM) {
        failures.push(format!("dashboard SIGTERM: {error}"));
    }
    let gateway_signal_at = tokio::time::Instant::now();
    if let Err(error) = signal_child(&gateway.child, libc::SIGINT) {
        failures.push(format!("gateway SIGINT: {error}"));
    }
    for (label, service, signal_at) in [
        ("dashboard", &mut dashboard, dashboard_signal_at),
        ("gateway", &mut gateway, gateway_signal_at),
    ] {
        if let Err(error) = await_bounded(service, label, signal_at, Duration::from_secs(3)).await {
            failures.push(error);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
