//! Durable publication of a fresh owned MySQL source under the physical lock.
//!
//! The empty target is installed first (see `audit_tables`). Publication then
//! poses, in one bounded InnoDB write transaction on the locked connection, the
//! audit genesis, the fresh creation anchor and the unique singleton sentinel.
//! The sentinel stays absent until this transaction; an interrupted publication
//! leaves a partial target that the next attempt refuses. After COMMIT the same
//! locked connection re-reads the published prefix in a new snapshot before any
//! receipt. Opening an existing source also requires verification of any later
//! history.

use super::super::fresh_anchor::{FreshAnchorDraft, FreshAnchorRequestV1};
use super::super::{OwnedAuditLock, decode_row, driver_error};
use super::{CheckedEmptyTarget, audit_manifest};
use crate::audit::event_codec::prepare_event;
use crate::model::Event;
use crate::mysql_storage::audit::{
    AuditChainMismatch, AuditHead, Binding, EncodedEvent, EventColumns, ExpectedAuditBinding,
    decode_event_columns, owned_genesis_event, require_owned_genesis,
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
    draft.require_locked_target(lock)?;
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
    verify_snapshot(conn, lock, draft, AuditRange::Fresh).await
}

/// Inspect the anchored audit chain through its durable head. The audit
/// projections and the current writer epoch require their own verification.
pub(super) async fn verify_anchored_chain(
    conn: &mut Conn,
    lock: &OwnedAuditLock,
    draft: &FreshAnchorDraft<'_>,
) -> Result<AuditHead> {
    verify_snapshot(conn, lock, draft, AuditRange::History).await
}

enum AuditRange {
    Fresh,
    History,
}

async fn verify_snapshot(
    conn: &mut Conn,
    lock: &OwnedAuditLock,
    draft: &FreshAnchorDraft<'_>,
    range: AuditRange,
) -> Result<AuditHead> {
    draft.require_locked_target(lock)?;
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
        if matches!(range, AuditRange::History) {
            audit_manifest::pin_history_tables(&mut tx, version).await?;
        }
        audit_manifest::verify_structure(&mut tx, lock, version).await?;
        let head = read_published_binding(&mut tx, version, &request.tenant_id, &namespace).await?;
        match range {
            AuditRange::Fresh => {
                replay_published_prefix(&mut tx, &head, draft).await?;
                audit_manifest::require_empty_projections(&mut tx, version).await?;
            }
            AuditRange::History => replay_anchored_chain(&mut tx, &head, draft).await?,
        }
        lock.verify(&mut tx).await?;
        Ok::<_, anyhow::Error>(head)
    }
    .await;
    tx.rollback().await.map_err(driver_error)?;
    verified
}

async fn replay_anchored_chain(
    tx: &mut Transaction<'_>,
    bound: &AuditHead,
    draft: &FreshAnchorDraft<'_>,
) -> Result<()> {
    ensure!(
        bound.seq >= PUBLISHED_EVENTS,
        "MySQL owned publication chain length mismatch"
    );
    // One extra row suffices to detect any suffix beyond the declared head.
    // The limit also bounds the cursor drain when a check fails early.
    let limit = u64::try_from(bound.seq)? + 1;
    let mut result = tx
        .exec_iter(
            "SELECT seq,event_id,received_at,prev_hash,hash,payload \
             FROM ai_guard_audit_event ORDER BY seq LIMIT ?",
            (limit,),
        )
        .await
        .map_err(driver_error)?;
    let replayed = async {
        let missing_origin = "MySQL owned publication chain length mismatch";
        let genesis = result
            .next()
            .await
            .map_err(driver_error)?
            .context(missing_origin)?;
        let anchor = result
            .next()
            .await
            .map_err(driver_error)?
            .context(missing_origin)?;
        let mut head =
            verify_owned_origin_prefix([decode_row(genesis)?, decode_row(anchor)?], draft)?;
        while let Some(row) = result.next().await.map_err(driver_error)? {
            head = verify_owned_tail_row(head, decode_row(row)?, bound)?;
        }
        Ok::<_, anyhow::Error>(head)
    }
    .await;
    // Finish the cursor even on a row error, preserving that first failure.
    let drained = result.drop_result().await.map_err(driver_error);
    let head = replayed?;
    drained?;
    require_owned_head(&head, bound)
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
    let bound = Binding::decode(
        row,
        ExpectedAuditBinding::Owned {
            version,
            tenant,
            namespace,
        },
    )?;
    ensure!(bound.head.seq > 0, "MySQL owned publication head is empty");
    crate::control::hash(&bound.head.hash)?;
    Ok(bound.head)
}

