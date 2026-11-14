use super::*;

fn identity(binding: u64) -> OwnedBrokerIdentityViewV2 {
    OwnedBrokerIdentityViewV2::new(
        7,
        OwnedBrokerSubjectV2 {
            installation_id: "00000000-0000-4000-8000-000000000001".into(),
            closure_id: "00000000-0000-4000-8000-000000000002".into(),
            source_id: "00000000-0000-4000-8000-000000000003".into(),
            source_format: 15,
            tenant_id: "tenant-a".into(),
            control_namespace_uuid: "00000000-0000-4000-8000-000000000004".into(),
            broker_id: "00000000-0000-4000-8000-000000000005".into(),
            incarnation_id: "00000000-0000-4000-8000-000000000006".into(),
            credential_id: "00000000-0000-4000-8000-000000000007".into(),
            binding_revision: binding,
            grant_revision: 1,
        },
    )
    .unwrap()
}

fn fence() -> OwnedBindingRotationFenceV2 {
    OwnedBindingRotationFenceV2 {
        previous_revision: 1,
        owner_incarnation: "00000000-0000-4000-8000-000000000010".into(),
        previous_owner_incarnation: "00000000-0000-4000-8000-000000000011".into(),
        previous_incarnation_seq: 2,
        previous_incarnation_hash: "e".repeat(64),
        incarnation_seq: 9,
        incarnation_hash: "d".repeat(64),
        receipt: crate::workload::WorkloadBindingReceiptV1 {
            version: 1,
            credential_id: identity(2).subject.credential_id,
            revision: 2,
            audit_event_id: "00000000-0000-4000-8000-000000000008".into(),
            audit_seq: 8,
            audit_hash: "a".repeat(64),
            replayed: true,
        },
    }
}

fn pending() -> OwnedHeartbeatPendingV2 {
    let old = identity(1);
    let broker = BrokerV1 {
        version: 1,
        broker_id: old.subject.broker_id.clone(),
        grant_revision: 1,
        heartbeat_revision: 0,
        credential_id: old.subject.credential_id.clone(),
        incarnation_id: old.subject.incarnation_id.clone(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: "local-docker".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2026-11-14T12:00:00.000Z".into(),
        revoked: false,
        granted_at: "2026-11-14T09:59:00.000Z".into(),
        revoked_at: None,
        last_heartbeat: None,
    };
    let view = OwnedBrokerSelfViewV2::new(
        old,
        broker.view_at("2026-11-14T10:00:00.000Z").unwrap(),
        None,
    )
    .unwrap();
    OwnedHeartbeatPendingV2::prepare(&view, "00000000-0000-4000-8000-000000000009".into()).unwrap()
}

#[test]
fn noncommitted_proof_requires_exact_predecessor_and_original_request_commitment() {
    let pending = pending();
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending.clone()),
        previous: None,
    };
    let mut proof = OwnedReconciliationResultV2 {
        profile: "broker_supervision_owned_v2".into(),
        version: 2,
        request_commitment: request.commitment().unwrap(),
        observation: identity(2),
        fence: fence(),
        head: OwnedReconciliationHeadV2::Heartbeat {
            view: pending.prior.clone(),
            receipt: None,
        },
        state: OwnedReconciliationStateV2::NonCommitted,
    };
    proof.non_committed_for(&request).unwrap();
    if let OwnedReconciliationHeadV2::Heartbeat { view, .. } = &mut proof.head {
        view.broker.heartbeat_revision = 4;
    }
    assert!(proof.non_committed_for(&request).is_err());
    proof.head = OwnedReconciliationHeadV2::Heartbeat {
        view: pending.prior.clone(),
        receipt: None,
    };
    proof.request_commitment = "b".repeat(64);
    assert!(proof.non_committed_for(&request).is_err());
}

