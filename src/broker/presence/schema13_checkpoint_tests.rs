//! Real retained stream files with descriptive gateway response fixtures.
//! The actual Store migration/checkpoint path is tested separately in audit.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture() -> preparation_tests::Fixture {
    let mut f = preparation_tests::Fixture::new();
    let state = f.root.join("checkpoint-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    f.value["version"] = serde_json::json!(2);
    f.value["checkpoint_state_dir"] = serde_json::json!(state);
    f.value["checkpoint_max_operations"] = serde_json::json!(8);
    f.value["checkpoint_max_state_bytes"] = serde_json::json!(1048576);
    f.write_config();
    f
}

fn prepared(f: &preparation_tests::Fixture) -> PreparedPresence {
    let mut p = PreparedPresence::load(&f.config, f.scope(), vec![]).unwrap();
    p.finalize(&[]).unwrap();
    p
}

fn pending(p: &PreparedPresence, schema: u32) -> checkpoint_storage::Pending {
    let identity = BrokerIdentityViewV1 {
        version: 1,
        tenant_id: p.authority.scope.expected_tenant_id.clone(),
        control_namespace_uuid: p.authority.scope.control_namespace_uuid.to_string(),
        schema_version: schema,
        broker_id: p.authority.config.broker_id.clone(),
        incarnation_id: p.authority.config.incarnation_id.clone(),
        credential_id: p.authority.config.credential_id.clone(),
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
                authority_id: p.authority.config.authority_id.clone(),
                runtime_kind: p.authority.scope.runtime_kind,
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
        input_hash: Store::broker_checkpoint_input_hash(&identity, &checkpoint).unwrap(),
        identity,
        checkpoint,
    }
}

fn view(p: &PreparedPresence, identity: &BrokerIdentityViewV1) -> BrokerCheckpointViewV1 {
    let broker = BrokerV1 {
        version: 1,
        broker_id: identity.broker_id.clone(),
        grant_revision: identity.grant_revision,
        heartbeat_revision: 0,
        credential_id: identity.credential_id.clone(),
        incarnation_id: identity.incarnation_id.clone(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: p.authority.config.authority_id.clone(),
            runtime_kind: p.authority.scope.runtime_kind,
        }],
        lease_seconds: 60,
        not_after: "2100-01-01T00:00:00.000Z".into(),
        revoked: false,
        granted_at: "2026-01-01T00:00:00.000Z".into(),
        revoked_at: None,
        last_heartbeat: None,
    };
    BrokerCheckpointViewV1 {
        version: 1,
        broker: broker.view_at("2026-01-01T00:00:01.000Z").unwrap(),
        checkpoint: None,
    }
}

fn ack(pending: &checkpoint_storage::Pending) -> BrokerCheckpointResultV1 {
    BrokerCheckpointResultV1 {
        checkpoint: pending.checkpoint.clone(),
        receipt: BrokerMutationReceiptV1 {
            operation_id: pending.checkpoint.operation_id.clone(),
            broker_id: pending.identity.broker_id.clone(),
            grant_revision: pending.identity.grant_revision,
            heartbeat_revision: 0,
            input_hash: pending.input_hash.clone(),
            audit_seq: 42,
            audit_hash: "b".repeat(64),
        },
        replayed: true,
    }
}

