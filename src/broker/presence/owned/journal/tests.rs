use super::*;
use crate::broker_registry::owned_supervision::*;
use crate::broker_registry::*;
use std::os::unix::fs::PermissionsExt;
use uuid::Uuid;

fn id(n: u128) -> String {
    Uuid::from_u128(n).to_string()
}

fn prepared(root: &Directory) -> PreparedOwnedPresence {
    prepared_profile(root, false)
}
fn prepared_profile(root: &Directory, checkpoints: bool) -> PreparedOwnedPresence {
    let state = root.persistent_child("state", true).unwrap().unwrap();
    let authority = BrokerAuthorityV1 {
        authority_id: "filesystem-fixture".into(),
        runtime_kind: BrokerRuntimeKindV1::Docker,
    };
    let subject = OwnedBrokerSubjectV2 {
        installation_id: id(1),
        closure_id: id(2),
        source_id: id(3),
        source_format: 15,
        tenant_id: "tenant-a".into(),
        control_namespace_uuid: id(4),
        broker_id: id(5),
        incarnation_id: id(6),
        credential_id: id(7),
        binding_revision: 1,
        grant_revision: 1,
    };
    let enrollment = OwnedBrokerEnrollmentV2 {
        profile: OWNED_SUPERVISION_PROFILE_V2.into(),
        version: 2,
        gateway_origin: "http://127.0.0.1:19099".into(),
        closure_revision: 7,
        subject,
        authority,
        lease_seconds: 30,
        grant_not_after: "2100-01-01T00:00:00.000Z".into(),
        credential_expires_at: "2100-01-01T00:00:00.000Z".into(),
        issued_at: "2026-11-14T10:00:00.000Z".into(),
        grant_receipt: OwnedEnrollmentReceiptV2 {
            operation_id: id(8),
            audit_seq: 1,
            audit_hash: "a".repeat(64),
        },
        binding_receipt: OwnedEnrollmentReceiptV2 {
            operation_id: id(9),
            audit_seq: 2,
            audit_hash: "b".repeat(64),
        },
    };
    if root
        .private_entry(OsStr::new("enrollment.json"))
        .unwrap()
        .is_none()
    {
        save(
            root,
            "enrollment.json",
            &serde_json::to_vec(&enrollment).unwrap(),
        )
        .unwrap();
        save(
            root,
            "bearer",
            b"synthetic-filesystem-fixture-bearer-000000000000",
        )
        .unwrap();
        let mut config = serde_json::json!({"profile":"broker_supervision_owned_v2","version":2,
            "enrollment_file": root.path().join("enrollment.json"), "bearer_file":root.path().join("bearer"),
            "interval_seconds":5,"heartbeat_journal":{"state_dir":state.path(),"max_operations":20,"max_state_bytes":2097152}});
        if checkpoints {
            let checkpoint = root
                .persistent_child("checkpoint-state", true)
                .unwrap()
                .unwrap();
            config["checkpoint_journal"] = serde_json::json!({"state_dir":checkpoint.path(),"max_operations":20,"max_state_bytes":2097152});
        }
        save(root, "presence.json", &serde_json::to_vec(&config).unwrap()).unwrap();
    }
    let scope = PresenceScope {
        installation_id: Uuid::from_u128(10),
        profile_id: "fixture".into(),
        gateway: enrollment.gateway_origin,
        expected_tenant_id: "tenant-a".into(),
        control_namespace_uuid: Uuid::from_u128(4),
        runtime_kind: BrokerRuntimeKindV1::Docker,
    };
    let mut p =
        PreparedOwnedPresence::load(&root.path().join("presence.json"), scope, vec![]).unwrap();
    p.finalize(&[]).unwrap();
    p
}
fn pending(p: &PreparedOwnedPresence) -> OwnedHeartbeatPendingV2 {
    let identity = OwnedBrokerIdentityViewV2::new(7, p.pins.enrolled.clone()).unwrap();
    let broker = BrokerV1 {
        version: 1,
        broker_id: id(5),
        grant_revision: 1,
        heartbeat_revision: 0,
        credential_id: id(7),
        incarnation_id: id(6),
        authorities: vec![p.pins.authority.clone()],
        lease_seconds: 30,
        not_after: "2100-01-01T00:00:00.000Z".into(),
        revoked: false,
        granted_at: "2026-11-14T10:00:00.000Z".into(),
        revoked_at: None,
        last_heartbeat: None,
    };
    let view = OwnedBrokerSelfViewV2::new(
        identity,
        broker.view_at("2026-11-14T10:00:01.000Z").unwrap(),
        None,
    )
    .unwrap();
    OwnedHeartbeatPendingV2::prepare(&view, id(11)).unwrap()
}
fn proof(old: &OwnedHeartbeatPendingV2) -> OwnedReconciliationResultV2 {
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(old.clone()),
        previous: None,
    };
    let mut observation = old.identity.clone();
    observation.subject.binding_revision = 2;
    observation.closure_revision = 8;
    OwnedReconciliationResultV2 {
        profile: OWNED_SUPERVISION_PROFILE_V2.into(),
        version: 2,
        request_commitment: request.commitment().unwrap(),
        observation,
        fence: OwnedBindingRotationFenceV2 {
            previous_revision: 1,
            owner_incarnation: id(14),
            previous_owner_incarnation: id(15),
            previous_incarnation_seq: 1,
            previous_incarnation_hash: "f".repeat(64),
            incarnation_seq: 4,
            incarnation_hash: "d".repeat(64),
            receipt: crate::workload::WorkloadBindingReceiptV1 {
                version: 1,
                credential_id: id(7),
                revision: 2,
                audit_event_id: id(12),
                audit_seq: 3,
                audit_hash: "c".repeat(64),
                replayed: true,
            },
        },
        head: OwnedReconciliationHeadV2::Heartbeat {
            view: old.prior.clone(),
            receipt: None,
        },
        state: OwnedReconciliationStateV2::NonCommitted,
    }
}
fn root() -> (tempfile::TempDir, Directory) {
    let tmp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let root = Directory::open_existing(tmp.path()).unwrap();
    (tmp, root)
}