#[test]
fn committed_rotation_ack_keeps_historical_actor_and_never_becomes_noncommitted() {
    let pending = pending();
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending.clone()),
        previous: None,
    };
    let mut broker = pending.prior.broker.clone();
    broker.heartbeat_revision = 1;
    broker.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
        incarnation_id: pending.heartbeat.incarnation_id.clone(),
        sequence: 1,
        received_at: "2026-11-14T10:00:01.000Z".into(),
        lease_expires_at: "2026-11-14T10:00:31.000Z".into(),
        heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    });
    let receipt = BrokerMutationReceiptV1 {
        operation_id: pending.heartbeat.operation_id.clone(),
        broker_id: broker.broker_id.clone(),
        grant_revision: 1,
        heartbeat_revision: 1,
        input_hash: pending.registry_input_hash.clone(),
        audit_seq: 7,
        audit_hash: "c".repeat(64),
    };
    let historical = OwnedBrokerHeartbeatResultV2::new(
        identity(1),
        pending.operation_commitment.clone(),
        BrokerMutationResultV1 {
            broker: broker.clone(),
            receipt: receipt.clone(),
            replayed: true,
        },
    )
    .unwrap();
    let proof = OwnedReconciliationResultV2 {
        profile: "broker_supervision_owned_v2".into(),
        version: 2,
        request_commitment: request.commitment().unwrap(),
        observation: identity(2),
        fence: fence(),
        head: OwnedReconciliationHeadV2::Heartbeat {
            view: broker.view_at("2026-11-14T10:00:02.000Z").unwrap(),
            receipt: Some(receipt),
        },
        state: OwnedReconciliationStateV2::CommittedExact {
            historical: OwnedHistoricalResultV2::Heartbeat {
                result: Box::new(historical.clone()),
            },
        },
    };
    proof.validate_for(&request).unwrap();
    assert_eq!(historical.identity.subject.binding_revision, 1);
    assert_eq!(proof.observation.subject.binding_revision, 2);
    assert!(proof.non_committed_for(&request).is_err());
    let mut rewritten = historical;
    rewritten.identity = identity(2);
    assert!(pending.validate_result(&rewritten).is_err());
}

#[test]
fn rotation_reconciliation_never_weakens_exact_replay_or_accepts_pin_changes() {
    let old = identity(1);
    let current = identity(2);
    assert_eq!(
        check_replay_identity(&old, &current),
        Err(OwnedIncompatibilityV2::BindingRevision)
    );
    fence().validate_for(&old, &current).unwrap();
    let mut other = current.clone();
    other.subject.tenant_id = "tenant-b".into();
    assert!(fence().validate_for(&old, &other).is_err());
    other = identity(3);
    assert!(fence().validate_for(&old, &other).is_err());
    other = current.clone();
    other.closure_revision = 6;
    assert!(fence().validate_for(&old, &other).is_err());
    let mut same_owner = fence();
    same_owner.incarnation_seq = same_owner.receipt.audit_seq;
    assert!(same_owner.validate_for(&old, &current).is_err());
    let mut repeated_append = fence();
    repeated_append.previous_owner_incarnation = repeated_append.owner_incarnation.clone();
    assert!(repeated_append.validate_for(&old, &current).is_err());
}

#[test]
fn final_profile_refuses_old_profile_and_unadmitted_source_format() {
    let mut value = serde_json::to_value(identity(1)).unwrap();
    value["profile"] = Value::String("broker_supervision_owned_v1".into());
    value["version"] = Value::from(1);
    let old: OwnedBrokerIdentityViewV2 = serde_json::from_value(value).unwrap();
    assert!(old.validate().is_err());
    assert_eq!(OwnedSourceFormatV2::from_verified(16), None);
    assert_eq!(
        OwnedBrokerOperationV2::classify("GET", "/api/v1/owned/brokers/heartbeats/reconcile"),
        None
    );
    assert_eq!(
        OwnedBrokerOperationV2::classify("POST", "/api/v1/owned/brokers/heartbeats/reconcile"),
        Some(OwnedBrokerOperationV2::HeartbeatReconcile)
    );
    assert!(!OwnedBrokerOperationV2::HeartbeatReconcile.mutating());
    let mut error = OwnedSupervisionErrorCodeV2::SupervisionUnavailable.body();
    assert_eq!(
        classify_remote_failure(503, &error),
        OwnedRemoteFailureV2::Owned(OwnedSupervisionErrorCodeV2::SupervisionUnavailable)
    );
    error["error"]["profile"] = Value::String("broker_supervision_owned_v1".into());
    assert_eq!(
        classify_remote_failure(503, &error),
        OwnedRemoteFailureV2::Unrecognized
    );
}
