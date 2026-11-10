//! Real private files and retained identities; no HTTP success or runtime is
//! claimed by these storage-level response fixtures.
use super::*;
use crate::broker_registry::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn prepared(fixture: &preparation_tests::Fixture) -> PreparedPresence {
    let mut p = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    p.finalize(&[]).unwrap();
    p
}
fn fixture() -> preparation_tests::Fixture {
    let mut fixture = preparation_tests::Fixture::new();
    let checkpoint_state = fixture.root.join("checkpoint-state");
    fs::create_dir(&checkpoint_state).unwrap();
    fs::set_permissions(&checkpoint_state, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.value["version"] = serde_json::json!(2);
    fixture.value["checkpoint_state_dir"] = serde_json::json!(checkpoint_state);
    fixture.value["checkpoint_max_operations"] = serde_json::json!(8);
    fixture.value["checkpoint_max_state_bytes"] = serde_json::json!(1048576);
    fixture.write_config();
    fixture
}
fn pending(p: &PreparedPresence) -> checkpoint_storage::Pending {
    let identity = BrokerIdentityViewV1 {
        version: 1,
        tenant_id: p.scope.expected_tenant_id.clone(),
        control_namespace_uuid: p.scope.control_namespace_uuid.to_string(),
        schema_version: 12,
        broker_id: p.config.broker_id.clone(),
        credential_id: p.config.credential_id.clone(),
        incarnation_id: p.config.incarnation_id.clone(),
        binding_revision: 1,
        grant_revision: 1,
    };
    let checkpoint = BrokerCheckpointV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        incarnation_id: identity.incarnation_id.clone(),
        previous_sequence: 0,
        sequence: 1,
        snapshot: BrokerCheckpointSnapshotV1 {
            version: 1,
            authority: BrokerAuthorityV1 {
                authority_id: p.config.authority_id.clone(),
                runtime_kind: p.scope.runtime_kind,
            },
            jobs: vec![BrokerJobCheckpointV1 {
                job_id: Uuid::new_v4().to_string(),
                revision: 17,
                checkpoint_hash: "a".repeat(64),
            }],
            has_more: true,
        },
    };
    checkpoint_storage::Pending {
        version: 1,
        input_hash: crate::audit::Store::broker_checkpoint_input_hash(&identity, &checkpoint)
            .unwrap(),
        identity,
        checkpoint,
    }
}

#[test]
fn checkpoint_pending_reopens_with_exact_identity_before_any_replacement_is_queued() {
    let fixture = fixture();
    let p = prepared(&fixture);
    let pending = pending(&p);
    let stream = p.checkpoint_stream.as_ref().unwrap();
    stream.append(&p, pending.clone()).unwrap();
    assert!(stream.append(&p, pending.clone()).is_err());
    drop(p);
    let reopened = prepared(&fixture);
    let actual = reopened
        .checkpoint_stream
        .as_ref()
        .unwrap()
        .pending(&reopened)
        .unwrap()
        .unwrap();
    assert_eq!(
        actual, pending,
        "reopen must not allocate a new sequence or operation after lost response"
    );
}

#[test]
fn checkpoint_rejects_wrong_ack_and_mutated_historical_evidence() {
    let fixture = fixture();
    let p = prepared(&fixture);
    let pending = pending(&p);
    let stream = p.checkpoint_stream.as_ref().unwrap();
    stream.append(&p, pending.clone()).unwrap();
    let mut result = BrokerCheckpointResultV1 {
        checkpoint: pending.checkpoint.clone(),
        replayed: false,
        receipt: BrokerMutationReceiptV1 {
            operation_id: pending.checkpoint.operation_id.clone(),
            broker_id: pending.identity.broker_id.clone(),
            grant_revision: 1,
            heartbeat_revision: 0,
            input_hash: pending.input_hash.clone(),
            audit_seq: 3,
            audit_hash: "b".repeat(64),
        },
    };
    result.checkpoint.sequence = 2;
    assert!(stream.acknowledge(&p, result.clone()).is_err());
    result.checkpoint = pending.checkpoint.clone();
    stream.acknowledge(&p, result).unwrap();
    let path = p
        .checkpoint_state
        .as_ref()
        .unwrap()
        .path()
        .join("op-00000000000000000001/pending.json");
    let mut bytes = fs::read(&path).unwrap();
    let index = bytes.iter().position(|byte| *byte == b'a').unwrap();
    bytes[index] = b'z';
    fs::write(&path, bytes).unwrap();
    assert!(
        stream.pending(&p).is_err(),
        "even acknowledged history is checked before another publication"
    );
}

#[test]
fn checkpoint_configuration_requires_separate_private_state_and_explicit_budgets() {
    for bad in ["same-state", "zero-budget", "legacy-field"] {
        let fixture = fixture();
        let mut value = fixture.value.clone();
        match bad {
            "same-state" => value["checkpoint_state_dir"] = serde_json::json!(&fixture.state),
            "zero-budget" => value["checkpoint_max_state_bytes"] = serde_json::json!(0),
            "legacy-field" => {
                value["version"] = serde_json::json!(1);
                value["checkpoint_state_dir"] = serde_json::Value::Null;
            }
            _ => unreachable!(),
        }
        fs::write(&fixture.config, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).is_err());
        assert_eq!(
            fs::read_dir(&fixture.state).unwrap().count(),
            0,
            "refusal must precede either stream's initialization"
        );
    }
}

#[tokio::test]
async fn checkpoint_config_without_a_real_source_is_refused_before_network_publication() {
    let fixture = fixture();
    let p = prepared(&fixture);
    let (_shutdown, receiver) = watch::channel(false);
    let result = p
        .run(
            receiver,
            Arc::new(Semaphore::new(1)),
            Arc::new(Semaphore::new(1)),
        )
        .await;
    assert_eq!(result, Err(PresenceError::InvalidConfig));
}
