use super::*;
use crate::broker::service::protocol::{BrokerCommandV1, BrokerReplyV1};

#[test]
fn recovery_inventory_is_additive_and_preserves_existing_command_bytes() {
    let id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    for (command, expected) in [
        (
            BrokerCommandV1::List {
                after: Some(id),
                limit: 20,
            },
            r#"{"list":{"after":"11111111-1111-4111-8111-111111111111","limit":20}}"#,
        ),
        (
            BrokerCommandV1::RecoveryStatus { operation_id: id },
            r#"{"recovery_status":{"operation_id":"11111111-1111-4111-8111-111111111111"}}"#,
        ),
    ] {
        assert_eq!(serde_json::to_string(&command).unwrap(), expected);
        assert_eq!(
            serde_json::to_string(&serde_json::from_str::<BrokerCommandV1>(expected).unwrap())
                .unwrap(),
            expected
        );
    }
    let command = r#"{"recovery_inventory":{"after":null,"limit":20}}"#;
    assert!(matches!(
        serde_json::from_str::<BrokerCommandV1>(command).unwrap(),
        BrokerCommandV1::RecoveryInventory {
            after: None,
            limit: 20
        }
    ));
    for invalid in [
        r#"{"recovery_inventory":{"after":null,"limit":20,"tenant_id":"other"}}"#,
        r#"{"recovery_inventory":{"after":null,"limit":20,"restore":true}}"#,
        r#"{"recovery_inventory":{"after":"invalid","limit":20}}"#,
    ] {
        assert!(serde_json::from_str::<BrokerCommandV1>(invalid).is_err());
    }
    let page = BrokerReplyV1::Page {
        entries: vec![],
        next_after: None,
    };
    assert_eq!(
        serde_json::to_string(&page).unwrap(),
        r#"{"page":{"entries":[],"next_after":null}}"#
    );
}

#[test]
fn recovery_inventory_closed_reply_cannot_include_runtime_or_business_payloads() {
    let base = serde_json::json!({
        "version":1,"tenant_id":"tenant-fixture",
        "control_namespace_uuid":"11111111-1111-4111-8111-111111111111",
        "profile_id":"profile-fixture","journal_mode":"stop_only",
        "source":"confirmed_journal","coverage":"accepted_stop_metadata",
        "runtime_evidence":"not_queried","incomplete":false,
        "reconciliation_required":true,"entries":[],"issues":[],
        "issues_truncated":false,"next_after":null
    });
    assert!(serde_json::from_value::<RecoveryInventoryV1>(base.clone()).is_ok());
    for field in ["prompt", "credential", "runtime_status", "quota_held"] {
        let mut unknown = base.clone();
        unknown[field] = serde_json::json!("synthetic-fixture");
        assert!(serde_json::from_value::<RecoveryInventoryV1>(unknown).is_err());
    }
    let mut unknown_source = base;
    unknown_source["source"] = serde_json::json!("runtime");
    assert!(serde_json::from_value::<RecoveryInventoryV1>(unknown_source).is_err());
}
