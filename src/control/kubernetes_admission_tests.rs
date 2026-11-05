use super::kubernetes_admission::*;
use serde_json::{Value, json};

// Keys are sorted recursively. This literal is also the independent preimage
// used for the intent-hash golden below.
const VALID_INTENT_JSON: &str = r#"{"authority":{"authority_id":"66666666-6666-4666-8666-666666666666","cluster_namespace_uid":"77777777-7777-4777-8777-777777777777","endpoint":"https://kubernetes.fixture.invalid/","namespace":"tenant-a","namespace_uid":"88888888-8888-4888-8888-888888888888","trust_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":1},"broker_id":"44444444-4444-4444-8444-444444444444","control_namespace_uuid":"33333333-3333-4333-8333-333333333333","create_id":"11111111-1111-4111-8111-111111111111","image":"fixture.invalid/worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","incarnation_id":"55555555-5555-4555-8555-555555555555","profile":"kubernetes_held_pod_v1","resources":{"cpu_millis":2000,"deadline_seconds":60,"memory_mib":1024,"temporary_mib":64,"workspace_mib":128},"session_id":"22222222-2222-4222-8222-222222222222","version":1}"#;

fn valid_intent_value() -> Value {
    serde_json::from_str(VALID_INTENT_JSON).unwrap()
}

fn intent_is_accepted(value: Value) -> bool {
    serde_json::from_value::<KubernetesCreateIntentV1>(value)
        .is_ok_and(|intent| intent.validate().is_ok())
}

#[test]
fn kubernetes_create_intent_hash_matches_independent_canonical_golden() {
    let intent: KubernetesCreateIntentV1 = serde_json::from_str(VALID_INTENT_JSON).unwrap();
    intent.validate().unwrap();

    assert_eq!(
        serde_json::to_value(&intent).unwrap(),
        serde_json::from_str::<Value>(VALID_INTENT_JSON).unwrap(),
        "the typed DTO must preserve the reviewed canonical literal"
    );
    assert_eq!(
        intent.intent_hash().unwrap(),
        "5f41364a7d517b3238e55c214de389580bee38b7d8cb28f09322ed924d30fe0e"
    );
}

#[test]
fn kubernetes_create_intent_rejects_invalid_identity_authority_profile_and_bounds() {
    assert!(
        intent_is_accepted(valid_intent_value()),
        "valid intent is the table baseline"
    );
    let mut cases = Vec::new();

    let mut value = valid_intent_value();
    value["version"] = json!(2);
    cases.push(("unsupported version", value));

    let mut value = valid_intent_value();
    value["create_id"] = json!("00000000-0000-0000-0000-000000000000");
    cases.push(("nil create id", value));

    let mut value = valid_intent_value();
    value["session_id"] = json!("22222222222242228222222222222222");
    cases.push(("noncanonical session id", value));

    let mut value = valid_intent_value();
    value["authority"]["endpoint"] = json!("http://kubernetes.fixture.invalid/");
    cases.push(("non-HTTPS authority", value));

    let mut value = valid_intent_value();
    value["image"] = json!("fixture.invalid/worker:mutable");
    cases.push(("mutable image reference", value));

    let mut value = valid_intent_value();
    value["resources"]["cpu_millis"] = json!(0);
    cases.push(("zero cpu allocation", value));

    let mut value = valid_intent_value();
    value["resources"]["workspace_mib"] = json!(1024);
    cases.push(("workspace exceeds memory budget", value));

    let mut value = valid_intent_value();
    value["unrecognized"] = json!(true);
    cases.push(("unknown top-level field", value));

    for (label, value) in cases {
        assert!(!intent_is_accepted(value), "accepted {label}");
    }
}