/// Verify the entire history of a source that has just been published.
async fn replay_published_prefix(
    tx: &mut Transaction<'_>,
    bound: &AuditHead,
    draft: &FreshAnchorDraft<'_>,
) -> Result<()> {
    ensure!(
        bound.seq == PUBLISHED_EVENTS,
        "MySQL owned publication prefix is not the fresh source"
    );
    let mut result = tx
        .query_iter(
            "SELECT seq,event_id,received_at,prev_hash,hash,payload \
             FROM ai_guard_audit_event ORDER BY seq LIMIT 3",
        )
        .await
        .map_err(driver_error)?;
    let prefix = async {
        let Some(genesis) = result.next().await.map_err(driver_error)? else {
            return Ok(None);
        };
        let Some(anchor) = result.next().await.map_err(driver_error)? else {
            return Ok(None);
        };
        let has_suffix = result.next().await.map_err(driver_error)?.is_some();
        Ok::<_, anyhow::Error>((!has_suffix).then_some([genesis, anchor]))
    }
    .await;
    // Drain the cursor before decoding the retained rows or reusing the transaction.
    let drained = result.drop_result().await.map_err(driver_error);
    let prefix = prefix?;
    drained?;
    let Some(rows) = prefix else {
        return Err(anyhow!("MySQL owned publication chain length mismatch"));
    };
    verify_owned_history(
        rows.into_iter().map(decode_row::<EventColumns>),
        bound,
        draft,
    )
}

/// Verify ordered stored rows through the declared head. Only the origin
/// records and the current head are retained while replay advances.
fn verify_owned_history(
    rows: impl IntoIterator<Item = Result<EventColumns>>,
    bound: &AuditHead,
    draft: &FreshAnchorDraft<'_>,
) -> Result<()> {
    let mut rows = rows.into_iter();
    let missing_origin = "MySQL owned publication chain length mismatch";
    let genesis = rows.next().context(missing_origin)??;
    let anchor = rows.next().context(missing_origin)??;
    let mut head = verify_owned_origin_prefix([genesis, anchor], draft)?;
    for columns in rows {
        head = verify_owned_tail_row(head, columns?, bound)?;
    }
    require_owned_head(&head, bound)
}

fn verify_owned_tail_row(
    head: AuditHead,
    columns: EventColumns,
    bound: &AuditHead,
) -> Result<AuditHead> {
    ensure!(
        head.seq < bound.seq,
        "MySQL audit row follows the durable head"
    );
    verify_owned_link(&decode_event_columns(columns)?, head)
}

fn require_owned_head(head: &AuditHead, bound: &AuditHead) -> Result<()> {
    ensure!(head == bound, "MySQL audit head mismatch");
    Ok(())
}

fn verify_owned_link(encoded: &EncodedEvent, head: AuditHead) -> Result<AuditHead> {
    // Fresh preparation preserves its descriptive mismatch contract.
    encoded
        .verify_next_head(head)
        .map_err(|error| match error.downcast::<AuditChainMismatch>() {
            Ok(mismatch) => anyhow::Error::msg(mismatch.reason),
            Err(error) => error,
        })
}

