#[cfg(test)]
mod tests {
    use super::*;
    use agentsandbox::model::Event;
    use serde_json::json;

    #[test]
    fn checkpoint_outbox_view_refuses_unconfirmed_frames() {
        let operation = Uuid::from_u128(1);
        let intent_hash = "a".repeat(64);
        for (version, domain, payload) in [
            (4, b"ai-guard:broker-checkpoint:v4\0", json!({"version":4})),
            (
                5,
                b"ai-guard:broker-checkpoint:v5\0",
                json!({"outbox":{"version":4}}),
            ),
        ] {
            let bytes = serde_json::to_vec(&json!({
                "version":version, "operation_id":operation, "intent_hash":intent_hash,
                "revision":19, "previous":null, "payload":payload
            }))
            .unwrap();
            let hash = hex::encode(
                Sha256::new()
                    .chain_update(domain)
                    .chain_update(&bytes)
                    .finalize(),
            );
            let head = CheckpointHead {
                revision: RevisionRefV1::new(19, &hash).unwrap(),
                intent_hash: intent_hash.clone(),
            };
            let outbox = confirmed_outbox_payload(&bytes, operation, &head).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(outbox.get()).unwrap(),
                json!({"version":4})
            );
            assert!(confirmed_outbox_payload(&bytes, Uuid::from_u128(2), &head).is_none());

            let mut differing = bytes.clone();
            differing.push(b' ');
            assert!(confirmed_outbox_payload(&differing, operation, &head).is_none());
            let wrong_revision = CheckpointHead {
                revision: RevisionRefV1::new(20, &hash).unwrap(),
                intent_hash: intent_hash.clone(),
            };
            assert!(confirmed_outbox_payload(&bytes, operation, &wrong_revision).is_none());
        }
    }

    #[test]
    fn checkpoint_outbox_delivery_distinguishes_absence_pending_ack_and_unknown() {
        let kind = "broker.batch.failure";
        let session = "synthetic-fixture-session";
        let mut event = Event::new(kind);
        event.session_id = Some(session.into());
        event.source = "runner".into();
        event.data = json!({"detail":"synthetic private fixture body"});
        let pending = OutboxEventV1::new(Uuid::from_u128(1), &"a".repeat(64), event).unwrap();
        let acknowledged = pending.acknowledged(22).unwrap();
        assert_eq!(outbox_delivery(&[], kind, session), OutboxDelivery::Absent);
        assert_eq!(
            outbox_delivery(&[pending.clone()], kind, session),
            OutboxDelivery::Pending
        );
        assert_eq!(
            outbox_delivery(&[acknowledged.clone()], kind, session),
            OutboxDelivery::Acknowledged
        );
        assert_eq!(
            outbox_delivery(&[pending.clone()], kind, "different-session"),
            OutboxDelivery::Unknown
        );
        assert_eq!(
            outbox_delivery(&[pending, acknowledged], kind, session),
            OutboxDelivery::Unknown
        );
        assert_eq!(
            serde_json::to_value(TerminalCheckpointObservation::default()).unwrap(),
            json!({"result_audit":"unknown", "failure_audit":"unknown"})
        );
    }
}