fn valid_claim_value() -> Value {
    json!({
        "version": 1,
        "operation_id": "99999999-9999-4999-8999-999999999999",
        "create_id": "11111111-1111-4111-8111-111111111111",
        "intent_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "expected_revision": 2,
        "expected_create_epoch": 1,
        "emitter_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    })
}

fn valid_result_value() -> Value {
    json!({
        "version": 1,
        "operation_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "create_id": "11111111-1111-4111-8111-111111111111",
        "intent_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "claim_operation_id": "99999999-9999-4999-8999-999999999999",
        "expected_revision": 2,
        "expected_create_epoch": 1,
        "outcome": {"kind":"ownership_unknown","reason":"transport_unknown","http_status":null}
    })
}

fn valid_cancel_value() -> Value {
    json!({
        "version": 1,
        "operation_id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "create_id": "11111111-1111-4111-8111-111111111111",
        "intent_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "expected_revision": 2,
        "expected_create_epoch": 1
    })
}

fn valid_receipt_value() -> Value {
    json!({
        "version": 1,
        "operation_id": "99999999-9999-4999-8999-999999999999",
        "input_hash": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "control_namespace_uuid": "33333333-3333-4333-8333-333333333333",
        "audit_seq": 7,
        "audit_hash": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "emission_window": null
    })
}

#[test]
fn kubernetes_create_mutations_and_receipts_reject_bad_hash_ids_cas_and_windows() {
    let claim: KubernetesCreateClaimV1 = serde_json::from_value(valid_claim_value()).unwrap();
    claim.validate().unwrap();
    let result: KubernetesCreateResultV1 = serde_json::from_value(valid_result_value()).unwrap();
    result.validate().unwrap();
    let cancel: KubernetesCreateCancelV1 = serde_json::from_value(valid_cancel_value()).unwrap();
    cancel.validate().unwrap();
    let receipt: KubernetesCreateReceiptV1 = serde_json::from_value(valid_receipt_value()).unwrap();
    receipt.validate().unwrap();
    let mut valid_window = valid_receipt_value();
    valid_window["emission_window"] = json!({
        "issued_at":"2026-11-05T12:00:00.000Z",
        "not_after":"2026-11-05T12:00:08.000Z"
    });
    let valid_window: KubernetesCreateReceiptV1 = serde_json::from_value(valid_window).unwrap();
    valid_window
        .validate()
        .expect("eight-second emission window is valid");

    let mut invalid_claims = Vec::new();
    let mut value = valid_claim_value();
    value["intent_hash"] = json!("not-a-sha256");
    invalid_claims.push(("malformed claim intent hash", value));
    let mut value = valid_claim_value();
    value["operation_id"] = json!("00000000-0000-0000-0000-000000000000");
    invalid_claims.push(("nil claim operation id", value));
    let mut value = valid_claim_value();
    value["expected_revision"] = json!(0);
    invalid_claims.push(("zero claim revision", value));
    let mut value = valid_claim_value();
    value["expected_create_epoch"] = json!(0);
    invalid_claims.push(("zero claim epoch", value));
    for (label, value) in invalid_claims {
        let request: KubernetesCreateClaimV1 = serde_json::from_value(value).unwrap();
        assert!(request.validate().is_err(), "accepted {label}");
    }

    let mut invalid_results = Vec::new();
    let mut value = valid_result_value();
    value["claim_operation_id"] = json!("not-a-uuid");
    invalid_results.push(("malformed result claim id", value));
    let mut value = valid_result_value();
    value["expected_revision"] = json!(0);
    invalid_results.push(("zero result revision", value));
    let mut value = valid_result_value();
    value["expected_create_epoch"] = json!(0);
    invalid_results.push(("zero result epoch", value));
    let mut value = valid_result_value();
    value["outcome"]["http_status"] = json!(503);
    invalid_results.push(("HTTP status on transport unknown", value));
    for (label, value) in invalid_results {
        let request: KubernetesCreateResultV1 = serde_json::from_value(value).unwrap();
        assert!(request.validate().is_err(), "accepted {label}");
    }

    let mut invalid_cancels = Vec::new();
    let mut value = valid_cancel_value();
    value["operation_id"] = json!("not-a-uuid");
    invalid_cancels.push(("malformed cancel operation id", value));
    let mut value = valid_cancel_value();
    value["expected_revision"] = json!(0);
    invalid_cancels.push(("zero cancel revision", value));
    let mut value = valid_cancel_value();
    value["expected_create_epoch"] = json!(0);
    invalid_cancels.push(("zero cancel epoch", value));
    for (label, value) in invalid_cancels {
        let request: KubernetesCreateCancelV1 = serde_json::from_value(value).unwrap();
        assert!(request.validate().is_err(), "accepted {label}");
    }

    let mut invalid_receipts = Vec::new();
    let mut value = valid_receipt_value();
    value["input_hash"] = json!("not-a-sha256");
    invalid_receipts.push(("malformed receipt input hash", value));
    let mut value = valid_receipt_value();
    value["control_namespace_uuid"] = json!("00000000-0000-0000-0000-000000000000");
    invalid_receipts.push(("nil receipt namespace", value));
    let mut value = valid_receipt_value();
    value["audit_seq"] = json!(0);
    invalid_receipts.push(("nonpositive receipt audit sequence", value));
    let mut value = valid_receipt_value();
    value["audit_hash"] = json!("not-a-sha256");
    invalid_receipts.push(("malformed receipt audit hash", value));
    let mut value = valid_receipt_value();
    value["emission_window"] = json!({
        "issued_at":"2026-11-05T12:00:00.000Z",
        "not_after":"2026-11-05T12:00:09.000Z"
    });
    invalid_receipts.push(("window exceeds eight seconds", value));
    for (label, value) in invalid_receipts {
        let request: KubernetesCreateReceiptV1 = serde_json::from_value(value).unwrap();
        assert!(request.validate().is_err(), "accepted {label}");
    }
}