#[test]
fn pending12_can_finish_after_schema13_without_rewriting_its_identity_or_input() {
    let f = fixture();
    let p = prepared(&f);
    let old = pending(&p, 12);
    let stream = p.checkpoint_stream.as_ref().unwrap();
    stream.append(&p, old.clone()).unwrap();
    let retained_path = p
        .checkpoint_state
        .as_ref()
        .unwrap()
        .path()
        .join("op-00000000000000000001/pending.json");
    let before = fs::read(&retained_path).unwrap();
    let head = stream.checked_head(&p).unwrap();
    assert_eq!(
        validate_remote(&p, &old.identity, &view(&p, &old.identity), &head),
        Ok(())
    );
    let mut current = old.identity.clone();
    current.schema_version = 13;
    assert_eq!(
        Store::broker_checkpoint_input_hash(&current, &old.checkpoint).unwrap(),
        old.input_hash
    );
    assert_eq!(
        validate_remote(&p, &current, &view(&p, &current), &head),
        Ok(())
    );
    let result = ack(&old);
    let mut remote = view(&p, &current);
    remote.checkpoint = Some(BrokerCheckpointObservationV1 {
        checkpoint: old.checkpoint.clone(),
        receipt: result.receipt.clone(),
        received_at: "2026-01-01T00:00:01.000Z".into(),
        lease_expires_at: "2026-01-01T00:01:01.000Z".into(),
        in_current_grant: true,
        freshness: BrokerCheckpointFreshnessV1::Current,
        coverage: BrokerCheckpointCoverageV1::DeclaredPartial,
        publication_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    });
    assert_eq!(validate_remote(&p, &current, &remote, &head), Ok(()));
    stream.acknowledge(&p, result).unwrap();
    assert_eq!(fs::read(&retained_path).unwrap(), before);
    drop(p);
    let p = prepared(&f);
    let stream = p.checkpoint_stream.as_ref().unwrap();
    let head = stream.checked_head(&p).unwrap();
    assert!(head.pending.is_none());
    let last = head.last.unwrap();
    assert_eq!(last.0, old);
    assert_eq!(last.0.identity.schema_version, 12);
    let mut next = pending(&p, 13);
    next.checkpoint.previous_sequence = 1;
    next.checkpoint.sequence = 2;
    next.input_hash =
        Store::broker_checkpoint_input_hash(&next.identity, &next.checkpoint).unwrap();
    stream.append(&p, next.clone()).unwrap();
    assert_eq!(stream.pending(&p).unwrap(), Some(next));
    assert_eq!(fs::read(&retained_path).unwrap(), before);
}

#[test]
fn pending_identity_downgrade_binding_or_grant_drift_is_refused_without_stream_writes() {
    let f = fixture();
    let p = prepared(&f);
    let old = pending(&p, 12);
    let stream = p.checkpoint_stream.as_ref().unwrap();
    stream.append(&p, old.clone()).unwrap();
    let head = stream.checked_head(&p).unwrap();
    let mut current = old.identity.clone();
    current.schema_version = 13;
    assert_eq!(
        validate_remote(&p, &current, &view(&p, &current), &head),
        Ok(())
    );
    let before = stream.checked_head(&p).unwrap();
    for change in [
        "tenant",
        "namespace",
        "broker",
        "incarnation",
        "credential",
        "binding",
        "grant",
        "future",
        "version",
    ] {
        let mut wrong = current.clone();
        match change {
            "tenant" => wrong.tenant_id = "another-tenant".into(),
            "namespace" => wrong.control_namespace_uuid = Uuid::new_v4().to_string(),
            "broker" => wrong.broker_id = Uuid::new_v4().to_string(),
            "incarnation" => wrong.incarnation_id = Uuid::new_v4().to_string(),
            "credential" => wrong.credential_id = Uuid::new_v4().to_string(),
            "binding" => wrong.binding_revision += 1,
            "grant" => wrong.grant_revision += 1,
            "future" => wrong.schema_version = 14,
            "version" => wrong.version = 2,
            _ => unreachable!(),
        }
        assert!(
            validate_remote(&p, &wrong, &view(&p, &wrong), &head).is_err(),
            "accepted {change} drift"
        );
        assert!(stream.checked_head(&p).unwrap() == before);
    }
    let newer_fixture = fixture();
    let newer = prepared(&newer_fixture);
    let newer_pending = pending(&newer, 13);
    let newer_stream = newer.checkpoint_stream.as_ref().unwrap();
    newer_stream.append(&newer, newer_pending.clone()).unwrap();
    let newer_head = newer_stream.checked_head(&newer).unwrap();
    assert_eq!(
        validate_remote(
            &newer,
            &newer_pending.identity,
            &view(&newer, &newer_pending.identity),
            &newer_head
        ),
        Ok(())
    );
    let mut downgraded = newer_pending.identity.clone();
    downgraded.schema_version = 12;
    assert_eq!(
        validate_remote(&newer, &downgraded, &view(&newer, &downgraded), &newer_head),
        Err(PresenceError::Conflict)
    );
    assert!(newer_stream.checked_head(&newer).unwrap() == newer_head);
    assert!(stream.checked_head(&p).unwrap() == before);
}
