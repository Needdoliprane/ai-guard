use agentsandbox::{
    model::Event,
    mysql_storage::{MysqlProbeError, audit::MysqlAuditStore},
};
use mysql_async::{Conn, prelude::Queryable};
use serde_json::json;
use std::{net::Ipv4Addr, time::Duration};
use tokio::{net::TcpListener, time::timeout};

const FIXTURE_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) async fn exercise(
    bootstrapper: &MysqlAuditStore,
    writer: &MysqlAuditStore,
    mut inspector: Conn,
    transport_fixture: impl FnOnce(u16) -> anyhow::Result<MysqlAuditStore>,
) -> anyhow::Result<()> {
    let boundary = bootstrapper.initialize_supervised().await?;
    let event = writer.append(Event::new("qa.mysql.integrity")).await?;
    let verified = writer.integrity().await?;
    let expected = json!({
        "valid": true,
        "event_count": event.seq,
        "head_hash": event.hash,
        "error": null,
    });
    assert_eq!(serde_json::to_value(verified)?, expected);

    // The owned loopback socket closes before sending a MySQL greeting.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let unavailable = transport_fixture(listener.local_addr()?.port())?;
    let (check, accepted) = tokio::join!(
        unavailable.integrity(),
        timeout(FIXTURE_TIMEOUT, async {
            let (socket, _) = listener.accept().await?;
            drop(socket);
            Ok::<_, std::io::Error>(())
        }),
    );
    accepted??;
    assert_eq!(
        check.unwrap_err().downcast_ref::<MysqlProbeError>(),
        Some(&MysqlProbeError::TransportUnavailable)
    );

    // Alter one row in the disposable schema while retaining its declared
    // head and the original raw prefix for comparison.
    let original = super::fixture_prefix(&mut inspector, event.seq).await?;
    let invalid_hash = if event.hash == "a".repeat(64) {
        "b".repeat(64)
    } else {
        "a".repeat(64)
    };
    replace_hash(&mut inspector, event.seq, invalid_hash.as_str()).await?;
    let broken = super::fixture_prefix(&mut inspector, event.seq).await?;
    assert_ne!(broken, original);
    assert_eq!(
        serde_json::to_value(writer.integrity().await?)?,
        json!({
            "valid": false,
            "event_count": boundary.seq,
            "head_hash": boundary.hash,
            "error": format!("integrity violation at sequence {}", event.seq),
        }),
    );
    assert_eq!(
        writer.verify().await.unwrap_err().to_string(),
        "MySQL audit chain mismatch"
    );
    assert_eq!(
        super::fixture_prefix(&mut inspector, event.seq).await?,
        broken
    );
    replace_hash(&mut inspector, event.seq, &event.hash).await?;
    assert_eq!(
        super::fixture_prefix(&mut inspector, event.seq).await?,
        original
    );
    assert_eq!(serde_json::to_value(writer.integrity().await?)?, expected);
    Ok(())
}

pub(super) async fn replace_hash(inspector: &mut Conn, seq: i64, hash: &str) -> anyhow::Result<()> {
    timeout(
        FIXTURE_TIMEOUT,
        inspector.exec_drop(
            "UPDATE ai_guard_audit_event SET hash=? WHERE seq=?",
            (hash, seq),
        ),
    )
    .await?
    .map_err(|_| anyhow::anyhow!("fixture event hash update failed"))?;
    assert_eq!(inspector.affected_rows(), 1);
    Ok(())
}
