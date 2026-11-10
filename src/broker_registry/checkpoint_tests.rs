use super::*;
use serde_json::json;

fn snapshot() -> BrokerCheckpointSnapshotV1 {
    BrokerCheckpointSnapshotV1 {
        version: 1,
        authority: BrokerAuthorityV1 {
            authority_id: "host-a:docker".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        },
        jobs: vec![BrokerJobCheckpointV1 {
            job_id: "10000000-0000-4000-8000-000000000001".into(),
            revision: 17,
            checkpoint_hash: "a".repeat(64),
        }],
        has_more: false,
    }
}

#[test]
fn checkpoint_refuses_ambiguous_job_order_unbounded_or_noncanonical_references() {
    let valid = snapshot();
    valid.validate().unwrap();
    let mut duplicate = valid.clone();
    duplicate.jobs.push(duplicate.jobs[0].clone());
    assert!(duplicate.validate().is_err());
    let mut oversized = valid.clone();
    oversized.jobs = vec![valid.jobs[0].clone(); MAX_BROKER_CHECKPOINT_JOBS + 1];
    assert!(oversized.validate().is_err());
    for (field, bad) in [
        ("job_id", json!("10000000-0000-4000-8000-00000000000A")),
        ("revision", json!(u64::MAX)),
        ("checkpoint_hash", json!("A".repeat(64))),
    ] {
        let mut value = serde_json::to_value(&valid).unwrap();
        value["jobs"][0][field] = bad;
        let parsed: BrokerCheckpointSnapshotV1 = serde_json::from_value(value).unwrap();
        assert!(parsed.validate().is_err());
    }
    let mut value = serde_json::to_value(valid).unwrap();
    value["tenant_id"] = json!("tenant-b");
    assert!(serde_json::from_value::<BrokerCheckpointSnapshotV1>(value).is_err());
}

#[test]
fn checkpoint_sequence_is_exact_and_empty_report_never_claims_complete_inventory() {
    let mut publication = BrokerCheckpointV1 {
        version: 1,
        operation_id: "20000000-0000-4000-8000-000000000001".into(),
        incarnation_id: "30000000-0000-4000-8000-000000000001".into(),
        previous_sequence: 0,
        sequence: 1,
        snapshot: snapshot(),
    };
    publication.validate().unwrap();
    publication.sequence = 2;
    assert!(publication.validate().is_err());
    publication.previous_sequence = 1;
    publication.validate().unwrap();
    publication.snapshot.jobs.clear();
    publication.snapshot.has_more = false;
    publication.validate().unwrap();
    assert_eq!(
        serde_json::to_value(BrokerCheckpointCoverageV1::DeclaredPartial).unwrap(),
        json!("declared_partial")
    );
    for field in [
        "tenant_id",
        "broker_id",
        "body",
        "runtime_inventory_complete",
    ] {
        let mut value = serde_json::to_value(&publication).unwrap();
        value[field] = json!(true);
        assert!(serde_json::from_value::<BrokerCheckpointV1>(value).is_err());
    }
}