/// Verify the two origin records against the expected draft and return their
/// head. The hash covers the exact stored payload bytes. A caller verifying
/// subsequent history must continue from this head.
fn verify_owned_origin_prefix(
    rows: [EventColumns; PUBLISHED_EVENTS as usize],
    draft: &FreshAnchorDraft<'_>,
) -> Result<AuditHead> {
    let request = draft.request();
    let namespace = request.namespace_id.to_string();
    let mut head = AuditHead::empty();
    for columns in rows {
        let encoded = decode_event_columns(columns)?;
        head = verify_owned_link(&encoded, head)?;
        let stored = &encoded.stored;
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
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::event_codec::digest;
    use crate::mysql_storage::owned_origin::fresh_anchor::ConfiguredTenantNamespace;

    fn origin_request(schema_version: u16) -> FreshAnchorRequestV1 {
        FreshAnchorRequestV1 {
            operation_id: Uuid::from_u128(1),
            schema_version,
            tenant_id: "tenant-qa".into(),
            namespace_id: Uuid::from_u128(2),
            installation_id: Uuid::from_u128(3),
            source_id: Uuid::from_u128(4),
            server_uuid: Uuid::from_u128(5),
            schema: "qa_owned".into(),
        }
    }

    fn origin_draft(request: &FreshAnchorRequestV1) -> Result<FreshAnchorDraft<'_>> {
        request.plan(
            request.schema_version,
            request.server_uuid,
            &request.schema,
            ConfiguredTenantNamespace {
                tenant_id: &request.tenant_id,
                namespace_id: request.namespace_id,
            },
            &Redactor::new(Vec::new()),
        )
    }

    fn encoded_origin_row(seq: i64, previous: String, mut event: Event) -> Result<EventColumns> {
        event.timestamp = "2026-11-25T00:00:00.000Z".into();
        let (event, payload) = prepare_event(&Redactor::new(Vec::new()), event)?;
        let received_at = "2026-11-25T00:00:01.000Z";
        let hash = digest(seq, received_at, &previous, &payload);
        Ok((seq, event.id, received_at.into(), previous, hash, payload))
    }

    fn origin_rows(draft: &FreshAnchorDraft<'_>) -> Result<[EventColumns; 2]> {
        let request = draft.request();
        let genesis = owned_genesis_event(&request.tenant_id, &request.namespace_id.to_string());
        let genesis = encoded_origin_row(1, "0".repeat(64), genesis)?;
        let anchor = encoded_origin_row(2, genesis.4.clone(), draft.event().clone())?;
        Ok([genesis, anchor])
    }

    #[test]
    fn owned_history_replay_requires_an_exact_tail_and_durable_head() -> Result<()> {
        let request = origin_request(14);
        let draft = origin_draft(&request)?;
        let [genesis, anchor] = origin_rows(&draft)?;
        let fresh_head = AuditHead {
            seq: 2,
            hash: anchor.4.clone(),
            payload_bytes: (genesis.5.len() + anchor.5.len()) as u64,
        };
        verify_owned_history(
            [genesis.clone(), anchor.clone()].map(Ok),
            &fresh_head,
            &draft,
        )?;

        let event = Event::new("qa.mysql.history");
        let appended = encoded_origin_row(3, anchor.4.clone(), event.clone())?;
        let head = AuditHead {
            seq: 3,
            hash: appended.4.clone(),
            payload_bytes: fresh_head.payload_bytes + appended.5.len() as u64,
        };
        let rows = [genesis.clone(), anchor.clone(), appended];
        verify_owned_history(rows.clone().map(Ok), &head, &draft)?;

        let other_request = FreshAnchorRequestV1 {
            installation_id: Uuid::from_u128(6),
            ..request.clone()
        };
        let other = origin_draft(&other_request)?;
        let other_anchor = encoded_origin_row(2, genesis.4.clone(), other.event().clone())?;
        let wrong_tail = encoded_origin_row(3, other_anchor.4, event)?;
        let wrong_head = AuditHead {
            hash: wrong_tail.4.clone(),
            ..head.clone()
        };
        let error =
            verify_owned_history([genesis, anchor, wrong_tail].map(Ok), &wrong_head, &draft)
                .expect_err("a suffix from another origin must be refused");
        assert_eq!(error.to_string(), "MySQL audit previous hash mismatch");

        let wrong_bytes = AuditHead {
            payload_bytes: head.payload_bytes + 1,
            ..head.clone()
        };
        let error = verify_owned_history(rows.clone().map(Ok), &wrong_bytes, &draft)
            .expect_err("the durable head must account for the exact payload bytes");
        assert_eq!(error.to_string(), "MySQL audit head mismatch");

        let error = verify_owned_history(rows.map(Ok), &fresh_head, &draft)
            .expect_err("a row beyond the declared durable head must be refused");
        assert_eq!(
            error.to_string(),
            "MySQL audit row follows the durable head"
        );
        Ok(())
    }

    #[test]
    fn owned_history_rejects_a_coherent_chain_for_another_installation() -> Result<()> {
        for schema_version in [14, 15] {
            let request = origin_request(schema_version);
            let foreign_request = FreshAnchorRequestV1 {
                installation_id: Uuid::from_u128(6),
                ..request.clone()
            };
            let foreign = origin_draft(&foreign_request)?;
            let [genesis, anchor] = origin_rows(&foreign)?;
            let tail = encoded_origin_row(3, anchor.4.clone(), Event::new("qa.mysql.history"))?;
            let head = AuditHead {
                seq: 3,
                hash: tail.4.clone(),
                payload_bytes: (genesis.5.len() + anchor.5.len() + tail.5.len()) as u64,
            };
            let rows = [genesis, anchor, tail];
            verify_owned_history(rows.clone().map(Ok), &head, &foreign)?;
            let error = verify_owned_history(rows.map(Ok), &head, &origin_draft(&request)?)
                .expect_err("the chain's integrity does not establish the requested installation");
            assert_eq!(
                error.to_string(),
                "MySQL fresh anchor differs from the requested operation"
            );
        }
        Ok(())
    }

    #[test]
    fn owned_origin_prefix_returns_the_verified_head_for_both_formats() -> Result<()> {
        for schema_version in [14, 15] {
            let request = origin_request(schema_version);
            let draft = origin_draft(&request)?;
            let rows = origin_rows(&draft)?;
            let expected = AuditHead {
                seq: 2,
                hash: rows[1].4.clone(),
                payload_bytes: (rows[0].5.len() + rows[1].5.len()) as u64,
            };
            assert_eq!(verify_owned_origin_prefix(rows, &draft)?, expected);
        }
        Ok(())
    }

    #[test]
    fn owned_origin_prefix_hashes_the_exact_stored_genesis_bytes() -> Result<()> {
        let request = origin_request(15);
        let draft = origin_draft(&request)?;
        let [mut genesis, _] = origin_rows(&draft)?;
        genesis.5.push('\n');
        genesis.4 = digest(genesis.0, &genesis.2, &genesis.3, &genesis.5);
        let anchor = encoded_origin_row(2, genesis.4.clone(), draft.event().clone())?;
        let expected = AuditHead {
            seq: 2,
            hash: anchor.4.clone(),
            payload_bytes: (genesis.5.len() + anchor.5.len()) as u64,
        };
        assert_eq!(
            verify_owned_origin_prefix([genesis, anchor], &draft)?,
            expected
        );
        Ok(())
    }

    #[test]
    fn owned_origin_prefix_rejects_another_format_or_installation() -> Result<()> {
        let request = origin_request(14);
        let published = origin_draft(&request)?;
        let rows = origin_rows(&published)?;
        let another_format = origin_request(15);
        let another_installation = FreshAnchorRequestV1 {
            installation_id: Uuid::from_u128(6),
            ..request.clone()
        };
        for expected in [another_format, another_installation] {
            assert!(verify_owned_origin_prefix(rows.clone(), &origin_draft(&expected)?).is_err());
        }
        Ok(())
    }

    #[test]
    fn owned_origin_prefix_rejects_a_disconnected_edge_or_changed_head() -> Result<()> {
        let request = origin_request(14);
        let draft = origin_draft(&request)?;
        let [genesis, anchor] = origin_rows(&draft)?;
        let disconnected = encoded_origin_row(2, "f".repeat(64), draft.event().clone())?;
        let mut changed_sequence = anchor.clone();
        changed_sequence.0 = 3;

        let mut changed_head = anchor;
        changed_head.4 = "f".repeat(64);
        for (row, message) in [
            (disconnected, "MySQL audit previous hash mismatch"),
            (changed_sequence, "MySQL audit chain mismatch"),
            (changed_head, "MySQL audit chain mismatch"),
        ] {
            let error = verify_owned_origin_prefix([genesis.clone(), row], &draft)
                .expect_err("divergent audit link must be refused");
            assert_eq!(error.to_string(), message);
            assert!(error.downcast_ref::<AuditChainMismatch>().is_none());
        }
        Ok(())
    }

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
