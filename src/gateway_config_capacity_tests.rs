//! Private configuration and real authentication SQLite only; no IdP request.
#![cfg(unix)]

use super::tests::oidc_fixture;
use super::*;
use agentsandbox::{
    audit::authentication::{
        AuthenticationActionV1, AuthenticationAudit, AuthenticationAuditPolicyV1,
        AuthenticationCapacityPlanV1, AuthenticationCapacityV1, AuthenticationOutcomeV1,
        AuthenticationPhaseV1, AuthenticationReasonV1, AuthenticationRecordV1,
        AuthenticationTenantCapacityV1, AuthenticationTokenFingerprintV1,
    },
    identity::PrincipalRefV1,
    redaction::Redactor,
};
use serde_json::{Value, json};
use std::fs;

fn capacity_plan(policy: &AuthenticationAuditPolicyV1) -> AuthenticationCapacityPlanV1 {
    // Use the fixture's real immutable G; three eighth-shares leave space for
    // genesis, the plan entry and the small historical prefix used below.
    let capacity = AuthenticationCapacityV1 {
        max_records: policy.max_records / 8,
        max_serialized_bytes: policy.max_serialized_bytes / 8,
    };
    assert!(capacity.max_records >= 4, "fixture record policy too small");
    assert!(
        capacity.max_serialized_bytes >= 65_536,
        "fixture byte policy too small"
    );
    let plan = AuthenticationCapacityPlanV1 {
        version: 1,
        unscoped: capacity.clone(),
        tenants: ["tenant-a", "tenant-b"]
            .map(|tenant_id| AuthenticationTenantCapacityV1 {
                tenant_id: tenant_id.into(),
                capacity: capacity.clone(),
            })
            .to_vec(),
    };
    plan.validate().unwrap();
    plan
}

