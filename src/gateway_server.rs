//! Shared ownership of the gateway's HTTP surfaces and recovery-auth epoch.
//!
//! The caller owns an asynchronous cleanup supervisor. Dropping the caller
//! retires authentication synchronously and asks that supervisor to stop both
//! acceptors. The supervisor retains the serving futures and blocking-usage
//! drain until they finish; dropping a caller never aborts a serving sibling.
use crate::api::GatewayControlRecoveryRetirementV2;
use anyhow::{Context, Result};
use axum::Router;
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::watch,
    task::{JoinHandle, JoinSet},
};

type ServingOwner = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
const HTTP_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Create this immediately after constructing GatewayState, before fallible
/// builders, signal installation or listener setup. It is deliberately not Clone.
pub struct GatewayServer {
    retirement: Arc<GatewayControlRecoveryRetirementV2>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<Result<()>>>,
}

impl GatewayServer {
    pub fn new(retirement: GatewayControlRecoveryRetirementV2) -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            retirement: Arc::new(retirement),
            stop,
            task: None,
        }
    }

    /// Bind all configured surfaces before polling any server. A successful
    /// return means both serving futures AND genuine recovery usages drained.
    /// There is no HTTP timeout standing in for the latter condition.
    pub async fn serve(
        mut self,
        public: (SocketAddr, Router),
        control: Option<(SocketAddr, Router)>,
        shutdown: impl Future<Output = ()> + Send + 'static,
        on_bound: impl FnOnce(SocketAddr, Option<SocketAddr>) -> Result<()> + Send + 'static,
    ) -> Result<()> {
        let retirement = Arc::clone(&self.retirement);
        let stop = self.stop.clone();
        self.task = Some(tokio::spawn(async move {
            let mut bound = BoundOwners {
                retirement,
                stop,
                owners: FuturesUnordered::new(),
            };
            let result = async {
                let public_address = bound.bind(public).await?;
                let control_address = match control {
                    Some(surface) => Some(bound.bind(surface).await?),
                    None => None,
                };
                anyhow::ensure!(!*bound.stop.borrow(), "gateway startup was cancelled");
                on_bound(public_address, control_address)?;
                supervise_owners(
                    std::mem::take(&mut bound.owners),
                    shutdown,
                    Arc::clone(&bound.retirement),
                    bound.stop.clone(),
                )
                .await
            }
            .await;
            // Covers bind/on_bound errors, as well as normal server completion.
            // Retire before dropping listeners that were never polled/served.
            bound.retire_and_stop();
            bound.owners.clear();
            wait_for_usage_drain(&bound.retirement).await;
            result
        }));
        self.task
            .as_mut()
            .context("gateway serving owner is unavailable")?
            .await
            .context("gateway serving owner stopped unexpectedly")?
    }
}

impl Drop for GatewayServer {
    fn drop(&mut self) {
        self.retirement.retire();
        self.stop.send_replace(true);
        // Do not abort the supervisor: it owns the remaining graceful shutdown
        // and waits for the actual blocking leases before releasing its owner.
        // Tokio runtime/process termination is outside this in-process contract.
    }
}

struct BoundOwners {
    retirement: Arc<GatewayControlRecoveryRetirementV2>,
    stop: watch::Sender<bool>,
    owners: FuturesUnordered<ServingOwner>,
}

impl BoundOwners {
    fn retire_and_stop(&self) {
        self.retirement.retire();
        self.stop.send_replace(true);
    }

    async fn bind(&mut self, (address, router): (SocketAddr, Router)) -> Result<SocketAddr> {
        let stopped = wait_for_stop(self.stop.subscribe());
        tokio::pin!(stopped);
        let listener = tokio::select! {
            biased;
            _ = &mut stopped => anyhow::bail!("gateway startup was cancelled"),
            listener = TcpListener::bind(address) => listener.context("gateway listener bind failed")?,
        };
        let address = listener.local_addr()?;
        self.owners.push(http_owner(
            listener,
            router,
            self.stop.clone(),
            Arc::clone(&self.retirement),
        ));
        Ok(address)
    }
}

