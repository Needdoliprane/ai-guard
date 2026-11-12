//! Parser/index tests only. These fixtures confer no filesystem, Store, marker,
//! incarnation, gate or approval authority. The retained-file constructor and
//! authenticated HTTP path still require the composed modern-control fixture.
use super::super::CredentialBinding;
use super::*;
use serde_json::{Value, json};

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn fixture() -> (HumanAuthorityClosureV1, CredentialIndex, Vec<String>, Value) {
    let source = |tenant: &str, n: u128| {
        json!({
            "source_id":id(n),"kind":"tenant_store","installation_id":id(1),
            "durable_namespace_id":id(n+10),"tenant_id":tenant,
            "storage_binding":{"parent_path":format!("/scratch/parser-only/{tenant}"),
                "file_name":"audit.db","parent_device":1,"parent_inode":n+20,
                "source_device":1,"source_inode":n+30},
            "required_format":14,"authority_marker_receipt":{"seq":1,"hash":"a".repeat(64)}
        })
    };
    let closure: HumanAuthorityClosureV1 = serde_json::from_value(json!({
        "version":1,"installation_id":id(1),"closure_id":id(2),"closure_revision":3,
        "closure_digest":"b".repeat(64),"sources":[source("a",3),source("b",4)],
        "configuration":{"control_config_revision":1,"public_config_digest":"c".repeat(64),
            "oidc":null,"files":[]},"supported_storage_profile":"local_posix_flock_v1"
    }))
    .unwrap();
    let mut index = CredentialIndex::default();
    let mut entries = Vec::new();
    for (tenant_index, tenant) in ["a", "b"].into_iter().enumerate() {
        for (offset, role) in [CredentialRole::Runner, CredentialRole::Viewer]
            .into_iter()
            .enumerate()
        {
            let token = format!("synthetic-{tenant}-{}", role.as_str());
            index.entries.insert(
                credential_digest(&token),
                CredentialBinding { tenant_index, role },
            );
            entries.push(json!({"tenant_id":tenant,"role":role.as_str(),
                "credential_id":id(100+tenant_index as u128*2+offset as u128),
                "generation":1,"token":token}));
        }
    }
    let envelope = json!({"version":1,"installation_id":id(1),"closure_id":id(2),
        "closure_revision":3,"control_config_public_digest":"c".repeat(64),"entries":entries});
    (closure, index, vec!["a".into(), "b".into()], envelope)
}

#[test]
fn private_envelope_resolves_only_the_exact_configured_runner_entries() {
    let (closure, index, tenants, envelope) = fixture();
    let resolved = resolve_envelope(
        &serde_json::to_vec(&envelope).unwrap(),
        &index,
        &tenants,
        &closure,
    )
    .unwrap();
    assert_eq!(resolved.len(), 2);
    let runner = resolved
        .get(&credential_digest("synthetic-a-runner"))
        .unwrap();
    assert_eq!(runner.tenant_id, "a");
    assert_eq!(runner.source_id, id(3));
    assert_eq!(runner.namespace, id(13));
    assert_eq!(runner.credential_id, id(100));
    assert_eq!(runner.generation, 1);
    assert!(!resolved.contains_key(&credential_digest("synthetic-a-viewer")));
}

#[test]
fn missing_extra_or_cross_role_credentials_cannot_create_an_index() {
    let (closure, index, tenants, envelope) = fixture();
    for mutation in 0..4 {
        let mut changed = envelope.clone();
        match mutation {
            0 => {
                changed["entries"].as_array_mut().unwrap().pop();
            }
            1 => {
                let extra = changed["entries"][0].clone();
                changed["entries"].as_array_mut().unwrap().push(extra);
            }
            2 => {
                changed["entries"][0]["tenant_id"] = json!("b");
            }
            _ => {
                changed["entries"][0]["role"] = json!("viewer");
            }
        }
        assert!(
            resolve_envelope(
                &serde_json::to_vec(&changed).unwrap(),
                &index,
                &tenants,
                &closure
            )
            .is_err()
        );
    }
}

#[test]
fn stable_credential_ids_are_unique_across_tenants_and_roles() {
    let (closure, index, tenants, mut envelope) = fixture();
    let first = envelope["entries"][0]["credential_id"].clone();
    envelope["entries"][3]["credential_id"] = first;
    assert!(
        resolve_envelope(
            &serde_json::to_vec(&envelope).unwrap(),
            &index,
            &tenants,
            &closure
        )
        .is_err()
    );
    envelope["entries"][3]["credential_id"] = json!(id(103));
    envelope["entries"][0]["generation"] = json!(0);
    assert!(
        resolve_envelope(
            &serde_json::to_vec(&envelope).unwrap(),
            &index,
            &tenants,
            &closure
        )
        .is_err()
    );
}

#[test]
fn closure_identity_revision_and_public_configuration_are_all_bound() {
    let (closure, index, tenants, envelope) = fixture();
    for (key, value) in [
        ("installation_id", json!(id(900))),
        ("closure_id", json!(id(901))),
        ("closure_revision", json!(4)),
        ("control_config_public_digest", json!("d".repeat(64))),
        ("version", json!(2)),
    ] {
        let mut changed = envelope.clone();
        changed[key] = value;
        assert!(
            resolve_envelope(
                &serde_json::to_vec(&changed).unwrap(),
                &index,
                &tenants,
                &closure
            )
            .is_err(),
            "{key}"
        );
    }
}

#[test]
fn private_values_and_parser_errors_never_appear_in_diagnostics() {
    let (closure, index, tenants, mut envelope) = fixture();
    let private = "synthetic-private-credential-that-must-not-be-rendered";
    envelope["entries"][0]["token"] = json!(private);
    envelope["private_unknown_field"] = json!(private);
    let failure = resolve_envelope(
        &serde_json::to_vec(&envelope).unwrap(),
        &index,
        &tenants,
        &closure,
    )
    .err()
    .unwrap();
    assert!(!format!("{failure:?} {failure}").contains(private));
    for bytes in [
        Vec::new(),
        vec![b'x'; MAX_ENVELOPE_BYTES + 1],
        b"{malformed".to_vec(),
    ] {
        assert!(resolve_envelope(&bytes, &index, &tenants, &closure).is_err());
    }
}

#[test]
fn every_tenant_needs_one_exact_modern_source_and_no_duplicate_scope() {
    let (mut closure, index, tenants, envelope) = fixture();
    let bytes = serde_json::to_vec(&envelope).unwrap();
    closure.sources[0].required_format = 13;
    assert!(resolve_envelope(&bytes, &index, &tenants, &closure).is_err());
    closure.sources[0].required_format = 14;
    let extra = closure.sources[0].clone();
    closure.sources.push(extra);
    assert!(resolve_envelope(&bytes, &index, &tenants, &closure).is_err());
}