#[test]
fn synced_supersession_survives_reopen_without_new_uuid_or_rewritten_old_pending() {
    let (_tmp, root) = root();
    let p = prepared(&root);
    let old = pending(&p);
    let journal = p.heartbeat_journal().unwrap();
    journal.append(&p, old.clone()).unwrap();
    let directory = journal
        .root
        .persistent_child(&operation_name(1), false)
        .unwrap()
        .unwrap();
    let original = directory
        .persistent_read(PENDING, 65536, &[])
        .unwrap()
        .unwrap()
        .bytes()
        .to_vec();
    let proof = proof(&old);
    let replacement = heartbeat::HeartbeatJournal::replacement(&old, &proof, id(13)).unwrap();
    journal
        .supersede(&p, &old, proof.clone(), replacement.clone())
        .unwrap();
    assert_eq!(
        journal.cached_head().unwrap().replacement,
        Some(replacement.clone())
    );
    drop(p);
    let p = prepared(&root);
    let journal = p.heartbeat_journal().unwrap();
    let head = journal.cached_head().unwrap();
    assert!(head.last.is_none());
    assert!(head.pending.is_none());
    assert_eq!(head.binding_floor, 2);
    assert_eq!(head.closure_floor, 8);
    assert!(
        p.observe_closure(&old.identity).is_err(),
        "restart retains the current binding floor separately"
    );
    assert_eq!(head.replacement, Some(replacement.clone()));
    let different_uuid = heartbeat::HeartbeatJournal::replacement(&old, &proof, id(40)).unwrap();
    assert!(
        journal.append(&p, different_uuid).is_err(),
        "restart cannot mint a second replacement UUID"
    );
    assert_eq!(
        journal.cached_head().unwrap().replacement,
        Some(replacement.clone())
    );
    journal.append(&p, replacement.clone()).unwrap();
    assert!(
        journal
            .supersede(&p, &old, proof, replacement.clone())
            .is_err(),
        "late old generation must not close replacement"
    );
    let mut old_broker = old.prior.broker.clone();
    old_broker.heartbeat_revision = 1;
    old_broker.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
        incarnation_id: id(6),
        sequence: 1,
        received_at: "2026-11-14T10:00:02.000Z".into(),
        lease_expires_at: "2026-11-14T10:00:32.000Z".into(),
        heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    });
    let late = OwnedBrokerHeartbeatResultV2::new(
        old.identity.clone(),
        old.operation_commitment.clone(),
        BrokerMutationResultV1 {
            broker: old_broker,
            receipt: BrokerMutationReceiptV1 {
                operation_id: old.heartbeat.operation_id.clone(),
                broker_id: id(5),
                grant_revision: 1,
                heartbeat_revision: 1,
                input_hash: old.registry_input_hash.clone(),
                audit_seq: 3,
                audit_hash: "e".repeat(64),
            },
            replayed: false,
        },
    )
    .unwrap();
    assert!(
        journal
            .acknowledge(&p, OwnedAcknowledgementSourceV2::Response, late)
            .is_err(),
        "late old ACK cannot clear the new Pending"
    );
    assert_eq!(journal.cached_head().unwrap().pending, Some(replacement));
    assert_eq!(
        directory
            .persistent_read(PENDING, 65536, &[])
            .unwrap()
            .unwrap()
            .bytes(),
        original
    );
}

