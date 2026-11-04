use super::*;

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn grant() -> BrokerGrantMutationV1 {
    BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: id(),
        expected_revision: None,
        credential_id: id(),
        incarnation_id: id(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: "docker:host-a".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2026-11-03T12:00:00.000Z".into(),
    }
}

#[test]
fn broker_contracts_are_closed_bounded_and_cannot_choose_tenant() {
    let grant = grant();
    grant.validate().unwrap();
    let mut json = serde_json::to_value(&grant).unwrap();
    json["tenant_id"] = serde_json::json!("tenant-b");
    assert!(serde_json::from_value::<BrokerGrantMutationV1>(json).is_err());
    let mut invalid = grant.clone();
    invalid.lease_seconds = 301;
    assert!(invalid.validate().is_err());
    invalid.lease_seconds = 0;
    assert!(invalid.validate().is_err());
    let mut invalid = grant.clone();
    invalid.authorities.push(invalid.authorities[0].clone());
    assert!(invalid.validate().is_err());
    let mut invalid = grant;
    invalid.broker_id = uuid::Uuid::nil().to_string();
    assert!(invalid.validate().is_err());
}

#[test]
fn heartbeat_cannot_claim_authorities_or_client_time() {
    let heartbeat = BrokerHeartbeatV1 {
        version: 1,
        operation_id: id(),
        expected_heartbeat_revision: 0,
        incarnation_id: id(),
        sequence: 1,
    };
    heartbeat.validate().unwrap();
    for (field, value) in [
        ("authorities", serde_json::json!(["stolen"])),
        ("broker_id", serde_json::json!(id())),
        ("tenant_id", serde_json::json!("tenant-b")),
        ("observed_at", serde_json::json!("2030-01-01T00:00:00Z")),
    ] {
        let mut json = serde_json::to_value(&heartbeat).unwrap();
        json[field] = value;
        assert!(serde_json::from_value::<BrokerHeartbeatV1>(json).is_err());
    }
}

#[test]
fn liveness_is_explicit_and_never_claims_a_runtime_observation() {
    let grant = grant();
    let snapshot = BrokerV1 {
        version: 1,
        broker_id: grant.broker_id.clone(),
        grant_revision: 1,
        heartbeat_revision: 0,
        credential_id: grant.credential_id.clone(),
        incarnation_id: grant.incarnation_id.clone(),
        authorities: grant.authorities.clone(),
        lease_seconds: 30,
        not_after: grant.not_after.clone(),
        revoked: false,
        granted_at: "2026-11-03T10:00:00.000Z".into(),
        revoked_at: None,
        last_heartbeat: None,
    };
    assert_eq!(
        snapshot.view_at("2026-11-03T10:00:00.000Z").unwrap().state,
        BrokerStateV1::Unseen
    );
    let mut current = snapshot.clone();
    current.heartbeat_revision = 1;
    current.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
        incarnation_id: current.incarnation_id.clone(),
        sequence: 1,
        received_at: "2026-11-03T10:00:00.000Z".into(),
        lease_expires_at: "2026-11-03T10:00:30.000Z".into(),
        heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    });
    assert_eq!(
        current.view_at("2026-11-03T10:00:29.999Z").unwrap().state,
        BrokerStateV1::Current
    );
    assert_eq!(
        current.view_at("2026-11-03T10:00:30.000Z").unwrap().state,
        BrokerStateV1::Stale
    );
    assert_eq!(
        current.view_at("2026-11-03T09:59:59.999Z").unwrap().state,
        BrokerStateV1::ClockUncertain
    );
    assert_eq!(
        current.view_at("2026-11-03T12:00:00.000Z").unwrap().state,
        BrokerStateV1::Expired
    );
    current.revoked = true;
    current.revoked_at = Some("2026-11-03T10:00:10.000Z".into());
    assert_eq!(
        current.view_at("2026-11-03T10:00:20.000Z").unwrap().state,
        BrokerStateV1::Revoked
    );
    assert_eq!(current.last_heartbeat.as_ref().unwrap().sequence, 1);
}
