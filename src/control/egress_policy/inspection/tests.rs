use super::*;
use serde_json::json;

fn request(selector: EgressPolicySelectorV1) -> EgressPolicyInspectionRequestV1 {
    EgressPolicyInspectionRequestV1::new(selector).unwrap()
}
fn tenant_observation() -> EgressPolicyInspectionV1 {
    EgressPolicyInspectionV1::observed(
        EgressPolicyExpectedScopeV1::new("A", "00000000-0000-0000-0000-000000000048").unwrap(),
        EgressPolicySelectorV1::Tenant,
        EgressPolicyObservedHeadV1::Absent,
        EgressPolicyObservedCeilingV1::NotApplicable,
        EgressPolicyObservedSessionResourceV1::NotApplicable,
        "2026-11-14T00:00:00.000Z".into(),
    ).unwrap()
}
fn session_observation() -> EgressPolicyInspectionV1 {
    EgressPolicyInspectionV1::observed(
        tenant_observation().scope().clone(),
        EgressPolicySelectorV1::session("00000000-0000-0000-0000-000000000049").unwrap(),
        EgressPolicyObservedHeadV1::Present { revision: 3, epoch: 4, state: EgressPolicyStateV1::RevokeRequested },
        EgressPolicyObservedCeilingV1::Active { revision: 2, epoch: 3, digest: "ab".repeat(32) },
        EgressPolicyObservedSessionResourceV1::Inactive,
        "2026-11-14T00:00:00.000Z".into(),
    ).unwrap()
}

#[test]
fn inspection_request_cannot_choose_tenant_scope_actor_or_noncanonical_session() {
    let bytes = request(EgressPolicySelectorV1::Tenant).canonical_bytes().unwrap();
    assert_eq!(bytes, br#"{"version":1,"selector":{"kind":"tenant"}}"#);
    for field in ["tenant_id", "expected_scope", "source_id", "principal", "actor", "clock", "operation_id"] {
        let mut value = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        value[field] = json!("untrusted");
        assert_eq!(EgressPolicyInspectionRequestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap_err(), E::InvalidRequest);
    }
    for selector in [
        json!({"kind":"tenant","session_id":null}),
        json!({"kind":"tenant","policy":{}}),
        json!({"kind":"session","session_id":"00000000000000000000000000000049"}),
        json!({"kind":"session","session_id":"00000000-0000-0000-0000-000000000000"}),
        json!({"kind":"session","session_id":"00000000-0000-0000-0000-000000000049","tenant_id":"B"}),
    ] {
        assert!(EgressPolicyInspectionRequestV1::from_json(&serde_json::to_vec(&json!({"version":1,"selector":selector})).unwrap()).is_err());
    }
    assert!(EgressPolicyInspectionRequestV1::from_json(&vec![b' '; REQUEST_BYTES + 1]).is_err());
}

#[test]
fn absent_and_inactive_unions_reject_hidden_policy_or_ceiling_data() {
    let good = tenant_observation();
    for (field, value) in [
        ("head", json!({"presence":"absent","revision":1})),
        ("head", json!({"presence":"present","revision":1,"epoch":1,"state":"active","policy":{}})),
        ("tenant_ceiling", json!({"status":"not_applicable","digest":"ab".repeat(32)})),
        ("tenant_ceiling", json!({"status":"absent","policy":{}})),
        ("tenant_ceiling", json!({"status":"inactive","revision":1,"epoch":2,"digest":"ab".repeat(32)})),
        ("session_resource", json!({"status":"not_applicable"})),
        ("selector", json!({"kind":"tenant","session_id":null})),
    ] {
        let mut raw = serde_json::to_value(&good).unwrap();
        raw[field] = value;
        assert!(serde_json::from_value::<EgressPolicyInspectionV1>(raw).is_err(), "{field}");
    }
    let mut raw = serde_json::to_value(good).unwrap();
    raw["policy"] = json!({"destinations":[]});
    assert!(serde_json::from_value::<EgressPolicyInspectionV1>(raw).is_err());
}

#[test]
fn inspection_response_validates_scope_time_bounds_and_selector_cross_fields() {
    let tenant = tenant_observation();
    let session = session_observation();
    for (base, pointer, value) in [
        (&tenant, "/schema_version", json!(14)),
        (&tenant, "/historical", json!(false)),
        (&tenant, "/observed_at", json!("2026-11-14T00:00:00Z")),
        (&tenant, "/scope/control_namespace_uuid", json!("00000000000000000000000000000048")),
        (&tenant, "/scope/tenant_id", json!("../A")),
        (&tenant, "/session_resource", json!("active")),
        (&tenant, "/tenant_ceiling", json!({"status":"absent"})),
        (&session, "/session_resource", json!("not_applicable")),
        (&session, "/tenant_ceiling", json!({"status":"not_applicable"})),
        (&session, "/head/revision", json!(0)),
        (&session, "/head/epoch", json!(u64::MAX)),
        (&session, "/tenant_ceiling/digest", json!("AB".repeat(32))),
        (&session, "/tenant_ceiling/revision", json!(0)),
        (&session, "/tenant_ceiling/epoch", json!(u64::MAX)),
    ] {
        let mut raw = serde_json::to_value(base).unwrap();
        *raw.pointer_mut(pointer).unwrap() = value;
        let parsed: EgressPolicyInspectionV1 = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.canonical_bytes().unwrap_err(), E::InvalidResponse, "{pointer}");
    }
    let same = request(session.selector().clone());
    session.validate_for(&same).unwrap();
    assert_eq!(session.validate_for(&request(EgressPolicySelectorV1::Tenant)), Err(E::InvalidResponse));
    assert_eq!(tenant.validate_for(&same), Err(E::InvalidResponse));
}

#[test]
fn canonical_inspection_is_bounded_historical_metadata_with_exact_resource_wire() {
    let tenant = tenant_observation();
    let bytes = tenant.canonical_bytes().unwrap();
    assert_eq!(bytes, br#"{"version":1,"schema_version":15,"scope":{"tenant_id":"A","control_namespace_uuid":"00000000-0000-0000-0000-000000000048"},"selector":{"kind":"tenant"},"head":{"presence":"absent"},"tenant_ceiling":{"status":"not_applicable"},"session_resource":"not_applicable","observed_at":"2026-11-14T00:00:00.000Z","coverage":"policy_registry_metadata","historical":true}"#);
    assert_eq!(EgressPolicyInspectionV1::from_canonical_bytes(&bytes).unwrap(), tenant);
    let mut padded = bytes.clone();
    padded.push(b' ');
    assert_eq!(EgressPolicyInspectionV1::from_canonical_bytes(&padded).unwrap_err(), E::InvalidResponse);
    assert_eq!(EgressPolicyInspectionV1::from_canonical_bytes(&vec![b' '; RECEIPT_BYTES + 1]).unwrap_err(), E::InvalidResponse);
    let session = session_observation();
    assert_eq!(session.session_resource(), EgressPolicyObservedSessionResourceV1::Inactive);
    assert!(session.historical());
    assert_eq!(session.coverage(), "policy_registry_metadata");
    for forbidden in ["destinations", "name", "port", "source_id", "binding", "credential", "actor", "policy_digest"] {
        assert!(!String::from_utf8(session.canonical_bytes().unwrap()).unwrap().contains(forbidden));
    }
}