#[test]
fn insufficient_recovery_budget_refuses_before_closure_and_torn_record_is_kept() {
    let (_tmp, root) = root();
    let mut p = prepared(&root);
    let old = pending(&p);
    p.heartbeat_journal()
        .unwrap()
        .append(&p, old.clone())
        .unwrap();
    let journal = p.heartbeat.as_mut().unwrap();
    journal.limits.bytes =
        journal.cached_head().unwrap().bytes + heartbeat::HeartbeatJournal::RECORD_BYTES;
    let proof = proof(&old);
    let replacement = heartbeat::HeartbeatJournal::replacement(&old, &proof, id(13)).unwrap();
    let journal = p.heartbeat_journal().unwrap();
    assert!(matches!(
        journal.supersede(&p, &old, proof, replacement),
        Err(PresenceError::Capacity)
    ));
    assert!(
        p.observe_closure(&old.identity).is_err(),
        "a validated proof stays observed even when closure financing refuses"
    );
    let directory = journal
        .root
        .persistent_child(&operation_name(1), false)
        .unwrap()
        .unwrap();
    assert!(
        directory
            .private_entry(OsStr::new(SUPERSEDED))
            .unwrap()
            .is_none()
    );
    save(&directory, SUPERSEDED, b"{").unwrap();
    assert!(journal.cached_head().unwrap().pending.is_some());
    assert!(journal.durable_pending(&p).is_err());
    assert_eq!(
        directory
            .persistent_read(SUPERSEDED, 32, &[])
            .unwrap()
            .unwrap()
            .bytes(),
        b"{"
    );
}

#[test]
fn v1_anchor_is_refused_without_conversion_or_rewriting_existing_bytes() {
    let (_tmp, root) = root();
    let p = prepared(&root);
    let original = p.heartbeat_journal().unwrap().anchor.bytes();
    let mut value: Value = serde_json::from_slice(original).unwrap();
    let old = root
        .persistent_child("old-v1-state", true)
        .unwrap()
        .unwrap();
    let (device, inode) = directory_identity(&old).unwrap();
    value["version"] = serde_json::json!(1);
    value["profile"] = serde_json::json!("broker_supervision_owned_v1");
    value["profile_version"] = serde_json::json!(1);
    value["pins"]["version"] = serde_json::json!(1);
    value["pins"]["profile"] = serde_json::json!("broker_supervision_owned_v1");
    value["directory_device"] = serde_json::json!(device);
    value["directory_inode"] = serde_json::json!(inode);
    let bytes = serde_json::to_vec(&value).unwrap();
    save(&old, LOCK, &[]).unwrap();
    save(&old, ANCHOR, &bytes).unwrap();
    assert!(
        Journal::<heartbeat::HeartbeatJournal>::open(
            &p,
            &old,
            JournalLimits {
                operations: 20,
                bytes: 2097152
            }
        )
        .is_err()
    );
    assert_eq!(
        old.persistent_read(ANCHOR, ANCHOR_BYTES, &[])
            .unwrap()
            .unwrap()
            .bytes(),
        bytes
    );
    assert_eq!(old.persistent_entries(4).unwrap().len(), 2);
}

