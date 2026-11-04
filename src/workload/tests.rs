use super::*;

#[test]
fn workload_inputs_cannot_choose_a_tenant_or_contain_a_plaintext_secret() {
    let issue = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        credential_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
        },
        sha256_digest: "a".repeat(64),
        expires_at: "2100-01-01T00:00:00.000Z".into(),
    };
    issue.validate().unwrap();
    for field in ["tenant_id", "token", "runner_token", "role"] {
        let mut json = serde_json::to_value(&issue).unwrap();
        json[field] = serde_json::json!("forged");
        assert!(serde_json::from_value::<WorkloadBindingIssueV1>(json).is_err());
    }
    for digest in ["A".repeat(64), "a".repeat(63), "raw-secret".into()] {
        let mut invalid = issue.clone();
        invalid.sha256_digest = digest;
        assert!(invalid.validate().is_err());
    }
    let unknown = serde_json::json!({"kind":"runner","worker_id":uuid::Uuid::new_v4().to_string()});
    assert!(serde_json::from_value::<WorkloadSubjectV1>(unknown).is_err());
    for generation in [0, 2, u64::MAX] {
        let subject = WorkloadSubjectV1::Worker {
            worker_id: uuid::Uuid::new_v4().to_string(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            generation,
        };
        assert!(
            subject.validate().is_err(),
            "v1 has no generation transition"
        );
    }
}
