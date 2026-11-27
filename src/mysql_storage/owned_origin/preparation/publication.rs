//! Durable publication of a fresh owned MySQL source under the physical lock.
//!
//! The empty target is installed first (see `audit_tables`). Publication then
//! poses, in one bounded InnoDB write transaction on the locked connection, the
//! audit genesis, the fresh creation anchor and the unique singleton sentinel.
//! The sentinel stays absent until this transaction; an interrupted publication
//! leaves a partial target that the next attempt refuses. After COMMIT the same
//! locked connection re-reads the published prefix in a new snapshot before any
//! receipt, and the open guard repeats that verification before admission.

use super::super::fresh_anchor::{FreshAnchorDraft, FreshAnchorRequestV1};
use super::super::{OwnedAuditLock, decode_row, driver_error};
use super::{CheckedEmptyTarget, audit_manifest};
use crate::audit::event_codec::prepare_event;
use crate::model::Event;
use crate::mysql_storage::audit::{
    AuditHead, EventColumns, decode_event_columns, owned_genesis_event, require_owned_genesis,
};
use crate::redaction::Redactor;
use anyhow::{Context, Result, anyhow, ensure};
use mysql_async::{Conn, IsolationLevel, Row, Transaction, TxOpts, prelude::Queryable};
use uuid::Uuid;

/// The fresh published prefix is exactly the genesis and the creation anchor.
const PUBLISHED_EVENTS: i64 = 2;

/// Publish the genesis, the fresh anchor and the singleton on the locked
/// connection that already proved the empty manifest. The caller commits only
/// after the in-transaction re-read; a failed check rolls back without effect.
pub(super) async fn publish(
    target: CheckedEmptyTarget<'_>,
    version: u16,
    draft: &FreshAnchorDraft<'_>,
    tenant: &str,
    namespace: Uuid,
    redactor: &Redactor,
) -> Result<AuditHead> {
    let CheckedEmptyTarget { conn, lock } = target;
    ensure!(
        matches!(version, 14 | 15),
        "invalid MySQL owned audit format"
    );
    let request = draft.request();
    ensure!(
        request.schema_version == version
            && request.tenant_id == tenant
            && request.namespace_id == namespace,
        "MySQL fresh anchor request differs from the selected publication"
    );
    let namespace = namespace.to_string();

    let mut options = TxOpts::new();
    options
        .with_readonly(false)
        .with_isolation_level(IsolationLevel::RepeatableRead);
    let mut tx = conn
        .start_transaction(options)
        .await
        .map_err(driver_error)?;
    let prepared = async {
        audit_manifest::verify_empty_in_transaction(&mut tx, lock, version).await?;
        require_durable_writable(&mut tx).await?;
        let mut head = AuditHead::empty();

        let genesis = owned_genesis_event(tenant, &namespace);
        let original = genesis.data.clone();
        let (genesis, payload) = prepare_event(redactor, genesis)
            .map_err(|_| anyhow!("invalid MySQL audit genesis"))?;
        ensure!(
            genesis.data == original,
            "MySQL audit genesis would change under redaction"
        );
        require_owned_genesis(&genesis, tenant, &namespace)?;
        insert_event(&mut tx, &mut head, &genesis, &payload).await?;

        let (anchor, payload) = prepare_event(redactor, draft.event().clone())
            .map_err(|_| anyhow!("invalid MySQL fresh anchor event"))?;
        ensure!(
            serde_json::to_value(&anchor)? == serde_json::to_value(draft.event())?,
            "MySQL fresh anchor would change under redaction"
        );
        insert_event(&mut tx, &mut head, &anchor, &payload).await?;

        // The singleton is the last insert: before it the target stays closed.
        tx.exec_drop(
            "INSERT INTO ai_guard_audit_state \
             (singleton, schema_version, tenant_id, namespace_id, head_seq, head_hash, payload_bytes) \
             VALUES (1, ?, ?, ?, ?, ?, ?)",
            (version, tenant, &namespace, head.seq, &head.hash, head.payload_bytes),
        )
        .await
        .map_err(driver_error)?;

        let durable = read_published_binding(&mut tx, version, tenant, &namespace).await?;
        ensure!(
            durable == head,
            "MySQL owned publication head differs from prepared events"
        );
        replay_published_prefix(&mut tx, &durable, draft).await?;
        Ok::<_, anyhow::Error>(head)
    }
    .await;
    match prepared {
        Ok(head) => {
            tx.commit().await.map_err(driver_error)?;
            Ok(head)
        }
        Err(error) => {
            tx.rollback().await.map_err(driver_error)?;
            Err(error)
        }
    }
}

