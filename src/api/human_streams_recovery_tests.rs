//! Controlled read-gate saturation explains an SSE error family. This is not
//! a natural-load reproduction, an ingress-pool RED, or a latency benchmark.
use crate::api::{self, gateway_router, store_io};
use anyhow::{Result, anyhow, ensure};
use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};

const BOUND: Duration = Duration::from_secs(60);
const FRAME_BOUND: usize = 8 * 1024;

struct Server {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl Server {
    async fn start(router: Router) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .map_err(|_| anyhow!("fixture HTTP server failed"))
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
            .ok_or_else(|| anyhow!("cleanup: server owner absent"))?;
        match tokio::time::timeout(BOUND, &mut task).await {
            Ok(joined) => joined.map_err(|_| anyhow!("cleanup: server task failed"))?,
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(anyhow!("cleanup: server drain observation deadline"))
            }
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

struct RequestOwner(Option<JoinHandle<Result<reqwest::Response>>>);
impl RequestOwner {
    async fn response(&mut self) -> Result<reqwest::Response> {
        self.0
            .take()
            .ok_or_else(|| anyhow!("setup: HTTP owner absent"))?
            .await
            .map_err(|_| anyhow!("setup: HTTP owner failed"))?
    }
}
impl Drop for RequestOwner {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    tokio::time::timeout(BOUND, request.send())
        .await
        .map_err(|_| anyhow!("setup: HTTP headers observation deadline"))?
        .map_err(|_| anyhow!("setup: HTTP transport failed"))
}
async fn json(mut response: reqwest::Response) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    tokio::time::timeout(BOUND, async {
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow!("setup: JSON transport failed"))?
        {
            ensure!(
                chunk.len() <= (64 * 1024usize).saturating_sub(bytes.len()),
                "setup: JSON byte bound"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("setup: JSON shape invalid"))
    })
    .await
    .map_err(|_| anyhow!("setup: JSON observation deadline"))?
}

// The test records only a fixed family; no arbitrary code/message/body enters
// its diagnostics. The real error code is nested under error.code.
fn is_capacity_frame(frame: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Code,
    }
    #[derive(serde::Deserialize)]
    struct Code {
        code: String,
    }
    let is_error = frame
        .split(|byte| *byte == b'\n')
        .any(|line| line == b"event: error");
    let mut data = frame
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"data: "));
    let Some(payload) = data.next() else {
        return false;
    };
    is_error
        && data.next().is_none()
        && serde_json::from_slice::<Envelope>(payload)
            .is_ok_and(|envelope| envelope.error.code == "capacity_exhausted")
}

#[derive(Clone, Copy)]
enum Saturated {
    Tenant,
    Shared,
}