#[test]
fn checkpoint_replacement_preserves_original_snapshot_and_sequence() {
    let (_tmp, root) = root();
    let p = prepared(&root);
    let hb = pending(&p);
    let snapshot = BrokerCheckpointSnapshotV1 {
        version: 1,
        authority: p.pins.authority.clone(),
        jobs: vec![BrokerJobCheckpointV1 {
            job_id: id(30),
            revision: 4,
            checkpoint_hash: "e".repeat(64),
        }],
        has_more: true,
    };
    let old = OwnedCheckpointPendingV2::prepare(&hb.identity, 0, snapshot.clone(), id(31)).unwrap();
    let request = OwnedReconciliationRequestV2::Checkpoint {
        pending: Box::new(old.clone()),
        previous: None,
    };
    let mut proof = proof(&hb);
    proof.request_commitment = request.commitment().unwrap();
    proof.head = OwnedReconciliationHeadV2::Checkpoint {
        view: hb.prior.clone(),
        current: None,
    };
    proof.non_committed_for(&request).unwrap();
    let replacement = checkpoint::CheckpointJournal::replacement(&old, &proof, id(32)).unwrap();
    assert_eq!(replacement.checkpoint.snapshot, snapshot);
    assert_eq!(replacement.checkpoint.sequence, 1);
    assert_eq!(replacement.checkpoint.previous_sequence, 0);
    assert_eq!(replacement.identity.subject.binding_revision, 2);
    assert_ne!(
        replacement.checkpoint.operation_id,
        old.checkpoint.operation_id
    );
}

#[test]
fn reconciled_ack_persists_old_actor_and_current_observation_as_distinct_facts() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let old = pending(&p);
    p.heartbeat_journal()
        .unwrap()
        .append(&p, old.clone())
        .unwrap();
    let mut broker = old.prior.broker.clone();
    broker.heartbeat_revision = 1;
    broker.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
        incarnation_id: id(6),
        sequence: 1,
        received_at: "2026-11-14T10:00:02.000Z".into(),
        lease_expires_at: "2026-11-14T10:00:32.000Z".into(),
        heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    });
    let receipt = BrokerMutationReceiptV1 {
        operation_id: old.heartbeat.operation_id.clone(),
        broker_id: id(5),
        grant_revision: 1,
        heartbeat_revision: 1,
        input_hash: old.registry_input_hash.clone(),
        audit_seq: 3,
        audit_hash: "e".repeat(64),
    };
    let historical = OwnedBrokerHeartbeatResultV2::new(
        old.identity.clone(),
        old.operation_commitment.clone(),
        BrokerMutationResultV1 {
            broker: broker.clone(),
            receipt: receipt.clone(),
            replayed: true,
        },
    )
    .unwrap();
    let mut proof = proof(&old);
    proof.fence.receipt.audit_seq = 4;
    proof.fence.incarnation_seq = 5;
    proof.head = OwnedReconciliationHeadV2::Heartbeat {
        view: broker.view_at("2026-11-14T10:00:03.000Z").unwrap(),
        receipt: Some(receipt),
    };
    proof.state = OwnedReconciliationStateV2::CommittedExact {
        historical: OwnedHistoricalResultV2::Heartbeat {
            result: Box::new(historical),
        },
    };
    p.heartbeat_journal()
        .unwrap()
        .acknowledge_reconciled(&p, proof)
        .unwrap();
    // The current observation must gate the peer journal immediately, while
    // the historical result actor stays immutable at binding1/closure7.
    assert!(p.observe_closure(&old.identity).is_err());
    let mut lower_closure = old.identity.clone();
    lower_closure.subject.binding_revision = 2;
    assert!(p.observe_closure(&lower_closure).is_err());
    drop(p);
    let p = prepared_profile(&root, true);
    let head = p.heartbeat_journal().unwrap().cached_head().unwrap();
    let ack = head.last.unwrap();
    assert_eq!(ack.result.identity.subject.binding_revision, 1);
    assert_eq!(ack.observation.subject.binding_revision, 2);
    assert_eq!(head.binding_floor, 2);
    assert_eq!(head.closure_floor, 8);
    assert!(head.pending.is_none());
}