/// Verify the same fresh publication under the physical lock in one read-only
/// snapshot. A target with subsequent history or projections is outside this
/// fresh-publication receipt path.
pub(super) async fn verify_published(
    conn: &mut Conn,
    lock: &OwnedAuditLock,
    draft: &FreshAnchorDraft<'_>,
) -> Result<AuditHead> {
    let request = draft.request();
    let version = request.schema_version;
    let namespace = request.namespace_id.to_string();
    let mut options = TxOpts::new();
    options
        .with_readonly(true)
        .with_isolation_level(IsolationLevel::RepeatableRead);
    let mut tx = conn
        .start_transaction(options)
        .await
        .map_err(driver_error)?;
    let verified = async {
        audit_manifest::verify_structure(&mut tx, lock, version).await?;
        let head = read_published_binding(&mut tx, version, &request.tenant_id, &namespace).await?;
        replay_published_prefix(&mut tx, &head, draft).await?;
        audit_manifest::require_empty_projections(&mut tx, version).await?;
        lock.verify(&mut tx).await?;
        Ok::<_, anyhow::Error>(head)
    }
    .await;
    tx.rollback().await.map_err(driver_error)?;
    verified
}

async fn require_durable_writable(tx: &mut Transaction<'_>) -> Result<()> {
    let row = tx
        .query_first::<Row, _>(
            "SELECT @@GLOBAL.innodb_flush_log_at_trx_commit, @@GLOBAL.read_only, \
             @@GLOBAL.super_read_only, @@SESSION.foreign_key_checks, @@SESSION.unique_checks",
        )
        .await
        .map_err(driver_error)?
        .context("MySQL owned durability settings unavailable")?;
    ensure!(
        decode_row::<(u64, u8, u8, u8, u8)>(row)? == (1, 0, 0, 1, 1),
        "MySQL owned publication requires writable durable InnoDB"
    );
    Ok(())
}

async fn insert_event(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    event: &Event,
    payload: &str,
) -> Result<()> {
    let received = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let next = head.extended(&received, payload)?;
    let prev_hash = std::mem::replace(head, next).hash;
    tx.exec_drop(
        "INSERT INTO ai_guard_audit_event (seq,event_id,received_at,prev_hash,hash,payload) \
         VALUES (?,?,?,?,?,?)",
        (
            head.seq, &event.id, &received, &prev_hash, &head.hash, payload,
        ),
    )
    .await
    .map_err(driver_error)?;
    Ok(())
}

/// Read the unique singleton and bind it to the configured format and tenant.
async fn read_published_binding(
    tx: &mut Transaction<'_>,
    version: u16,
    tenant: &str,
    namespace: &str,
) -> Result<AuditHead> {
    let row = tx
        .query_first::<Row, _>(
            "SELECT schema_version,tenant_id,namespace_id,head_seq,head_hash,payload_bytes \
             FROM ai_guard_audit_state WHERE singleton=1",
        )
        .await
        .map_err(driver_error)?
        .context("MySQL owned publication singleton is absent")?;
    let (found, bound_tenant, bound_namespace, seq, hash, payload_bytes): (
        u16,
        Option<String>,
        Option<String>,
        i64,
        String,
        u64,
    ) = decode_row(row)?;
    ensure!(
        found == version
            && bound_tenant.as_deref() == Some(tenant)
            && bound_namespace.as_deref() == Some(namespace),
        "MySQL owned publication binding mismatch"
    );
    ensure!(seq > 0, "MySQL owned publication head is empty");
    crate::control::hash(&hash)?;
    Ok(AuditHead {
        seq,
        hash,
        payload_bytes,
    })
}

