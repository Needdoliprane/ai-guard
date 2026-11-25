//! Fresh-source request and replay checks for the private MySQL preparer.

use super::{OwnedAuditLock, decode_row, driver_error};
use crate::{
    audit::event_codec::{digest, prepare_event},
    broker_registry::{format_time, parse_time},
    control::{content_hash, hash},
    human_authority::HumanAuthorityMarkerReceiptV1,
    model::Event,
    mysql_storage::audit::{EventColumns, decode_event_columns},
    redaction::Redactor,
    tenancy::validate_tenant_id,
};
use anyhow::{Result, anyhow, ensure};
use mysql_async::{Conn, IsolationLevel, Row, Transaction, TxOpts, prelude::Queryable};
use serde::Serialize;
use uuid::Uuid;

const KIND: &str = "control.mysql.owned.source_anchor";
const REQUEST_HASH_DOMAIN: &str = "ai-guard:mysql-owned:fresh-anchor-request:v1";
const REPLAY_CONFLICT: &str = "MySQL fresh anchor differs from the requested operation";

/// Expected operator input. Physical observations and receipts are kept out of it.
#[derive(Clone, Serialize)]
pub(super) struct FreshAnchorRequestV1 {
    pub(super) operation_id: Uuid,
    pub(super) schema_version: u16,
    pub(super) tenant_id: String,
    pub(super) namespace_id: Uuid,
    pub(super) installation_id: Uuid,
    pub(super) source_id: Uuid,
    pub(super) server_uuid: Uuid,
    pub(super) schema: String,
}

#[derive(Serialize)]
struct FreshAnchorFactV1<'a> {
    version: u8,
    request: &'a FreshAnchorRequestV1,
    request_hash: String,
}

/// A validated draft. Only an observed row can yield a receipt.
pub(super) struct PlannedAnchor<'a> {
    request: &'a FreshAnchorRequestV1,
    event: Event,
}

impl FreshAnchorRequestV1 {
    pub(super) fn plan(
        &self,
        version: u16,
        server: Uuid,
        schema: &str,
        redactor: &Redactor,
    ) -> Result<PlannedAnchor<'_>> {
        validate_tenant_id(&self.tenant_id)?;
        ensure!(
            request_shape_is_valid(self.schema_version, &self.schema)
                && [
                    self.operation_id,
                    self.namespace_id,
                    self.installation_id,
                    self.source_id,
                    self.server_uuid
                ]
                .into_iter()
                .all(|id| !id.is_nil()),
            "invalid MySQL fresh anchor request"
        );
        ensure!(
            self.schema_version == version && self.server_uuid == server && self.schema == schema,
            "MySQL fresh anchor request differs from the selected target"
        );
        let mut event = Event::new(KIND);
        event.id = self.operation_id.to_string();
        event.source = "mysql-storage".into();
        event.data = serde_json::to_value(FreshAnchorFactV1 {
            version: 1,
            request: self,
            request_hash: content_hash(REQUEST_HASH_DOMAIN, self)?,
        })?;
        let expected = serde_json::to_value(&event)?;
        let (event, _) = prepare_event(redactor, event)
            .map_err(|_| anyhow!("invalid MySQL fresh anchor event"))?;
        ensure!(
            serde_json::to_value(&event)? == expected,
            "MySQL fresh anchor would change under redaction"
        );
        Ok(PlannedAnchor {
            request: self,
            event,
        })
    }
}

