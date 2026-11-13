use super::*;

fn input<'a>(
    tenant: &'a str,
    role: TechnicalCredentialRoleV1,
    token: &'a str,
    id: u128,
) -> TechnicalCredentialInput<'a> {
    TechnicalCredentialInput {
        tenant_id: tenant,
        role,
        token,
        credential_id: Uuid::from_u128(id),
        generation: 1,
    }
}

#[test]
fn pure_configuration_checks_headers_roles_tokens_and_stable_identity() {
    let tenants = vec!["a".to_string()];
    let inputs = [
        input("a", TechnicalCredentialRoleV1::Runner, "runner-fixture", 3),
        input("a", TechnicalCredentialRoleV1::Viewer, "viewer-fixture", 4),
    ];
    let expected = EnvelopeExpectationsV1::from_configured_inputs(
        Uuid::from_u128(1),
        Uuid::from_u128(2),
        1,
        "a".repeat(64),
        &tenants,
        &inputs,
    )
    .unwrap();
    let bytes = prepare_envelope_configuration(&expected, &inputs).unwrap();
    assert!(validate_envelope_configuration(&bytes, &expected).is_ok());
    for (key, value) in [
        ("version", serde_json::json!(2)),
        ("closure_revision", serde_json::json!(2)),
        ("installation_id", serde_json::json!(Uuid::from_u128(7))),
    ] {
        let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        changed[key] = value;
        assert!(
            validate_envelope_configuration(&serde_json::to_vec(&changed).unwrap(), &expected)
                .is_err()
        );
    }
    for (key, value) in [
        ("generation", serde_json::json!(2)),
        ("credential_id", serde_json::json!(Uuid::from_u128(7))),
        ("token", serde_json::json!("another-token")),
        ("role", serde_json::json!("viewer")),
    ] {
        let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        changed["entries"][0][key] = value;
        assert!(
            validate_envelope_configuration(&serde_json::to_vec(&changed).unwrap(), &expected)
                .is_err()
        );
    }
}

#[test]
fn pure_configuration_rejects_duplicate_and_unknown_json_before_authority() {
    let tenants = vec!["a".to_string()];
    let inputs = [
        input("a", TechnicalCredentialRoleV1::Runner, "runner-fixture", 3),
        input("a", TechnicalCredentialRoleV1::Viewer, "viewer-fixture", 4),
    ];
    let expected = EnvelopeExpectationsV1::from_configured_inputs(
        Uuid::from_u128(1),
        Uuid::from_u128(2),
        1,
        "a".repeat(64),
        &tenants,
        &inputs,
    )
    .unwrap();
    let bytes = prepare_envelope_configuration(&expected, &inputs).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let duplicate = text.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert!(validate_envelope_configuration(duplicate.as_bytes(), &expected).is_err());
    let unknown = text.replacen("\"version\":1", "\"version\":1,\"secret_unknown\":true", 1);
    assert!(validate_envelope_configuration(unknown.as_bytes(), &expected).is_err());
    assert!(
        validate_envelope_configuration(&vec![b' '; MAX_ENVELOPE_BYTES + 1], &expected).is_err()
    );
}