/// Replay the closed genesis+anchor prefix and recompute the durable head from
/// the exact stored payload bytes, never a re-serialization.
async fn replay_published_prefix(
    tx: &mut Transaction<'_>,
    bound: &AuditHead,
    draft: &FreshAnchorDraft<'_>,
) -> Result<()> {
    ensure!(
        bound.seq == PUBLISHED_EVENTS,
        "MySQL owned publication prefix is not the fresh source"
    );
    let rows: Vec<Row> = tx
        .query(
            "SELECT seq,event_id,received_at,prev_hash,hash,payload \
             FROM ai_guard_audit_event ORDER BY seq LIMIT 3",
        )
        .await
        .map_err(driver_error)?;
    ensure!(
        rows.len() as i64 == PUBLISHED_EVENTS,
        "MySQL owned publication chain length mismatch"
    );
    let request = draft.request();
    let namespace = request.namespace_id.to_string();
    let mut head = AuditHead::empty();
    for row in rows {
        let columns: EventColumns = decode_row(row)?;
        let encoded = decode_event_columns(columns)?;
        let stored = &encoded.stored;
        ensure!(
            head.hash == stored.prev_hash,
            "MySQL audit previous hash mismatch"
        );
        let next = head.extended(&stored.received_at, &encoded.payload)?;
        ensure!(
            next.seq == stored.seq && next.hash == stored.hash,
            "MySQL audit chain mismatch"
        );
        head = next;
        if head.seq == 1 {
            require_owned_genesis(&stored.event, &request.tenant_id, &namespace)?;
        } else {
            FreshAnchorRequestV1::verify_stored_origin(
                &stored.event,
                &request.tenant_id,
                &namespace,
                request.schema_version,
                request.server_uuid,
                &request.schema,
            )?;
            ensure!(
                draft.matches_recorded_fact(&encoded)?,
                "MySQL fresh anchor differs from the requested operation"
            );
        }
    }
    ensure!(head == *bound, "MySQL audit head mismatch");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::event_codec::digest;

    #[test]
    fn owned_genesis_binds_the_configured_tenant_and_namespace() {
        let event = owned_genesis_event("tenant-qa", "abcde000-0000-0000-0000-000000000001");
        assert!(
            require_owned_genesis(&event, "tenant-qa", "abcde000-0000-0000-0000-000000000001")
                .is_ok()
        );
        assert!(
            require_owned_genesis(
                &event,
                "tenant-other",
                "abcde000-0000-0000-0000-000000000001"
            )
            .is_err()
        );
        assert!(
            require_owned_genesis(&event, "tenant-qa", "abcde000-0000-0000-0000-000000000002")
                .is_err()
        );
        assert_eq!(event.kind, "control.mysql.audit.genesis");
        assert_eq!(event.source, "mysql-storage");
    }

    #[test]
    fn owned_publication_head_extends_the_closed_chain() {
        let empty = AuditHead::empty();
        assert_eq!(empty.seq, 0);
        assert_eq!(empty.hash, "0".repeat(64));
        let payload = "{\"kind\":\"control.mysql.audit.genesis\"}";
        let received = "2026-11-25T00:00:00.000Z";
        let first = empty.extended(received, payload).unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(first.hash, digest(1, received, &empty.hash, payload));
        assert_eq!(first.payload_bytes, payload.len() as u64);
        let second = first.extended(received, payload).unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.hash, digest(2, received, &first.hash, payload));
        assert_eq!(second.payload_bytes, 2 * payload.len() as u64);
    }

    #[test]
    fn owned_publication_prefix_is_exactly_the_fresh_source() {
        assert_eq!(PUBLISHED_EVENTS, 2);
    }
}