fn document(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_document(path: &Path, value: &Value) {
    // The existing fixture created this file0600; replacing its bytes keeps
    // that inode and mode. No policy field is edited by these helpers.
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn configure_v2(path: &Path, plan: &AuthenticationCapacityPlanV1) {
    let mut value = document(path);
    value["version"] = json!(2);
    value["authentication_audit"]["capacity_plan"] = serde_json::to_value(plan).unwrap();
    write_document(path, &value);
}

fn open_audit(directory: &Path, config: &OidcFile) -> AuthenticationAudit {
    AuthenticationAudit::open_existing(
        &directory.join(&config.authentication_audit.path),
        config.authentication_audit.installation_id,
        config.authentication_audit.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap()
}

fn directory_entries(directory: &Path) -> BTreeSet<std::ffi::OsString> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[tokio::test]
async fn version_two_configuration_initializes_v4_and_reopens_the_exact_plan() {
    let (directory, path, config) = oidc_fixture();
    let plan = capacity_plan(&config.authentication_audit.policy);
    let policy_before = document(&path)["authentication_audit"]["policy"].clone();
    configure_v2(&path, &plan);

    let gateway = load_oidc(&path, true, Redactor::new(vec![]))
        .expect("explicit version2 configuration must initialize a real V4 audit");
    drop(gateway);
    let audit = open_audit(directory.path(), &config);
    assert_eq!(audit.format_version().unwrap(), 4);
    assert_eq!(audit.capacity_plan().unwrap(), Some(plan.clone()));
    let before = audit.verify().unwrap();
    assert_eq!(before.records, 2, "real genesis plus the durable plan");
    drop(audit);

    let gateway = load_oidc(&path, false, Redactor::new(vec![]))
        .expect("normal reopen must accept the exact durable plan");
    drop(gateway);
    let reopened = open_audit(directory.path(), &config);
    assert_eq!(reopened.verify().unwrap(), before);
    assert_eq!(reopened.capacity_plan().unwrap(), Some(plan));
    assert_eq!(
        document(&path)["authentication_audit"]["policy"],
        policy_before,
        "configuration must retain the fixture's original global policy"
    );
}

#[tokio::test]
async fn changed_valid_capacity_plan_is_refused_without_mutating_the_ledger() {
    let (directory, path, config) = oidc_fixture();
    let plan = capacity_plan(&config.authentication_audit.policy);
    configure_v2(&path, &plan);
    drop(load_oidc(&path, true, Redactor::new(vec![])).unwrap());
    let audit = open_audit(directory.path(), &config);
    let before = audit.verify().unwrap();
    drop(audit);

    let mut changed = plan.clone();
    changed.tenants[0].capacity.max_records -= 1;
    changed.validate().unwrap();
    configure_v2(&path, &changed);
    assert!(
        load_oidc(&path, false, Redactor::new(vec![])).is_err(),
        "a valid but different configured plan cannot replace the durable plan"
    );
    let unchanged = open_audit(directory.path(), &config);
    assert_eq!(unchanged.format_version().unwrap(), 4);
    assert_eq!(unchanged.verify().unwrap(), before);
    assert_eq!(unchanged.capacity_plan().unwrap(), Some(plan.clone()));
    drop(unchanged);

    configure_v2(&path, &plan);
    drop(load_oidc(&path, false, Redactor::new(vec![])).unwrap());
    assert_eq!(
        open_audit(directory.path(), &config).verify().unwrap(),
        before
    );
}

#[tokio::test]
async fn explicit_v3_to_v4_migration_requires_cfg2_and_preserves_history_and_new_backup() {
    let (directory, path, config) = oidc_fixture();
    let plan = capacity_plan(&config.authentication_audit.policy);
    let policy_before = document(&path)["authentication_audit"]["policy"].clone();
    drop(load_oidc(&path, true, Redactor::new(vec![])).unwrap());
    let audit = open_audit(directory.path(), &config);
    assert_eq!(audit.format_version().unwrap(), 3);
    let operation = uuid::Uuid::new_v4();
    let principal = PrincipalRefV1::HumanOidc {
        issuer: config.gateway.provider.issuer.clone(),
        subject: "synthetic-capacity-config-member".into(),
    };
    let intention = AuthenticationRecordV1 {
        operation_id: operation,
        action: AuthenticationActionV1::Login,
        phase: AuthenticationPhaseV1::Intention,
        issuer: config.gateway.provider.issuer.clone(),
        client_id: config.gateway.provider.web_client_id.clone(),
        principal: None,
        transaction_id: Some(operation),
        session_id: Some(uuid::Uuid::new_v4()),
        causal: None,
    };
    // Real V3 storage APIs create an actual historical receipt. This test
    // makes no claim that an OIDC provider verified the synthetic principal.
    let pending = audit
        .begin_verified_initial(
            intention.clone(),
            principal.clone(),
            AuthenticationTokenFingerprintV1::access_token("synthetic-config-capacity-proof"),
        )
        .unwrap();
    assert!(pending.inserted);
    let terminal = AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Rejected,
            reason: AuthenticationReasonV1::NoMembership,
        },
        principal: Some(principal),
        causal: Some(pending.receipt),
        ..intention
    };
    let historical_receipt = audit.append_typed(terminal.clone()).unwrap();
    let before = audit.verify().unwrap();
    assert_eq!(before.records, 3);
    assert_eq!(before.head, historical_receipt);
    let occupied = directory.path().join("existing-v3-backup.sqlite");
    assert_eq!(audit.backup_new(&occupied).unwrap(), before);
    drop(audit);

    let backup = directory.path().join("before-capacity-v4.sqlite");
    assert!(
        migrate_authentication(&path, &backup, 4, Redactor::new(vec![])).is_err(),
        "target4 requires configuration2 with its explicit plan"
    );
    assert!(!backup.exists());
    assert_eq!(
        open_audit(directory.path(), &config).verify().unwrap(),
        before
    );

    configure_v2(&path, &plan);
    assert!(
        load_oidc(&path, false, Redactor::new(vec![])).is_err(),
        "normal V4 configuration startup must not upgrade an existing V3 audit"
    );
    let still_v3 = open_audit(directory.path(), &config);
    assert_eq!(still_v3.format_version().unwrap(), 3);
    assert_eq!(still_v3.capacity_plan().unwrap(), None);
    assert_eq!(still_v3.verify().unwrap(), before);
    drop(still_v3);

    assert!(
        migrate_authentication(&path, &occupied, 4, Redactor::new(vec![])).is_err(),
        "an existing backup destination must never be overwritten"
    );
    assert_eq!(
        open_audit(directory.path(), &config).verify().unwrap(),
        before
    );
    let existing = AuthenticationAudit::open_existing(
        &occupied,
        config.authentication_audit.installation_id,
        config.authentication_audit.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(existing.format_version().unwrap(), 3);
    assert_eq!(existing.verify().unwrap(), before);
    drop(existing);

    let migrated = migrate_authentication(&path, &backup, 4, Redactor::new(vec![]))
        .expect("explicit V3 to V4 migration must succeed with a new backup");
    assert_eq!(migrated.namespace_id, before.namespace_id);
    assert_eq!(migrated.records, before.records + 1);
    let current = open_audit(directory.path(), &config);
    assert_eq!(current.format_version().unwrap(), 4);
    assert_eq!(current.capacity_plan().unwrap(), Some(plan));
    assert_eq!(current.verify().unwrap(), migrated);
    assert_eq!(
        current.append_typed(terminal.clone()).unwrap(),
        historical_receipt
    );
    assert_eq!(
        current.verify().unwrap(),
        migrated,
        "historical replay is a no-op"
    );
    drop(current);

    let saved = AuthenticationAudit::open_existing(
        &backup,
        config.authentication_audit.installation_id,
        config.authentication_audit.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(saved.format_version().unwrap(), 3);
    assert_eq!(saved.capacity_plan().unwrap(), None);
    assert_eq!(saved.verify().unwrap(), before);
    assert_eq!(saved.append_typed(terminal).unwrap(), historical_receipt);
    assert_eq!(saved.verify().unwrap(), before);
    drop(saved);
    drop(load_oidc(&path, false, Redactor::new(vec![])).unwrap());
    assert_eq!(
        document(&path)["authentication_audit"]["policy"],
        policy_before,
        "the original global policy is preserved through migration"
    );
}

#[tokio::test]
async fn incompatible_versions_and_invalid_plans_are_refused_before_database_creation() {
    for case in [
        "v1-with-plan",
        "v2-without-plan",
        "invalid-plan-version",
        "duplicate-tenant",
    ] {
        let (directory, path, config) = oidc_fixture();
        let plan = capacity_plan(&config.authentication_audit.policy);
        let mut value = document(&path);
        match case {
            "v1-with-plan" => {
                value["authentication_audit"]["capacity_plan"] =
                    serde_json::to_value(&plan).unwrap();
            }
            "v2-without-plan" => value["version"] = json!(2),
            "invalid-plan-version" | "duplicate-tenant" => {
                let mut invalid = plan;
                if case == "invalid-plan-version" {
                    invalid.version = 2;
                } else {
                    invalid.tenants[1].tenant_id = invalid.tenants[0].tenant_id.clone();
                }
                assert!(
                    invalid.validate().is_err(),
                    "negative plan must be structurally invalid"
                );
                value["version"] = json!(2);
                value["authentication_audit"]["capacity_plan"] =
                    serde_json::to_value(invalid).unwrap();
            }
            _ => unreachable!("closed test cases"),
        }
        write_document(&path, &value);
        let before = directory_entries(directory.path());
        assert!(
            load_oidc(&path, true, Redactor::new(vec![])).is_err(),
            "invalid configuration accepted: {case}"
        );
        assert!(
            !directory
                .path()
                .join(&config.authentication_audit.path)
                .exists(),
            "invalid configuration created the ledger: {case}"
        );
        assert_eq!(
            directory_entries(directory.path()),
            before,
            "invalid configuration created storage artifacts: {case}"
        );
    }
}
