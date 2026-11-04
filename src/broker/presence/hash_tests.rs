use crate::{
    audit::Store,
    broker_registry::{BrokerHeartbeatV1, BrokerIdentityViewV1},
};
use sha2::{Digest, Sha256};

fn identity() -> BrokerIdentityViewV1 {
    BrokerIdentityViewV1 {
        version: 1,
        tenant_id: "fixture-tenant".into(),
        control_namespace_uuid: "50000000-0000-4000-8000-000000000001".into(),
        schema_version: 6,
        broker_id: "10000000-0000-4000-8000-000000000001".into(),
        incarnation_id: "30000000-0000-4000-8000-000000000001".into(),
        credential_id: "20000000-0000-4000-8000-000000000001".into(),
        binding_revision: 3,
        grant_revision: 7,
    }
}
fn heartbeat() -> BrokerHeartbeatV1 {
    BrokerHeartbeatV1 {
        version: 1,
        operation_id: "60000000-0000-4000-8000-000000000001".into(),
        expected_heartbeat_revision: 9,
        incarnation_id: "30000000-0000-4000-8000-000000000001".into(),
        sequence: 2,
    }
}

#[test]
fn publisher_commitment_preserves_the_existing_server_serialized_hash_contract() {
    // Literal captures Change/IdentityEvidence's pre-publisher serialized bytes.
    // A reordered field, extra DTO field, or wrong domain must change this hash.
    let historical = r#"{"action":"heartbeat","identity":{"tenant_id":"fixture-tenant","broker_id":"10000000-0000-4000-8000-000000000001","credential_id":"20000000-0000-4000-8000-000000000001","binding_revision":3,"grant_revision":7},"heartbeat":{"version":1,"operation_id":"60000000-0000-4000-8000-000000000001","expected_heartbeat_revision":9,"incarnation_id":"30000000-0000-4000-8000-000000000001","sequence":2}}"#;
    let expected = hex::encode(
        Sha256::new()
            .chain_update(b"ai-guard:broker-registry-input:v1\0")
            .chain_update(historical.as_bytes())
            .finalize(),
    );
    assert_eq!(
        Store::broker_heartbeat_input_hash(&identity(), &heartbeat()).unwrap(),
        expected
    );
}

#[test]
fn commitment_does_not_turn_invalid_identity_dto_into_a_proof() {
    for field in ["version", "schema", "namespace", "binding", "incarnation"] {
        let mut view = identity();
        match field {
            "version" => view.version = 2,
            "schema" => view.schema_version = 5,
            "namespace" => {
                view.control_namespace_uuid = "00000000-0000-0000-0000-000000000000".into()
            }
            "binding" => view.binding_revision = 0,
            "incarnation" => view.incarnation_id = "30000000-0000-4000-8000-000000000002".into(),
            _ => unreachable!(),
        }
        assert!(Store::broker_heartbeat_input_hash(&view, &heartbeat()).is_err());
    }
}