fn heartbeat_ack(old: &OwnedHeartbeatPendingV2, closure: u64) -> OwnedBrokerHeartbeatResultV2 {
    let mut broker = old.prior.broker.clone();
    broker.heartbeat_revision = 1;
    broker.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
        incarnation_id: id(6),
        sequence: 1,
        received_at: "2026-11-14T10:00:02.000Z".into(),
        lease_expires_at: "2026-11-14T10:00:32.000Z".into(),
        heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    });
    let mut identity = old.identity.clone();
    identity.closure_revision = closure;
    let result = OwnedBrokerHeartbeatResultV2::new(
        identity,
        old.operation_commitment.clone(),
        BrokerMutationResultV1 {
            broker,
            receipt: BrokerMutationReceiptV1 {
                operation_id: old.heartbeat.operation_id.clone(),
                broker_id: id(5),
                grant_revision: 1,
                heartbeat_revision: 1,
                input_hash: old.registry_input_hash.clone(),
                audit_seq: 3,
                audit_hash: "e".repeat(64),
            },
            replayed: false,
        },
    )
    .unwrap();
    old.validate_result(&result).unwrap();
    result
}
fn checkpoint_pending(p: &PreparedOwnedPresence) -> OwnedCheckpointPendingV2 {
    OwnedCheckpointPendingV2::prepare(
        &pending(p).identity,
        0,
        BrokerCheckpointSnapshotV1 {
            version: 1,
            authority: p.pins.authority.clone(),
            jobs: vec![],
            has_more: false,
        },
        id(31),
    )
    .unwrap()
}
fn checkpoint_ack(old: &OwnedCheckpointPendingV2, closure: u64) -> OwnedBrokerCheckpointResultV2 {
    let mut identity = old.identity.clone();
    identity.closure_revision = closure;
    let result = OwnedBrokerCheckpointResultV2::new(
        identity,
        old.operation_commitment.clone(),
        BrokerCheckpointResultV1 {
            checkpoint: old.checkpoint.clone(),
            receipt: BrokerMutationReceiptV1 {
                operation_id: old.checkpoint.operation_id.clone(),
                broker_id: id(5),
                grant_revision: 1,
                heartbeat_revision: 1,
                input_hash: old.registry_input_hash.clone(),
                audit_seq: 4,
                audit_hash: "d".repeat(64),
            },
            replayed: false,
        },
    )
    .unwrap();
    old.validate_result(&result).unwrap();
    result
}

#[test]
fn heartbeat_ack_syncs_a_higher_observation_and_blocks_the_checkpoint_view_without_reopen() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let old = pending(&p);
    p.observe_closure(&old.identity).unwrap();
    let journal = p.heartbeat_journal().unwrap();
    journal.append(&p, old.clone()).unwrap();
    let result = heartbeat_ack(&old, 8);
    journal
        .acknowledge(&p, OwnedAcknowledgementSourceV2::Response, result.clone())
        .unwrap();
    let directory = journal
        .root
        .persistent_child(&operation_name(1), false)
        .unwrap()
        .unwrap();
    let (record, _): (AckRecordV2<OwnedBrokerHeartbeatResultV2>, _) = read(
        &p,
        &directory,
        ACK,
        heartbeat::HeartbeatJournal::RECORD_BYTES,
    )
    .unwrap();
    assert_eq!(record.result, result);
    assert_eq!(record.observation.closure_revision, 8);
    let peer = OwnedBrokerCheckpointSelfViewV2::new(
        old.identity.clone(),
        BrokerCheckpointViewV1 {
            version: 1,
            broker: old.prior.clone(),
            checkpoint: None,
        },
    )
    .unwrap();
    validate_checkpoint_handshake(&p.pins, &peer, p.min_lease()).unwrap();
    assert!(
        p.observe_closure(&peer.identity).is_err(),
        "durable heartbeat ACK8 must gate checkpoint view7 in this process"
    );
    assert!(
        p.checkpoint_journal()
            .unwrap()
            .cached_head()
            .unwrap()
            .last
            .is_none()
    );
}

