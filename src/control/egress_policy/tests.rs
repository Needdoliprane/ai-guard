use super::*;
use serde_json::json;

fn request() -> EgressReplacePolicyRequestV1 {
    EgressReplacePolicyRequestV1::new(
        Uuid::from_u128(71),
        EgressPolicyExpectedScopeV1::new("A", &Uuid::from_u128(72).to_string()).unwrap(),
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        EgressPolicyInputV1::new(
            1,
            1000,
            &[Authority::new("example.com", 443, EgressTransport::ConnectTcpOpaque).unwrap()],
            PolicyLimits::new(1, 1).unwrap(),
        )
        .unwrap(),
        None,
    )
    .unwrap()
}
fn receipt(request: &EgressReplacePolicyRequestV1) -> EgressPolicyReceiptV1 {
    serde_json::from_value(json!({
        "version":1,"operation_id":request.operation_id(),"action":"replace","selector":{"kind":"tenant"},
        "revision":1,"policy_digest":hex::encode(request.policy().to_policy(1).unwrap().digest()),
        "before_epoch":0,"after_epoch":1,"state":"active","event_id":request.operation_id().to_string(),
        "event_seq":10,"event_hash":"ab".repeat(32),"received_at":"2026-11-13T00:00:00.000Z"
    })).unwrap()
}
#[test]
fn scope_is_required_closed_and_never_a_tenant_selector() {
    let request = request();
    let bytes = request.canonical_bytes().unwrap();
    assert_eq!(
        EgressReplacePolicyRequestV1::from_json(&bytes).unwrap(),
        request
    );
    for field in ["expected_scope", "operation_id"] {
        let mut value = serde_json::to_value(&request).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert_eq!(
            EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap())
                .unwrap_err(),
            EgressPolicyAdminErrorV1::InvalidRequest
        );
    }
    for value in [
        json!(null),
        json!({"tenant_id":"A"}),
        json!({"tenant_id":"A","control_namespace_uuid":Uuid::new_v4(),"authority":true}),
    ] {
        let mut input = serde_json::to_value(&request).unwrap();
        input["expected_scope"] = value;
        assert!(
            EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&input).unwrap()).is_err()
        );
    }
}
#[test]
fn closed_bounds_precede_core_clone_and_empty_deny_all_is_preserved() {
    let request = request();
    let mut value = serde_json::to_value(&request).unwrap();
    value["policy"]["destinations"] = json!([]);
    assert!(EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_ok());
    value["policy"]["destinations"] = json!((0..129).map(|i|json!({"name":format!("d{i}.example.test"),"port":443,"transport":"connect_tcp_opaque"})).collect::<Vec<_>>());
    assert!(EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(&request).unwrap();
    value["policy"]["destinations"][0]["name"] = json!("x".repeat(254));
    assert!(EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(&request).unwrap();
    value["policy"]["max_resolution_ips"] = json!(17);
    assert!(EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(&request).unwrap();
    value["actor"] = json!("human");
    assert!(EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(EgressReplacePolicyRequestV1::from_json(&vec![b' '; REQUEST_BYTES + 1]).is_err());
}
#[test]
fn replace_digest_is_correlated_but_revoke_only_validates_its_shape() {
    let request = request();
    let good = receipt(&request);
    let bytes = good.canonical_bytes().unwrap();
    EgressPolicyReceiptV1::from_canonical_bytes(&bytes)
        .unwrap()
        .validate_for_replace(&request)
        .unwrap();
    let mut value = serde_json::to_value(&good).unwrap();
    value["policy_digest"] = json!("cd".repeat(32));
    let wrong: EgressPolicyReceiptV1 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        wrong.validate_for_replace(&request),
        Err(EgressPolicyAdminErrorV1::InvalidResponse)
    );
    let revoke = EgressRevokePolicyRequestV1::new(
        request.operation_id(),
        request.expected_scope().clone(),
        EgressPolicySelectorV1::Tenant,
        1,
        1,
    )
    .unwrap();
    value["action"] = json!("revoke");
    value["state"] = json!("revoke_requested");
    value["before_epoch"] = json!(1);
    value["after_epoch"] = json!(2);
    let changed: EgressPolicyReceiptV1 = serde_json::from_value(value.clone()).unwrap();
    changed.validate_for_revoke(&revoke).unwrap();
    value["policy_digest"] = json!("not-a-digest");
    let malformed: EgressPolicyReceiptV1 = serde_json::from_value(value).unwrap();
    assert_eq!(
        malformed.validate_for_revoke(&revoke),
        Err(EgressPolicyAdminErrorV1::InvalidResponse)
    );
}
#[test]
fn canonical_receipt_and_uuid_forms_remain_closed() {
    let request = request();
    let good = receipt(&request);
    let mut bytes = good.canonical_bytes().unwrap();
    bytes.push(b' ');
    assert_eq!(
        EgressPolicyReceiptV1::from_canonical_bytes(&bytes).unwrap_err(),
        EgressPolicyAdminErrorV1::InvalidResponse
    );
    let mut value = serde_json::to_value(&request).unwrap();
    value["operation_id"] = json!(request.operation_id().simple().to_string());
    assert!(EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(&good).unwrap();
    value["fresh"] = json!(true);
    assert!(serde_json::from_value::<EgressPolicyReceiptV1>(value).is_err());
}
