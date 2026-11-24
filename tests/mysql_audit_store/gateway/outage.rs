//! SQL-05 connectivity recovery with the configured Legacy MySQL target.
use super::*;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinSet,
};

const MAX_LINK_FLOWS: usize = 8;

struct LinkChange {
    available: bool,
    confirmed: oneshot::Sender<()>,
}

async fn set_link(changes: &mpsc::Sender<LinkChange>, available: bool) -> Result<()> {
    timeout(DEADLINE, async {
        let (confirmed, applied) = oneshot::channel();
        changes
            .send(LinkChange {
                available,
                confirmed,
            })
            .await
            .map_err(|_| anyhow::anyhow!("SQL-05 fixture link closed"))?;
        applied
            .await
            .context("SQL-05 fixture link change unconfirmed")
    })
    .await
    .context("SQL-05 fixture link change timed out")?
}

/// Forward only to the existing loopback MySQL fixture. The gateway and
/// MySQL server negotiate TLS directly through this stream.
async fn serve_link(
    listener: TcpListener,
    target: SocketAddr,
    mut changes: mpsc::Receiver<LinkChange>,
) -> Result<()> {
    let mut available = true;
    let mut flows = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            change = changes.recv() => {
                let Some(change) = change else { break Ok(()) };
                available = change.available;
                if !available {
                    flows.shutdown().await;
                }
                let _ = change.confirmed.send(());
            }
            _ = flows.join_next(), if !flows.is_empty() => {}
            accepted = listener.accept() => {
                let client = match accepted {
                    Ok((client, _)) => client,
                    Err(error) => break Err(error.into()),
                };
                if available && flows.len() < MAX_LINK_FLOWS {
                    flows.spawn(async move {
                        let mut client = client;
                        if let Ok(mut mysql) = TcpStream::connect(target).await {
                            let _ = copy_bidirectional(&mut client, &mut mysql).await;
                        }
                    });
                }
            }
        }
    };
    flows.shutdown().await;
    result
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a built gateway"]
async fn gateway_replays_mysql_batch_after_a_storage_connection_outage() -> Result<()> {
    let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let (mut conn_a, mut conn_b) = fresh_gateway_pair(&a, &b).await?;
    prepare_gateway_target(&mut conn_a, &a, "tenant-a", &DIGEST_A).await?;
    prepare_gateway_target(&mut conn_b, &b, "tenant-b", &DIGEST_B).await?;
    let before_a = batch_snapshot(&mut conn_a).await?;
    let witness_b = batch_snapshot(&mut conn_b).await?;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, a.target.port));
    let registry = Registry::new()?;
    let mut entry_a = registry.entry(&a, "tenant-a", "a")?;
    entry_a["storage"]["port"] = json!(listener.local_addr()?.port());
    registry.write(&[entry_a, registry.entry(&b, "tenant-b", "b")?])?;
    let (link, changes) = mpsc::channel(1);

    let exercise = async move {
        let http = Client::builder().no_proxy().timeout(DEADLINE).build()?;
        let gateway = Gateway::start(&registry).await?;
        let observed = async {
            let health = async || {
                Ok::<_, anyhow::Error>(http.get(gateway.url("/healthz")).send().await?.status())
            };
            ensure!(
                health().await? == StatusCode::OK,
                "initial gateway unhealthy"
            );
            let first = ModelEvent::new("qa.sql05.retained");
            let second = ModelEvent::new("qa.sql05.retained");
            let pending = ModelEvent::new("qa.sql05.pending");
            let events = [&first, &second];
            let expected = json!({"stored": [
                {"id": first.id, "seq": before_a.0.3 + 1},
                {"id": second.id, "seq": before_a.0.3 + 2}
            ]});
            let receipt = post_batch(&http, &gateway, RUNNER_A, &events).await?;
            ensure!(
                receipt == (StatusCode::OK, expected),
                "initial receipt differs"
            );
            let committed_a = batch_snapshot(&mut conn_a).await?;
            ensure!(
                committed_a.1.starts_with(&before_a.1)
                    && committed_a.1.len() == before_a.1.len() + 2
                    && committed_a.0.3 == before_a.0.3 + 2,
                "initial batch did not extend the prepared history"
            );

            set_link(&link, false).await?;
            let refusal = post_batch(&http, &gateway, RUNNER_A, &[&pending]).await?;
            ensure!(
                refusal.0 == StatusCode::SERVICE_UNAVAILABLE
                    && refusal.1.get("stored").is_none()
                    && health().await? == StatusCode::SERVICE_UNAVAILABLE,
                "outage did not refuse the write and expose degraded health"
            );
            ensure!(
                batch_snapshot(&mut conn_a).await? == committed_a
                    && batch_snapshot(&mut conn_b).await? == witness_b,
                "the refused write changed an audit"
            );

            set_link(&link, true).await?;
            let replay = post_batch(&http, &gateway, RUNNER_A, &events).await?;
            ensure!(
                replay == receipt,
                "reconnection changed the durable receipt"
            );
            ensure!(
                health().await? == StatusCode::OK
                    && batch_snapshot(&mut conn_a).await? == committed_a,
                "replay did not recover health while preserving the same audit"
            );

            let appended = post_batch(&http, &gateway, RUNNER_A, &[&pending]).await?;
            let next = committed_a.0.3 + 1;
            ensure!(
                appended
                    == (
                        StatusCode::OK,
                        json!({"stored": [{"id": pending.id, "seq": next}]})
                    ),
                "the refused event did not receive the next sequence after reconnection"
            );
            let after_a = batch_snapshot(&mut conn_a).await?;
            let [row] = after_a
                .1
                .strip_prefix(committed_a.1.as_slice())
                .context("reconnection changed the existing audit history")?
            else {
                anyhow::bail!("recovered write added an unexpected number of rows");
            };
            ensure!(
                row.0 == next
                    && row.1.as_slice() == pending.id.as_bytes()
                    && row.3.as_slice() == committed_a.0.4.as_bytes()
                    && after_a.0.3 == next
                    && after_a.0.4.as_bytes() == row.4.as_slice()
                    && batch_snapshot(&mut conn_b).await? == witness_b,
                "recovered write did not extend only the configured target"
            );
            Ok(())
        }
        .await;
        let stopped = gateway.stop().await;
        drop(link);
        observed.and(stopped)
    };
    let (closed, observed) = tokio::join!(serve_link(listener, target, changes), exercise);
    observed?;
    closed?;
    println!("SQL05_STORAGE_OUTAGE_REPLAY_PASS");
    println!("SQL05_BACKUP_RESTORE_NOT_RUN");
    Ok(())
}