#[test]
fn checkpoint_ack_syncs_a_higher_observation_and_blocks_the_heartbeat_view_without_reopen() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let hb = pending(&p);
    let old = checkpoint_pending(&p);
    p.observe_closure(&old.identity).unwrap();
    let journal = p.checkpoint_journal().unwrap();
    journal.append(&p, old.clone()).unwrap();
    let result = checkpoint_ack(&old, 8);
    journal
        .acknowledge(&p, OwnedAcknowledgementSourceV2::Response, result.clone())
        .unwrap();
    let directory = journal
        .root
        .persistent_child(&operation_name(1), false)
        .unwrap()
        .unwrap();
    let (record, _): (AckRecordV2<OwnedBrokerCheckpointResultV2>, _) = read(
        &p,
        &directory,
        ACK,
        checkpoint::CheckpointJournal::RECORD_BYTES,
    )
    .unwrap();
    assert_eq!(record.result, result);
    assert_eq!(record.observation.closure_revision, 8);
    let peer = OwnedBrokerSelfViewV2::new(hb.identity.clone(), hb.prior.clone(), None).unwrap();
    validate_heartbeat_handshake(&p.pins, &peer, p.min_lease()).unwrap();
    assert!(
        p.observe_closure(&peer.identity).is_err(),
        "durable checkpoint ACK8 must gate heartbeat view7 in this process"
    );
}

#[test]
fn a_stale_ack_cannot_publish_below_the_shared_floor_or_clear_either_pending() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let hb = pending(&p);
    let cp = checkpoint_pending(&p);
    let heartbeat = p.heartbeat_journal().unwrap();
    let checkpoint = p.checkpoint_journal().unwrap();
    heartbeat.append(&p, hb.clone()).unwrap();
    checkpoint.append(&p, cp.clone()).unwrap();
    let mut current = hb.identity.clone();
    current.closure_revision = 8;
    p.observe_closure(&current).unwrap();
    assert!(
        heartbeat
            .acknowledge(
                &p,
                OwnedAcknowledgementSourceV2::Response,
                heartbeat_ack(&hb, 7)
            )
            .is_err()
    );
    assert!(
        checkpoint
            .acknowledge(
                &p,
                OwnedAcknowledgementSourceV2::Response,
                checkpoint_ack(&cp, 7)
            )
            .is_err()
    );
    assert_eq!(heartbeat.durable_pending(&p).unwrap(), hb);
    assert_eq!(checkpoint.durable_pending(&p).unwrap(), cp);
    for root in [&heartbeat.root, &checkpoint.root] {
        let directory = root
            .persistent_child(&operation_name(1), false)
            .unwrap()
            .unwrap();
        assert!(directory.private_entry(OsStr::new(ACK)).unwrap().is_none());
    }
}

#[test]
fn rejected_observation_does_not_partially_raise_the_other_floor() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let old = pending(&p);
    let mut accepted = old.identity.clone();
    accepted.closure_revision = 8;
    p.observe_closure(&accepted).unwrap();
    let mut rejected = old.identity.clone();
    rejected.subject.binding_revision = 2;
    assert!(p.observe_closure(&rejected).is_err());
    accepted.closure_revision = 9;
    p.observe_closure(&accepted).unwrap();
}