impl PlannedAnchor<'_> {
    /// Inspect a possible replay diagnostically. The caller still refuses to
    /// prepare a nonempty target, even on an exact match.
    pub(super) async fn reject_divergence(
        &self,
        conn: &mut Conn,
        lock: &OwnedAuditLock,
    ) -> Result<()> {
        ensure!(
            lock.server == self.request.server_uuid && lock.schema == self.request.schema,
            "MySQL fresh anchor request differs from the locked target"
        );
        let mut options = TxOpts::new();
        options
            .with_readonly(true)
            .with_isolation_level(IsolationLevel::RepeatableRead);
        let mut tx = conn
            .start_transaction(options)
            .await
            .map_err(driver_error)?;
        let replay = async {
            lock.verify_preparation_target(&mut tx).await?;
            if let Some((row, predecessor)) = self.recorded_row(&mut tx).await? {
                self.verify_recorded(row, predecessor.as_deref())?;
            }
            lock.verify_preparation_target(&mut tx).await
        }
        .await;
        tx.rollback().await.map_err(driver_error)?;
        replay
    }

    async fn recorded_row(
        &self,
        tx: &mut Transaction<'_>,
    ) -> Result<Option<(EventColumns, Option<String>)>> {
        let table = tx.query_first::<u8, _>(
            "SELECT EXISTS (SELECT 1 FROM information_schema.TABLES \
             WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() AND BINARY TABLE_NAME = 'ai_guard_audit_event' \
             AND TABLE_TYPE = 'BASE TABLE' AND ENGINE = 'InnoDB')",
        ).await.map_err(driver_error)?;
        if table != Some(1) {
            return Ok(None);
        }
        let mut rows: Vec<Row> = tx
            .exec(
                "SELECT e.seq,e.event_id,e.received_at,e.prev_hash,e.hash, \
                    IF(OCTET_LENGTH(e.payload) <= 131072,e.payload,NULL),p.hash \
             FROM ai_guard_audit_event e LEFT JOIN ai_guard_audit_event p ON p.seq=e.seq-1 \
             WHERE e.event_id=? AND BINARY e.event_id=BINARY ? LIMIT 2",
                (&self.event.id, &self.event.id),
            )
            .await
            .map_err(driver_error)?;
        ensure!(rows.len() <= 1, REPLAY_CONFLICT);
        rows.pop()
            .map(|row| {
                let (seq, id, received, prev, hash, payload, predecessor): (
                    i64,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                ) = decode_row(row).map_err(|_| anyhow!(REPLAY_CONFLICT))?;
                Ok(((seq, id, received, prev, hash, payload), predecessor))
            })
            .transpose()
    }

    fn verify_recorded(
        &self,
        row: EventColumns,
        predecessor: Option<&str>,
    ) -> Result<HumanAuthorityMarkerReceiptV1> {
        ensure!(row.5.len() <= 131_072, REPLAY_CONFLICT);
        let raw = row.5.clone();
        let computed = digest(row.0, &row.2, &row.3, &row.5);
        let stored = decode_event_columns(row)
            .map_err(|_| anyhow!(REPLAY_CONFLICT))?
            .stored;
        let time = parse_time(&stored.event.timestamp).map_err(|_| anyhow!(REPLAY_CONFLICT))?;
        let received = parse_time(&stored.received_at).map_err(|_| anyhow!(REPLAY_CONFLICT))?;
        let ordered = time <= received;
        let mut expected = self.event.clone();
        expected.timestamp = format_time(time);
        // Compare the actual hashed bytes with the codec's closed event form.
        // This also excludes duplicate keys and extensions in the stored JSON.
        let exact_fact = raw == serde_json::to_string(&expected)?;
        ensure!(
            ordered
                && hash(&stored.prev_hash).is_ok()
                && receipt_matches(
                    stored.seq,
                    &stored.prev_hash,
                    predecessor,
                    &stored.hash,
                    &computed,
                    exact_fact
                ),
            REPLAY_CONFLICT
        );
        Ok(HumanAuthorityMarkerReceiptV1 {
            seq: stored.seq,
            hash: stored.hash,
        })
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn request() -> FreshAnchorRequestV1 {
        FreshAnchorRequestV1 {
            operation_id: Uuid::from_u128(1),
            schema_version: 14,
            tenant_id: "tenant_a".into(),
            namespace_id: Uuid::from_u128(2),
            installation_id: Uuid::from_u128(3),
            source_id: Uuid::from_u128(4),
            server_uuid: Uuid::from_u128(5),
            schema: "qa_owned".into(),
        }
    }

    fn plan(request: &FreshAnchorRequestV1) -> Result<PlannedAnchor<'_>> {
        request.plan(
            request.schema_version,
            request.server_uuid,
            &request.schema,
            &Redactor::new(Vec::new()),
        )
    }

    fn encoded_row(anchor: &PlannedAnchor<'_>) -> Result<EventColumns> {
        let mut event = anchor.event.clone();
        event.timestamp = "2026-11-25T00:00:00.000Z".into();
        let (event, payload) = prepare_event(&Redactor::new(Vec::new()), event)?;
        let received = "2026-11-25T00:00:01.000Z";
        let previous = "a".repeat(64);
        let hash = digest(2, received, &previous, &payload);
        Ok((2, event.id, received.into(), previous, hash, payload))
    }

    #[test]
    fn fresh_anchor_fact_is_closed_and_refuses_redacted_identity() -> Result<()> {
        let request = request();
        let anchor = plan(&request)?;
        assert_eq!(anchor.event.id, request.operation_id.to_string());
        assert_eq!(anchor.event.data["version"], 1);
        assert_eq!(
            anchor.event.data["request_hash"],
            "38f1caaf3b0480b1c7c42b051a0cc7be25e4e0fb008a1d109a21736ba570f996"
        );
        assert_eq!(
            anchor.event.data["request"],
            serde_json::to_value(&request)?
        );
        assert!(
            request
                .plan(
                    15,
                    request.server_uuid,
                    &request.schema,
                    &Redactor::new(Vec::new())
                )
                .is_err()
        );
        assert!(
            request
                .plan(
                    14,
                    request.server_uuid,
                    &request.schema,
                    &Redactor::new(vec![request.schema.clone()])
                )
                .is_err()
        );
        let mut invalid = request.clone();
        invalid.namespace_id = Uuid::nil();
        assert!(plan(&invalid).is_err());

        let mut row = encoded_row(&anchor)?;
        let mut event: serde_json::Value = serde_json::from_str(&row.5)?;
        event["data"]["previous_format"] = 13.into();
        row.5 = serde_json::to_string(&event)?;
        row.4 = digest(row.0, &row.2, &row.3, &row.5);
        assert!(anchor.verify_recorded(row.clone(), Some(&row.3)).is_err());
        Ok(())
    }

    #[test]
    fn fresh_anchor_replays_only_the_same_operation_and_canonical_request() -> Result<()> {
        let original = request();
        let row = encoded_row(&plan(&original)?)?;
        let receipt = plan(&original)?.verify_recorded(row.clone(), Some(&row.3))?;
        assert_eq!((receipt.seq, receipt.hash), (row.0, row.4.clone()));
        for changed in [
            FreshAnchorRequestV1 {
                schema_version: 15,
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                tenant_id: "tenant_b".into(),
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                namespace_id: Uuid::from_u128(6),
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                installation_id: Uuid::from_u128(6),
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                source_id: Uuid::from_u128(6),
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                server_uuid: Uuid::from_u128(6),
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                schema: "qa_other".into(),
                ..original.clone()
            },
            FreshAnchorRequestV1 {
                operation_id: Uuid::from_u128(6),
                ..original.clone()
            },
        ] {
            assert!(
                plan(&changed)?
                    .verify_recorded(row.clone(), Some(&row.3))
                    .is_err()
            );
        }
        Ok(())
    }
}