impl Drop for BoundOwners {
    fn drop(&mut self) {
        // Also runs before owned listener futures are dropped during unwinding.
        self.retire_and_stop();
    }
}

async fn wait_for_stop(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

fn http_owner(
    mut listener: TcpListener,
    router: Router,
    stop: watch::Sender<bool>,
    retirement: Arc<GatewayControlRecoveryRetirementV2>,
) -> ServingOwner {
    async move {
        let mut connections = JoinSet::new();
        let stopped = wait_for_stop(stop.subscribe());
        tokio::pin!(stopped);
        let mut failure = loop {
            let accepted = tokio::select! {
                biased;
                _ = &mut stopped => break None,
                result = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = result {
                        // Retire before starting this owner's error cleanup,
                        // and before its sibling can be asked to drain.
                        retirement.retire();
                        stop.send_replace(true);
                        break Some(anyhow::Error::new(error).context("gateway connection owner failed"));
                    }
                    continue;
                }
                // Reuse Axum's accept retry policy, including its existing
                // one-second retry for resource errors; no new retry deadline.
                accepted = axum::serve::Listener::accept(&mut listener) => accepted,
            };
            let (stream, _remote_address) = accepted;
            let service = TowerToHyperService::new(router.clone());
            let connection_stop = stop.subscribe();
            connections.spawn(async move {
                // Match the current axum::serve builder/defaults and body
                // mapping through Router's Service implementation. Gateway
                // routers have no ConnectInfo make-service or upgrade handler.
                let builder = Builder::new(TokioExecutor::new());
                let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
                tokio::pin!(connection);
                tokio::select! {
                    _ = &mut connection => {}, // Axum also treats connection errors locally.
                    _ = wait_for_stop(connection_stop) => {
                        connection.as_mut().graceful_shutdown();
                        let _ = connection.await;
                    }
                }
            });
        };
        // Stop accepting before graceful drain; both sibling owners receive
        // the same stop only after the shared epoch has been retired.
        drop(listener);
        let graceful = async {
            while let Some(result) = connections.join_next().await {
                if let Err(error) = result
                    && failure.is_none()
                {
                    failure = Some(
                        anyhow::Error::new(error).context("gateway connection owner failed"),
                    );
                }
            }
        };
        if tokio::time::timeout(HTTP_GRACE_PERIOD, graceful).await.is_err() {
            eprintln!("Délai de drainage HTTP de 5 secondes atteint ; arrêt des connexions gateway.");
            connections.abort_all();
            // Abort is a request; await every owned task to prove its I/O and
            // stream bodies have actually been dropped before returning.
            while let Some(result) = connections.join_next().await {
                // Only cancellations after this owner's own abort_all are
                // expected. A panic remains an internal failure in this phase.
                if let Err(error) = result
                    && !error.is_cancelled()
                    && failure.is_none()
                {
                    failure = Some(
                        anyhow::Error::new(error).context("gateway connection owner failed"),
                    );
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    .boxed()
}

async fn supervise_owners(
    mut owners: FuturesUnordered<ServingOwner>,
    shutdown: impl Future<Output = ()> + Send,
    retirement: Arc<GatewayControlRecoveryRetirementV2>,
    stop: watch::Sender<bool>,
) -> Result<()> {
    tokio::pin!(shutdown);
    let caller_stop = wait_for_stop(stop.subscribe());
    tokio::pin!(caller_stop);
    let first = tokio::select! {
        _ = &mut shutdown => None,
        _ = &mut caller_stop => None,
        result = owners.next() => result,
    };
    // This order is essential: no sibling future is cancelled or gracefully
    // drained while its epoch can still issue a new usage lease.
    retirement.retire();
    stop.send_replace(true);
    let mut failure = first.and_then(Result::err);
    while let Some(result) = owners.next().await {
        if let Err(error) = result
            && failure.is_none()
        {
            failure = Some(error);
        }
    }
    wait_for_usage_drain(&retirement).await;
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn wait_for_usage_drain(retirement: &GatewayControlRecoveryRetirementV2) {
    while !retirement.drained() {
        // A scheduling cadence, not a completion deadline or permission expiry.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[cfg(test)]
#[path = "gateway_server_tests.rs"]
mod tests;