#[test]
fn late_supersession_cannot_publish_below_a_peer_observation() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let old = pending(&p);
    let journal = p.heartbeat_journal().unwrap();
    journal.append(&p, old.clone()).unwrap();
    let proof = proof(&old);
    let replacement = heartbeat::HeartbeatJournal::replacement(&old, &proof, id(13)).unwrap();
    let mut current = proof.observation.clone();
    current.closure_revision = 9;
    p.observe_closure(&current).unwrap();
    assert!(journal.supersede(&p, &old, proof, replacement).is_err());
    let directory = journal
        .root
        .persistent_child(&operation_name(1), false)
        .unwrap()
        .unwrap();
    assert!(
        directory
            .private_entry(OsStr::new(SUPERSEDED))
            .unwrap()
            .is_none()
    );
    assert_eq!(journal.durable_pending(&p).unwrap(), old);
}

#[test]
fn a_fixed_replacement_keeps_its_engaged_identity_without_lowering_the_current_floor() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let old = pending(&p);
    let journal = p.heartbeat_journal().unwrap();
    journal.append(&p, old.clone()).unwrap();
    let proof = proof(&old);
    let replacement = heartbeat::HeartbeatJournal::replacement(&old, &proof, id(13)).unwrap();
    journal
        .supersede(&p, &old, proof, replacement.clone())
        .unwrap();
    let mut later = replacement.identity.clone();
    later.subject.binding_revision = 3;
    later.closure_revision = 9;
    p.observe_closure(&later).unwrap();
    // This only persists the already fixed Pending, never an ACK or a POST.
    // The actor still needs a current self-view and rotation reconciliation.
    journal.append(&p, replacement.clone()).unwrap();
    assert_eq!(journal.durable_pending(&p).unwrap(), replacement);
    assert!(journal.cached_head().unwrap().last.is_none());
    assert!(p.observe_closure(&replacement.identity).is_err());
}

// This test-only pause sits before the actual immutable ACK write. It grants
// no permission, changes no record and cannot simulate a successful fsync.
type AckBoundary = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
thread_local! { static ACK_BOUNDARY: std::cell::RefCell<Option<AckBoundary>> = const { std::cell::RefCell::new(None) }; }
pub(super) fn before_ack_save(p: &PreparedOwnedPresence) {
    if let Some((entered, release)) = ACK_BOUNDARY.with(|slot| slot.borrow_mut().take()) {
        assert!(
            matches!(
                p.observations.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ),
            "actual ACK must retain the shared observation gate through its save"
        );
        entered.send(()).unwrap();
        release
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
    }
}

#[test]
fn ack_publication_keeps_the_shared_gate_until_the_real_save_and_head_update() {
    let (_tmp, root) = root();
    let p = prepared_profile(&root, true);
    let old = pending(&p);
    let journal = p.heartbeat_journal().unwrap();
    journal.append(&p, old.clone()).unwrap();
    let result = heartbeat_ack(&old, 8);
    let (entered, at_save) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let ack = scope.spawn(|| {
            ACK_BOUNDARY.with(|slot| *slot.borrow_mut() = Some((entered, released)));
            journal.acknowledge(&p, OwnedAcknowledgementSourceV2::Response, result)
        });
        at_save
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(
            journal
                .root
                .persistent_child(&operation_name(1), false)
                .unwrap()
                .unwrap()
                .private_entry(OsStr::new(ACK))
                .unwrap()
                .is_none()
        );
        let mut higher = old.identity.clone();
        higher.closure_revision = 9;
        let profile = &p;
        let observer = scope.spawn(move || {
            profile.observe_closure(&higher).unwrap();
            // A later observation cannot complete while an older accepted
            // ACK is still waiting to sync/publish its local head.
            let head = journal.cached_head().unwrap();
            assert!(head.pending.is_none());
            assert_eq!(head.last.unwrap().observation.closure_revision, 8);
        });
        release.send(()).unwrap();
        ack.join().unwrap().unwrap();
        observer.join().unwrap();
    });
    assert!(journal.cached_head().unwrap().pending.is_none());
    let directory = journal
        .root
        .persistent_child(&operation_name(1), false)
        .unwrap()
        .unwrap();
    let (record, _): (AckRecordV2<OwnedBrokerHeartbeatResultV2>, _) = read(
        &p,
        &directory,
        ACK,
        heartbeat::HeartbeatJournal::RECORD_BYTES,
    )
    .unwrap();
    assert_eq!(record.observation.closure_revision, 8);
}
