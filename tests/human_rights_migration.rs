//! Explicit human-rights capability marker, its backup and the capabilities it
//! admits, with real private SQLite stores and the operator CLI.
mod support;
use agentsandbox::{audit::Store, identity::*, redaction::Redactor};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::{Command, Stdio},
};
use uuid::Uuid;

const NEW_CAPABILITIES: [HumanCapabilityV1; 4] = [
    HumanCapabilityV1::ReadMemberships,
    HumanCapabilityV1::ReadConfiguration,
    HumanCapabilityV1::LaunchSession,
    HumanCapabilityV1::SendSessionMessage,
];

fn migrated(path: &Path, through_workers: bool) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let steps: Vec<fn(&Store, &Path) -> anyhow::Result<agentsandbox::control::MigrationReportV1>> = vec![
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ];
    let count = if through_workers { steps.len() } else { 7 };
    for (index, step) in steps.into_iter().take(count).enumerate() {
        step(&store, &path.with_extension(format!("before{}", index + 3))).unwrap();
    }
    store
}
fn control(change: IdentityChangeV1) -> IdentityMutationV1 {
    by(
        PrincipalRefV1::LegacyControl {
            credential_id: "human-rights-migration-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    )
}
fn by(actor: PrincipalRefV1, change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor,
        change,
    }
}
fn preparation(capabilities: &[HumanCapabilityV1]) -> (LocalHumanProvisionV1, MembershipV1) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let token = generate_local_credential().unwrap();
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: capabilities.iter().copied().collect(),
    };
    let provision = LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            control(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Human rights reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            control(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal,
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            control(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: membership.clone(),
            }),
        ],
    };
    (provision, membership)
}
fn refusal(result: anyhow::Result<impl std::fmt::Debug>) -> Option<IdentityFailureV1> {
    result
        .unwrap_err()
        .downcast_ref::<IdentityFailureV1>()
        .copied()
}
fn migrate_cli(path: &Path, backup: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["migrate-human-rights", "--data"])
        .arg(path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(backup)
        .args(["--confirm-control-stopped", "--max-events", "1000"])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn new_capabilities_require_the_backed_up_marker_and_survive_reopen() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    let before = store.integrity().unwrap();
    assert!(!store.human_rights_installed().unwrap());
    for capability in NEW_CAPABILITIES {
        let (prepared, _) = preparation(&[capability]);
        assert_eq!(
            refusal(store.provision_local_human(&prepared)),
            Some(IdentityFailureV1::MigrationRequired),
            "{capability:?} must wait for the explicit marker"
        );
    }
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    drop(store);

    let backup = files.path().join("before-rights.backup");
    let output = migrate_cli(&path, &backup);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["from_version"], 13);
    assert_eq!(
        report["to_version"], 13,
        "the marker never changes the format"
    );
    assert!(report["genesis_seq"].as_i64().unwrap() > before.event_count as i64);

    let upgraded =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(upgraded.supervision_state().unwrap().schema_version, 13);
    assert!(upgraded.human_rights_installed().unwrap());
    assert_eq!(
        upgraded.integrity().unwrap().event_count,
        before.event_count + 1
    );
    let mut granted = Vec::new();
    for capability in NEW_CAPABILITIES {
        let (prepared, membership) = preparation(&[capability]);
        upgraded.provision_local_human(&prepared).unwrap();
        granted.push(membership);
    }
    for membership in &granted {
        assert_eq!(
            upgraded
                .identity_memberships(&membership.principal)
                .unwrap(),
            vec![membership.clone()]
        );
    }
    let head = upgraded.head().unwrap();
    drop(upgraded);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), head);
    assert!(
        reopened
            .migrate_human_rights_v2(&files.path().join("second.backup"))
            .is_err(),
        "the marker is installed once"
    );
    assert_eq!(reopened.head().unwrap(), head);
    drop(reopened);

    let saved =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
    assert!(!saved.human_rights_installed().unwrap());
    let (prepared, _) = preparation(&[HumanCapabilityV1::ReadMemberships]);
    assert_eq!(
        refusal(saved.provision_local_human(&prepared)),
        Some(IdentityFailureV1::MigrationRequired)
    );
}

#[test]
fn marker_requires_the_worker_registry_marker_and_leaves_the_store_unchanged() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, false);
    let before = store.integrity().unwrap();
    assert!(
        store
            .migrate_human_rights_v2(&files.path().join("schema9.backup"))
            .is_err()
    );
    assert_eq!(store.supervision_state().unwrap().schema_version, 9);
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    assert!(!store.human_rights_installed().unwrap());
}

#[test]
fn a_schema_downcast_cannot_hide_the_human_rights_marker() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 9).unwrap();
    drop(db);
    assert!(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).is_err(),
        "the human rights marker is invalid on a store without worker access"
    );
}

#[test]
fn organization_administrators_cannot_introduce_control_only_capabilities() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (admin, admin_membership) = preparation(&[
        HumanCapabilityV1::ManageMemberships,
        HumanCapabilityV1::ReadMetadata,
    ]);
    store.provision_local_human(&admin).unwrap();
    let (member, member_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&member).unwrap();
    let grant = |capabilities: &[HumanCapabilityV1], revision: u64| {
        by(
            admin_membership.principal.clone(),
            IdentityChangeV1::Membership {
                expected_revision: revision,
                membership: MembershipV1 {
                    revision: revision + 1,
                    capabilities: capabilities.iter().copied().collect(),
                    ..member_membership.clone()
                },
            },
        )
    };
    for capability in NEW_CAPABILITIES {
        let head = store.head().unwrap();
        assert_eq!(
            refusal(
                store.mutate_identity(&grant(&[HumanCapabilityV1::ReadMetadata, capability], 1))
            ),
            Some(IdentityFailureV1::PermissionDenied),
            "an organization administrator must not grant {capability:?}"
        );
        assert_eq!(store.head().unwrap(), head);
    }
    // The existing organization delegation is unchanged by the fence.
    store
        .mutate_identity(&grant(
            &[
                HumanCapabilityV1::ReadMetadata,
                HumanCapabilityV1::ReadContent,
            ],
            1,
        ))
        .unwrap();
    // Trusted control may still introduce a control-only capability.
    store
        .mutate_identity(&control(IdentityChangeV1::Membership {
            expected_revision: 2,
            membership: MembershipV1 {
                revision: 3,
                capabilities: [
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadContent,
                    HumanCapabilityV1::LaunchSession,
                ]
                .into_iter()
                .collect(),
                ..member_membership.clone()
            },
        }))
        .unwrap();
}
