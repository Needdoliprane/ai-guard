//! Fresh-source request and replay checks for the private MySQL preparer.

use super::{OwnedAuditLock, decode_row, driver_error};
use crate::{
    audit::event_codec::{digest, prepare_event},
    broker_registry::{format_time, parse_time},
    control::{content_hash, hash},
    human_authority::HumanAuthorityMarkerReceiptV1,
    model::Event,
    mysql_storage::audit::{EncodedEvent, EventColumns, decode_event_columns},
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

/// The operator fixes the intended tenant, namespace, installation and source
/// for creation, and names the expected physical target. The installation and
/// source IDs are proposed assignments. The live lock attests the SQL target.
#[derive(Clone, Serialize, serde::Deserialize)]
pub(crate) struct FreshAnchorRequestV1 {
    pub(super) operation_id: Uuid,
    pub(super) schema_version: u16,
    pub(super) tenant_id: String,
    pub(super) namespace_id: Uuid,
    pub(super) installation_id: Uuid,
    pub(super) source_id: Uuid,
    pub(super) server_uuid: Uuid,
    pub(super) schema: String,
}

/// Tenant and namespace resolved from the selected storage configuration.
#[derive(Clone, Copy)]
pub(crate) struct ConfiguredTenantNamespace<'a> {
    pub(super) tenant_id: &'a str,
    pub(super) namespace_id: Uuid,
}

#[derive(Serialize)]
struct FreshAnchorFactV1<'a> {
    version: u8,
    request: &'a FreshAnchorRequestV1,
    request_hash: String,
}

/// A checked request and its draft event. A matching historical row yields a
/// row receipt. Complete source verification and publication remain separate.
pub(super) struct FreshAnchorDraft<'a> {
    request: &'a FreshAnchorRequestV1,
    event: Event,
}

impl FreshAnchorRequestV1 {
    pub(super) fn plan(
        &self,
        version: u16,
        server: Uuid,
        schema: &str,
        configured: ConfiguredTenantNamespace<'_>,
        redactor: &Redactor,
    ) -> Result<FreshAnchorDraft<'_>> {
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
        ensure!(
            self.tenant_id == configured.tenant_id && self.namespace_id == configured.namespace_id,
            "MySQL fresh anchor request differs from the configured tenant or namespace"
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
        Ok(FreshAnchorDraft {
            request: self,
            event,
        })
    }

    /// Verify the stored origin and request hash against the configured target.
    /// An exact operation replay must also compare the fact with its draft.
    pub(super) fn verify_stored_origin(
        event: &Event,
        tenant: &str,
        namespace: &str,
        version: u16,
        server: Uuid,
        schema: &str,
    ) -> Result<()> {
        ensure!(
            event.kind == KIND && event.source == "mysql-storage" && event.data["version"] == 1,
            "MySQL fresh anchor origin mismatch"
        );
        let request: FreshAnchorRequestV1 = serde_json::from_value(event.data["request"].clone())
            .map_err(|_| anyhow!(REPLAY_CONFLICT))?;
        ensure!(
            serde_json::to_value(&request)? == event.data["request"],
            REPLAY_CONFLICT
        );
        ensure!(
            request.tenant_id == tenant
                && request.namespace_id.to_string() == namespace
                && request.schema_version == version
                && request.server_uuid == server
                && request.schema == schema
                && !request.operation_id.is_nil()
                && !request.installation_id.is_nil()
                && !request.source_id.is_nil(),
            "MySQL fresh anchor origin mismatch"
        );
        ensure!(
            event.data["request_hash"].as_str()
                == Some(content_hash(REQUEST_HASH_DOMAIN, &request)?.as_str()),
            REPLAY_CONFLICT
        );
        Ok(())
    }
}