fn request_shape_is_valid(version: u16, schema: &str) -> bool {
    matches!(version, 14 | 15)
        && !schema.is_empty()
        && schema.chars().count() <= 64
        && !schema.chars().any(char::is_control)
}

fn receipt_matches(
    sequence: i64,
    previous: &str,
    predecessor: Option<&str>,
    hash: &str,
    digest: &str,
    exact_fact: bool,
) -> bool {
    sequence > 1 && predecessor == Some(previous) && hash == digest && exact_fact
}

#[cfg(test)]
mod replay_tests {
    use super::{receipt_matches, request_shape_is_valid};

    #[test]
    fn fresh_anchor_shape_is_bounded_and_format_closed() {
        assert!(request_shape_is_valid(14, "qa_owned"));
        assert!(request_shape_is_valid(15, "qa_owned"));
        assert!(!request_shape_is_valid(1, "qa_owned"));
        assert!(!request_shape_is_valid(16, "qa_owned"));
        assert!(!request_shape_is_valid(14, ""));
        assert!(!request_shape_is_valid(14, "qa\nowned"));
        assert!(!request_shape_is_valid(14, &"x".repeat(65)));
    }

    #[test]
    fn fresh_anchor_receipt_rejects_divergent_fact_and_chain_edge() {
        assert!(receipt_matches(
            2,
            "previous",
            Some("previous"),
            "actual",
            "actual",
            true
        ));
        assert!(!receipt_matches(
            2,
            "previous",
            Some("previous"),
            "actual",
            "actual",
            false
        ));
        assert!(!receipt_matches(
            2,
            "previous",
            Some("different"),
            "actual",
            "actual",
            true
        ));
        assert!(!receipt_matches(
            2, "previous", None, "actual", "actual", true
        ));
        assert!(!receipt_matches(
            1,
            "previous",
            Some("previous"),
            "actual",
            "actual",
            true
        ));
        assert!(!receipt_matches(
            2,
            "previous",
            Some("previous"),
            "actual",
            "different",
            true
        ));
    }
}