async fn capacity_case(saturated: Saturated) -> Result<()> {
    let (_files, state, token) = api::read_execution_tests::local_identity_state();
    let (arrived, arrival) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    let barrier = Arc::new(Mutex::new(Some((arrived, resume))));
    let router = gateway_router(state.clone()).layer(axum::middleware::from_fn(
        move |request: Request, next: axum::middleware::Next| {
            let barrier = Arc::clone(&barrier);
            async move {
                let stream = request.uri().path() == "/api/v1/human/stream";
                let response = next.run(request).await;
                let parked = if stream && response.status() == StatusCode::OK {
                    barrier.lock().ok().and_then(|mut held| held.take())
                } else {
                    None
                };
                if let Some((arrived, resume)) = parked {
                    // Auth/context/SSE admission finished. The real response
                    // Body has not yet been polled by the HTTP server.
                    let _ = arrived.send(());
                    let _ = resume.await;
                }
                response
            }
        },
    ));
    let gateway = Server::start(router).await?;
    let ingress = Server::start(crate::ingress::ingress_router(&gateway.url)?).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .build()?;
    let outcome: Result<()> = async {
        let identity = send(
            client
                .get(format!("{}/api/v1/human/identity", ingress.url))
                .bearer_auth(&token),
        )
        .await?;
        ensure!(
            identity.status() == StatusCode::OK,
            "setup: local authentication refused"
        );
        let identity = json(identity).await?;
        let context = identity["contexts"]
            .as_array()
            .and_then(|contexts| {
                contexts
                    .iter()
                    .find(|entry| entry["tenant_id"] == "tenant-b")
            })
            .and_then(|entry| entry["id"].as_str())
            .ok_or_else(|| anyhow!("setup: source context absent"))?;
        let request = client
            .get(format!("{}/api/v1/human/stream", ingress.url))
            .bearer_auth(&token)
            .header("x-aiguard-context", context);
        let mut owner = RequestOwner(Some(tokio::spawn(send(request))));
        tokio::time::timeout(BOUND, arrival)
            .await
            .map_err(|_| anyhow!("setup: post-admission response barrier deadline"))?
            .map_err(|_| anyhow!("setup: post-admission response barrier lost"))?;
        let gate = match saturated {
            Saturated::Tenant => &state.tenants[1].io,
            Saturated::Shared => &state.io,
        };
        let mut permits = Vec::new();
        while let Ok(permit) = gate.admit(store_io::Lane::Read, false) {
            permits.push(permit);
        }
        ensure!(
            permits.len()
                == match saturated {
                    Saturated::Tenant => 4,
                    Saturated::Shared => 32,
                },
            "setup: production read capacity was not fully retained"
        );
        let observations =
            crate::audit::read_test_observer::observe(state.tenants[1].store.sqlite().unwrap());
        release
            .send(())
            .map_err(|_| anyhow!("setup: response barrier owner lost"))?;
        let mut response = owner.response().await?;
        ensure!(
            response.status() == StatusCode::OK,
            "setup: SSE did not retain its admitted HTTP200"
        );
        ensure!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream")),
            "setup: SSE content type absent"
        );
        let mut bytes = Vec::new();
        tokio::time::timeout(BOUND, async {
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow!("SSE transport failed"))?
            {
                ensure!(
                    chunk.len() <= FRAME_BOUND.saturating_sub(bytes.len()),
                    "SSE byte bound"
                );
                bytes.extend_from_slice(&chunk);
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow!("SSE terminal observation deadline"))??;
        ensure!(
            bytes.ends_with(b"\n\n") && is_capacity_frame(&bytes[..bytes.len() - 2]),
            "expected one terminal structured SSE Capacity family"
        );
        ensure!(
            !observations
                .try_iter()
                .any(|sample| sample.call == crate::audit::read_test_observer::Call::BeginChecked),
            "saturated read gate unexpectedly opened a Store transaction"
        );
        drop(response);
        drop(permits);
        // This is a separate new request with the same real identity/context.
        // Recovery is not a transparent continuation of the terminated stream.
        let mut recovered = send(
            client
                .get(format!("{}/api/v1/human/stream", ingress.url))
                .bearer_auth(&token)
                .header("x-aiguard-context", context),
        )
        .await?;
        ensure!(
            recovered.status() == StatusCode::OK,
            "reconnect: admitted context not reusable"
        );
        let mut frame = Vec::new();
        tokio::time::timeout(BOUND, async {
            loop {
                let chunk = recovered
                    .chunk()
                    .await
                    .map_err(|_| anyhow!("reconnect: SSE transport failed"))?
                    .ok_or_else(|| anyhow!("reconnect: SSE ended"))?;
                ensure!(
                    chunk.len() <= FRAME_BOUND.saturating_sub(frame.len()),
                    "reconnect: SSE frame bound"
                );
                frame.extend_from_slice(&chunk);
                while let Some(end) = frame.windows(2).position(|part| part == b"\n\n") {
                    let current = &frame[..end];
                    ensure!(
                        !current
                            .split(|byte| *byte == b'\n')
                            .any(|line| line == b"event: error"),
                        "reconnect: SSE returned an error family"
                    );
                    if current == b": heartbeat" {
                        return Ok::<(), anyhow::Error>(());
                    }
                    frame.drain(..end + 2);
                }
            }
        })
        .await
        .map_err(|_| anyhow!("reconnect: product heartbeat observation deadline"))??;
        drop(recovered);
        ensure!(
            observations
                .try_iter()
                .any(|sample| sample.call == crate::audit::read_test_observer::Call::BeginChecked),
            "reconnect: no real Store read observed"
        );
        Ok(())
    }
    .await;
    drop(client);
    let ingress_cleanup = ingress.finish().await;
    let gateway_cleanup = gateway.finish().await;
    ingress_cleanup?;
    gateway_cleanup?;
    outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tenant_read_saturation_terminates_sse_with_capacity_then_allows_explicit_reconnect() {
    capacity_case(Saturated::Tenant).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_read_saturation_terminates_sse_with_capacity_then_allows_explicit_reconnect() {
    capacity_case(Saturated::Shared).await.unwrap();
}