impl FreshAnchorDraft<'_> {
    /// The prepared, redaction-checked creation event ready for durable insert.
    pub(super) fn event(&self) -> &Event {
        &self.event
    }

    /// The canonical request bound to this draft.
    pub(super) fn request(&self) -> &FreshAnchorRequestV1 {
        self.request
    }

    /// Bind the drafted origin to the physical target recorded by the lock.
    /// The caller still verifies ownership on its exact SQL connection.
    pub(super) fn require_locked_target(&self, lock: &OwnedAuditLock) -> Result<()> {
        ensure!(
            lock.server == self.request.server_uuid && lock.schema == self.request.schema,
            "MySQL fresh anchor request differs from the locked target"
        );
        Ok(())
    }

    /// Check a historical row under the observed server, schema and session
    /// lock. The caller's refusal to prepare a nonempty target still applies.
    pub(super) async fn reject_divergence(
        &self,
        conn: &mut Conn,
        lock: &OwnedAuditLock,
    ) -> Result<()> {
        self.require_locked_target(lock)?;
        let mut options = TxOpts::new();
        options
            .with_readonly(true)
            .with_isolation_level(IsolationLevel::RepeatableRead);
        let mut tx = conn
            .start_transaction(options)
            .await
            .map_err(driver_error)?;
        let replay = async {
            lock.verify(&mut tx).await?;
            if let Some((row, predecessor)) = self.recorded_row(&mut tx).await? {
                self.verify_recorded(row, predecessor.as_deref())?;
            }
            lock.verify(&mut tx).await
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
        let operation_id = self.request.operation_id.to_string();
        let id = operation_id.as_bytes();
        // Keep the indexed lookup and make exact binary equality explicit.
        let mut rows: Vec<Row> = tx
            .exec(
                "SELECT e.seq,e.event_id,e.received_at,e.prev_hash,e.hash, \
                    IF(OCTET_LENGTH(e.payload) <= 131072,e.payload,NULL),p.hash \
             FROM ai_guard_audit_event e LEFT JOIN ai_guard_audit_event p ON p.seq=e.seq-1 \
             WHERE e.event_id=CAST(? AS BINARY) \
             AND CAST(e.event_id AS BINARY)=CAST(? AS BINARY) LIMIT 2",
                (id, id),
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
        let operation_id = self.request.operation_id.to_string();
        ensure!(
            recorded_row_matches(&operation_id, &row.1, row.5.len()),
            REPLAY_CONFLICT
        );
        let computed = digest(row.0, &row.2, &row.3, &row.5);
        let encoded = decode_event_columns(row).map_err(|_| anyhow!(REPLAY_CONFLICT))?;
        let exact_fact = self.matches_recorded_fact(&encoded)?;
        let stored = encoded.stored;
        ensure!(
            hash(&stored.prev_hash).is_ok()
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

    /// Match the exact draft against a row whose chain is checked by the caller.
    pub(super) fn matches_recorded_fact(&self, encoded: &EncodedEvent) -> Result<bool> {
        let stored = &encoded.stored;
        let time = parse_time(&stored.event.timestamp).map_err(|_| anyhow!(REPLAY_CONFLICT))?;
        let received = parse_time(&stored.received_at).map_err(|_| anyhow!(REPLAY_CONFLICT))?;
        let mut expected = self.event.clone();
        expected.timestamp = format_time(time);
        // Compare the hashed bytes, including the operation ID and the closed
        // request containing the installation and source IDs.
        Ok(time <= received && encoded.payload == serde_json::to_string(&expected)?)
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

    fn configured(request: &FreshAnchorRequestV1) -> ConfiguredTenantNamespace<'_> {
        ConfiguredTenantNamespace {
            tenant_id: &request.tenant_id,
            namespace_id: request.namespace_id,
        }
    }

    fn plan(request: &FreshAnchorRequestV1) -> Result<FreshAnchorDraft<'_>> {
        request.plan(
            request.schema_version,
            request.server_uuid,
            &request.schema,
            configured(request),
            &Redactor::new(Vec::new()),
        )
    }

    fn encoded_row(anchor: &FreshAnchorDraft<'_>) -> Result<EventColumns> {
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
                    configured(&request),
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
                    configured(&request),
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
    fn fresh_anchor_rejects_a_conflicting_configured_tenant_or_namespace() {
        let request = request();
        let redactor = Redactor::new(Vec::new());
        for configured in [
            ConfiguredTenantNamespace {
                tenant_id: "tenant_b",
                ..configured(&request)
            },
            ConfiguredTenantNamespace {
                namespace_id: Uuid::from_u128(6),
                ..configured(&request)
            },
        ] {
            assert!(
                request
                    .plan(
                        14,
                        request.server_uuid,
                        &request.schema,
                        configured,
                        &redactor
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn fresh_anchor_replays_only_the_same_operation_and_canonical_request() -> Result<()> {
        let original = request();
        let row = encoded_row(&plan(&original)?)?;
        let receipt = plan(&original)?.verify_recorded(row.clone(), Some(&row.3))?;
        assert_eq!((receipt.seq, receipt.hash), (row.0, row.4.clone()));
        let mut wrong_id = row.clone();
        wrong_id.1 = Uuid::from_u128(6).to_string();
        assert!(
            plan(&original)?
                .verify_recorded(wrong_id, Some(&row.3))
                .is_err()
        );
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

    #[test]
    fn fresh_anchor_requires_its_locked_physical_target() -> Result<()> {
        let request = request();
        let draft = plan(&request)?;
        let mut lock = OwnedAuditLock {
            name: "synthetic-lock".into(),
            session: 42,
            server: request.server_uuid,
            schema: request.schema.clone(),
        };
        draft.require_locked_target(&lock)?;

        lock.server = Uuid::from_u128(9);
        assert!(draft.require_locked_target(&lock).is_err());

        lock.server = request.server_uuid;
        lock.schema = "qa_other".into();
        assert!(draft.require_locked_target(&lock).is_err());

        lock.schema = request.schema.to_ascii_uppercase();
        assert!(draft.require_locked_target(&lock).is_err());
        Ok(())
    }

    #[test]
    fn fresh_anchor_stored_origin_binds_the_configured_target() -> Result<()> {
        let request = request();
        let draft = plan(&request)?;
        let event = draft.event();
        let namespace = request.namespace_id.to_string();
        let server = request.server_uuid;
        let ok = |event: &Event, tenant: &str, version: u16, server: Uuid, schema: &str| {
            FreshAnchorRequestV1::verify_stored_origin(
                event, tenant, &namespace, version, server, schema,
            )
        };
        assert!(ok(event, "tenant_a", 14, server, "qa_owned").is_ok());
        assert!(ok(event, "tenant_b", 14, server, "qa_owned").is_err());
        assert!(ok(event, "tenant_a", 15, server, "qa_owned").is_err());
        assert!(ok(event, "tenant_a", 14, Uuid::from_u128(9), "qa_owned").is_err());
        assert!(ok(event, "tenant_a", 14, server, "qa_other").is_err());

        let mut tampered_hash = event.clone();
        tampered_hash.data["request_hash"] = "0".repeat(64).into();
        assert!(ok(&tampered_hash, "tenant_a", 14, server, "qa_owned").is_err());

        let mut extended = event.clone();
        extended.data["request"]["extra"] = 1.into();
        assert!(ok(&extended, "tenant_a", 14, server, "qa_owned").is_err());

        let mut wrong_kind = event.clone();
        wrong_kind.kind = "control.mysql.audit.genesis".into();
        assert!(ok(&wrong_kind, "tenant_a", 14, server, "qa_owned").is_err());
        Ok(())
    }
}

fn request_shape_is_valid(version: u16, schema: &str) -> bool {
    matches!(version, 14 | 15)
        && !schema.is_empty()
        && schema.chars().count() <= 64
        && !schema.chars().any(char::is_control)
}

fn recorded_row_matches(requested_id: &str, stored_id: &str, payload_len: usize) -> bool {
    stored_id.as_bytes() == requested_id.as_bytes() && payload_len <= 131_072
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
    use super::{receipt_matches, recorded_row_matches, request_shape_is_valid};

    #[test]
    fn fresh_anchor_rejects_a_divergent_stored_event_id() {
        let expected = "abcdef00-0000-0000-0000-000000000001";
        assert!(recorded_row_matches(expected, expected, 512));
        assert!(!recorded_row_matches(
            expected,
            "abcdef00-0000-0000-0000-000000000002",
            512
        ));
        assert!(!recorded_row_matches(
            expected,
            "ABCDEF00-0000-0000-0000-000000000001",
            512
        ));
        assert!(!recorded_row_matches(
            expected,
            "abcdef00-0000-0000-0000-000000000001 ",
            512
        ));
    }

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
