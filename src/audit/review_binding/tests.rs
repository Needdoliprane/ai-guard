use super::*;
use crate::control::review_binding::tests::provision;
use std::os::unix::fs::PermissionsExt;

fn fixture_record(command: ProvisionRunnerReviewBindingV1) -> BindingRecordV1 {
    BindingRecordV1 {
        version: 1,
        authority_source_id: "11000000-0000-4000-8000-000000000020".into(),
        owner_incarnation: "11000000-0000-4000-8000-000000000021".into(),
        input_hash: command.input_hash().unwrap(),
        previous_revision: command.expected_revision,
        snapshot: transition(
            None,
            &BindingChangeV1::Provision {
                command: command.clone(),
            },
        )
        .unwrap(),
        change: BindingChangeV1::Provision { command },
    }
}

fn fixture_revision_record(command: ProvisionRunnerReviewBindingV1) -> BindingRecordV1 {
    BindingRecordV1 {
        version: 1,
        authority_source_id: "11000000-0000-4000-8000-000000000020".into(),
        owner_incarnation: "11000000-0000-4000-8000-000000000021".into(),
        input_hash: command.input_hash().unwrap(),
        previous_revision: command.expected_revision,
        snapshot: ReviewBindingSnapshotV1 {
            version: 1,
            binding: command.binding.clone(),
            policy: command.policy.clone(),
            binding_digest: command.binding.digest().unwrap(),
            policy_digest: command.policy.digest().unwrap(),
        },
        change: BindingChangeV1::Provision { command },
    }
}

// Without the immutable policy identity check, P1 -> P2 replaces the only
// snapshot of P1. After revocation, another binding could then reuse P1 at1.
// This regression must refuse the remapping itself and preserve P1's floor.
#[test]
fn policy_identity_cannot_remap_or_be_reused_after_revision_and_revocation() {
    let initial = provision();
    let first = fixture_record(initial.clone());
    let mut projection = Projection::default();
    projection.apply(&first, 1, "tenant-A").unwrap();
    let before = projection.snapshots.clone();
    let before_operations = projection.operations.clone();

    let mut revision = initial.clone();
    revision.operation_id = "11000000-0000-4000-8000-000000000060".into();
    revision.expected_revision = 1;
    revision.expected_policy_revision = 1;
    revision.binding.revision = 2;
    revision.policy.revision = 2;
    revision.binding.session_binding_policy_revision = 2;
    let mut remapped = revision.clone();
    remapped.policy.policy_id = "11000000-0000-4000-8000-000000000061".into();
    remapped.binding.session_binding_policy_id = remapped.policy.policy_id.clone();
    remapped.binding.session_binding_policy_digest = remapped.policy.digest().unwrap();
    let remapping = fixture_revision_record(remapped);
    let error = projection.apply(&remapping, 2, "tenant-A").unwrap_err();
    assert_eq!(
        error.downcast_ref::<ReviewBindingFailureV1>(),
        Some(&ReviewBindingFailureV1::Conflict)
    );
    assert_eq!(projection.snapshots, before);
    assert_eq!(projection.operations, before_operations);
    assert_eq!(projection.last_seq, 1);

    // Contents may change with the same policy ID and exact revision/CAS.
    revision.policy.shares[0].permissions = vec![ReviewSessionPermissionV1::Content];
    revision.binding.session_binding_policy_digest = revision.policy.digest().unwrap();
    let revised = fixture_revision_record(revision);
    projection.apply(&revised, 2, "tenant-A").unwrap();
    assert_eq!(
        projection.snapshots[&initial.binding.binding_id]
            .0
            .policy
            .policy_id,
        initial.policy.policy_id
    );
    assert_eq!(
        projection.snapshots[&initial.binding.binding_id]
            .0
            .policy
            .revision,
        2
    );
    assert_ne!(revised.snapshot.policy_digest, first.snapshot.policy_digest);
    let change = BindingChangeV1::Revoke {
        command: RevokeRunnerReviewBindingV1 {
            version: 1,
            operation_id: "11000000-0000-4000-8000-000000000062".into(),
            operator_delegation_id: initial.operator_delegation_id.clone(),
            binding_id: initial.binding.binding_id.clone(),
            expected_revision: 2,
            expected_binding_digest: revised.snapshot.binding_digest.clone(),
            reason: ReviewBindingRevocationReasonV1::OperatorRevoked,
        },
    };
    let revoked = BindingRecordV1 {
        input_hash: change.input_hash().unwrap(),
        previous_revision: 2,
        snapshot: transition(Some(&revised.snapshot), &change).unwrap(),
        change,
        ..first
    };
    projection.apply(&revoked, 3, "tenant-A").unwrap();
    let revoked_prefix = projection.snapshots.clone();
    let revoked_operations = projection.operations.clone();
    assert!(!revoked.snapshot.binding.active);

    let mut reuse = initial;
    reuse.operation_id = "11000000-0000-4000-8000-000000000063".into();
    reuse.binding.binding_id = "11000000-0000-4000-8000-000000000064".into();
    reuse.binding.credential_generation = 2; // Generation floor is satisfied.
    let error = projection
        .apply(&fixture_record(reuse), 4, "tenant-A")
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ReviewBindingFailureV1>(),
        Some(&ReviewBindingFailureV1::Conflict)
    );
    assert_eq!(projection.snapshots, revoked_prefix);
    assert_eq!(projection.operations, revoked_operations);
    assert_eq!(projection.last_seq, 3);
}

#[test]
fn revoked_snapshots_keep_credential_floors_namespace_and_policy_collisions() {
    let initial = provision();
    let first = fixture_record(initial.clone());
    let mut projection = Projection::default();
    projection.apply(&first, 1, "tenant-A").unwrap();
    let change = BindingChangeV1::Revoke {
        command: RevokeRunnerReviewBindingV1 {
            version: 1,
            operation_id: "11000000-0000-4000-8000-000000000065".into(),
            operator_delegation_id: initial.operator_delegation_id.clone(),
            binding_id: initial.binding.binding_id.clone(),
            expected_revision: 1,
            expected_binding_digest: first.snapshot.binding_digest.clone(),
            reason: ReviewBindingRevocationReasonV1::OperatorRevoked,
        },
    };
    let revoked = BindingRecordV1 {
        input_hash: change.input_hash().unwrap(),
        previous_revision: 1,
        snapshot: transition(Some(&first.snapshot), &change).unwrap(),
        change,
        ..first
    };
    projection.apply(&revoked, 2, "tenant-A").unwrap();
    let before = projection.snapshots.clone();
    let before_operations = projection.operations.clone();
    for refusal in ["generation", "namespace", "policy"] {
        let mut candidate = initial.clone();
        candidate.operation_id = "11000000-0000-4000-8000-000000000066".into();
        candidate.binding.binding_id = "11000000-0000-4000-8000-000000000067".into();
        candidate.binding.credential_generation = 2;
        if refusal == "policy" {
            // A different technical credential cannot adopt the old policy ID.
            candidate.binding.credential_id = "11000000-0000-4000-8000-000000000068".into();
        } else {
            candidate.policy.policy_id = "11000000-0000-4000-8000-000000000069".into();
            candidate.binding.session_binding_policy_id = candidate.policy.policy_id.clone();
            if refusal == "generation" {
                candidate.binding.credential_generation = 1;
            }
            if refusal == "namespace" {
                candidate.binding.control_namespace_uuid =
                    "11000000-0000-4000-8000-000000000070".into();
                candidate.policy.control_namespace_uuid =
                    candidate.binding.control_namespace_uuid.clone();
            }
            candidate.binding.session_binding_policy_digest = candidate.policy.digest().unwrap();
        }
        let error = projection
            .apply(&fixture_record(candidate), 3, "tenant-A")
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<ReviewBindingFailureV1>(),
            Some(&ReviewBindingFailureV1::Conflict),
            "{refusal}"
        );
        assert_eq!(projection.snapshots, before, "{refusal}");
        assert_eq!(projection.operations, before_operations, "{refusal}");
        assert_eq!(projection.last_seq, 2, "{refusal}");
    }
}

// These tests exercise the actual deterministic event replay and SQLite
// projection. They do not construct a runtime owner, marker or credential proof.
#[test]
fn projection_replay_cas_revocation_and_divergence_preserve_prefix() {
    let command = provision();
    let first = fixture_record(command.clone());
    let mut projection = Projection::default();
    projection.apply(&first, 1, "tenant-A").unwrap();
    let before = projection.snapshots.clone();
    let same = projection.apply(&first, 2, "tenant-A");
    assert!(same.is_err()); // Repeated command is a lookup, never a second event.
    assert_eq!(projection.snapshots, before);

    let mut conflict_command = command.clone();
    conflict_command.operation_id = "11000000-0000-4000-8000-000000000022".into();
    conflict_command.binding.binding_id = "11000000-0000-4000-8000-000000000023".into();
    let conflicting = fixture_record(conflict_command);
    assert!(projection.apply(&conflicting, 2, "tenant-A").is_err());
    assert_eq!(projection.snapshots, before);

    let revoke = RevokeRunnerReviewBindingV1 {
        version: 1,
        operation_id: "11000000-0000-4000-8000-000000000024".into(),
        operator_delegation_id: command.operator_delegation_id.clone(),
        binding_id: command.binding.binding_id.clone(),
        expected_revision: 1,
        expected_binding_digest: first.snapshot.binding_digest.clone(),
        reason: ReviewBindingRevocationReasonV1::OperatorRevoked,
    };
    let change = BindingChangeV1::Revoke { command: revoke };
    let revoked = transition(Some(&first.snapshot), &change).unwrap();
    assert!(!revoked.binding.active);
    assert_eq!(revoked.binding.revision, 2);
    let record = BindingRecordV1 {
        input_hash: change.input_hash().unwrap(),
        previous_revision: 1,
        change,
        snapshot: revoked.clone(),
        ..first.clone()
    };
    projection.apply(&record, 2, "tenant-A").unwrap();
    assert_eq!(projection.snapshots[&command.binding.binding_id].0, revoked);
    let revoked_prefix = projection.snapshots.clone();
    let mut wrong = record.clone();
    wrong.change = BindingChangeV1::Revoke {
        command: RevokeRunnerReviewBindingV1 {
            operation_id: "11000000-0000-4000-8000-000000000025".into(),
            ..match record.change.clone() {
                BindingChangeV1::Revoke { command } => command,
                _ => unreachable!(),
            }
        },
    };
    wrong.input_hash = wrong.change.input_hash().unwrap();
    assert!(projection.apply(&wrong, 3, "tenant-A").is_err());
    assert_eq!(projection.snapshots, revoked_prefix);

    // A fresh binding UUID must not reset the same technical credential to a
    // generation that was already revoked. Use a separate policy identity so
    // this oracle specifically discriminates the credential generation floor.
    let mut reset = command.clone();
    reset.operation_id = "11000000-0000-4000-8000-000000000026".into();
    reset.binding.binding_id = "11000000-0000-4000-8000-000000000027".into();
    reset.policy.policy_id = "11000000-0000-4000-8000-000000000028".into();
    reset.binding.session_binding_policy_id = reset.policy.policy_id.clone();
    reset.binding.session_binding_policy_digest = reset.policy.digest().unwrap();
    assert!(
        projection
            .apply(&fixture_record(reset.clone()), 3, "tenant-A")
            .is_err()
    );
    assert_eq!(projection.snapshots, revoked_prefix);
    reset.binding.credential_generation = 2;
    projection
        .apply(&fixture_record(reset), 3, "tenant-A")
        .unwrap();
    assert_eq!(projection.snapshots.len(), 2);
}

#[test]
fn sqlite_projection_is_exact_and_replay_rejects_tampered_snapshot() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE events(seq INTEGER PRIMARY KEY);")
        .unwrap();
    for (_, _, sql) in SCHEMA {
        db.execute_batch(sql).unwrap();
    }
    db.execute("INSERT INTO events(seq) VALUES(1)", []).unwrap();
    let record = fixture_record(provision());
    let mut projection = Projection::default();
    projection.apply(&record, 1, "tenant-A").unwrap();
    write_snapshot(&db, &record.snapshot, 1, None).unwrap();
    verify_rows(&db, &projection).unwrap();
    let stored = load_snapshot(&db, &record.snapshot.binding.binding_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored, record.snapshot);
    db.execute("UPDATE runner_review_bindings SET binding_revision=7", [])
        .unwrap();
    assert!(verify_rows(&db, &projection).is_err());
    let mut tampered = record.clone();
    tampered.snapshot.binding.broker_profile_id = "another-profile".into();
    let mut clean = Projection::default();
    assert!(clean.apply(&tampered, 1, "tenant-A").is_err());
    assert!(clean.snapshots.is_empty());
}

#[test]
fn first_grant_has_only_policy_owner_shares_and_exact_session() {
    let policy = provision().policy;
    let grant = derive_session_grant(&policy, "11000000-0000-4000-8000-000000000030").unwrap();
    assert_eq!(grant.tenant_id, "tenant-A");
    assert_eq!(grant.organization_scope, "organization-A");
    assert_eq!(grant.owner, policy.owner);
    assert_eq!(grant.shares.len(), 1);
    assert_eq!(grant.shares[0].principal, policy.shares[0].principal);
    assert_eq!(
        grant.shares[0].permissions,
        std::collections::BTreeSet::from([crate::identity::ResourcePermissionV1::Content])
    );
    assert_eq!(
        serde_json::to_value(&grant.shares[0].permissions).unwrap(),
        serde_json::json!(["content"])
    );
    // Approval remains descriptive policy ACL, never an extra legacy grant right.
    let mut content_only = policy.clone();
    content_only.owner_permissions = vec![ReviewSessionPermissionV1::Content];
    content_only.shares[0].permissions = vec![ReviewSessionPermissionV1::Content];
    assert_eq!(
        derive_session_grant(&content_only, "11000000-0000-4000-8000-000000000030").unwrap(),
        grant
    );
    assert_eq!(
        grant.resource.kind,
        crate::identity::ResourceKindV1::Session
    );
    assert_eq!(
        grant.resource.resource_id,
        "11000000-0000-4000-8000-000000000030"
    );
    assert!(derive_session_grant(&policy, "../another-session").is_err());
}

fn policy_accounts_fixture(policy: &ReviewSessionBindingPolicyV1) -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE identity_accounts(principal_key TEXT PRIMARY KEY,payload_json TEXT NOT NULL);CREATE TABLE identity_memberships(membership_id TEXT PRIMARY KEY,principal_key TEXT NOT NULL,organization_scope TEXT NOT NULL,payload_json TEXT NOT NULL);").unwrap();
    for (index, principal) in std::iter::once(&policy.owner)
        .chain(policy.shares.iter().map(|share| &share.principal))
        .enumerate()
    {
        let account = HumanAccountV1 {
            version: 1,
            principal: principal.clone(),
            display_name: format!("Fixture person {index}"),
            revision: 1,
            active: true,
        };
        let membership = MembershipV1 {
            version: 1,
            membership_id: format!("11000000-0000-4000-8000-{:012}", index + 40),
            principal: principal.clone(),
            tenant_id: policy.tenant_id.clone(),
            organization_scope: policy.organization_scope.clone(),
            revision: 1,
            active: true,
            capabilities: [
                HumanCapabilityV1::ReadContent,
                HumanCapabilityV1::ApproveLaunch,
            ]
            .into_iter()
            .collect(),
        };
        db.execute(
            "INSERT INTO identity_accounts VALUES(?1,?2)",
            params![
                principal.key().unwrap(),
                serde_json::to_string(&account).unwrap()
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO identity_memberships VALUES(?1,?2,?3,?4)",
            params![
                membership.membership_id,
                principal.key().unwrap(),
                policy.organization_scope,
                serde_json::to_string(&membership).unwrap()
            ],
        )
        .unwrap();
    }
    db
}

fn set_fixture_capabilities(
    db: &Connection,
    principal: &crate::identity::PrincipalRefV1,
    capabilities: &[HumanCapabilityV1],
) {
    let key = principal.key().unwrap();
    let payload: String = db
        .query_row(
            "SELECT payload_json FROM identity_memberships WHERE principal_key=?1",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    let mut membership: MembershipV1 = serde_json::from_str(&payload).unwrap();
    membership.capabilities = capabilities.iter().copied().collect();
    assert_eq!(
        db.execute(
            "UPDATE identity_memberships SET payload_json=?1 WHERE membership_id=?2",
            params![
                serde_json::to_string(&membership).unwrap(),
                membership.membership_id
            ]
        )
        .unwrap(),
        1
    );
}

// Reintroducing owner=>approve=true rejects this Content-only owner even though
// the unchanged share is still explicitly approval-capable in its own scope.
#[test]
fn content_only_owner_does_not_require_approval_capability() {
    let mut policy = provision().policy;
    policy.owner_permissions = vec![ReviewSessionPermissionV1::Content];
    let db = policy_accounts_fixture(&policy);
    set_fixture_capabilities(&db, &policy.owner, &[HumanCapabilityV1::ReadContent]);
    let before = db.total_changes();
    validate_policy_accounts(&db, &policy).unwrap();
    assert_eq!(db.total_changes(), before);
    assert!(
        !policy
            .owner_permissions
            .contains(&ReviewSessionPermissionV1::ApproveLaunch)
    );
}

#[test]
fn explicit_owner_approver_requires_both_current_content_and_approval_capabilities() {
    let policy = provision().policy;
    let db = policy_accounts_fixture(&policy);
    validate_policy_accounts(&db, &policy).unwrap();
    for capabilities in [
        vec![],
        vec![HumanCapabilityV1::ReadContent],
        vec![HumanCapabilityV1::ApproveLaunch],
    ] {
        set_fixture_capabilities(&db, &policy.owner, &capabilities);
        let before = db.total_changes();
        assert_eq!(
            validate_policy_accounts(&db, &policy)
                .unwrap_err()
                .downcast_ref::<ReviewBindingFailureV1>(),
            Some(&ReviewBindingFailureV1::PermissionDenied)
        );
        assert_eq!(db.total_changes(), before);
    }
    set_fixture_capabilities(
        &db,
        &policy.owner,
        &[
            HumanCapabilityV1::ReadContent,
            HumanCapabilityV1::ApproveLaunch,
        ],
    );
    let before = db.total_changes();
    validate_policy_accounts(&db, &policy).unwrap();
    assert_eq!(db.total_changes(), before);
}

#[test]
fn content_only_share_has_no_approval_requirement_but_explicit_share_does() {
    let mut policy = provision().policy;
    let db = policy_accounts_fixture(&policy);
    set_fixture_capabilities(
        &db,
        &policy.shares[0].principal,
        &[HumanCapabilityV1::ReadContent],
    );
    let before = db.total_changes();
    assert!(validate_policy_accounts(&db, &policy).is_err());
    assert_eq!(db.total_changes(), before);
    policy.shares[0].permissions = vec![ReviewSessionPermissionV1::Content];
    validate_policy_accounts(&db, &policy).unwrap();
    assert_eq!(db.total_changes(), before);
}

#[test]
fn owner_permission_change_requires_exact_policy_and_binding_cas() {
    let first = fixture_record(provision());
    let mut projection = Projection::default();
    projection.apply(&first, 1, "tenant-A").unwrap();
    let before = projection.snapshots.clone();
    let mut changed = provision();
    changed.operation_id = "11000000-0000-4000-8000-000000000071".into();
    changed.policy.owner_permissions = vec![ReviewSessionPermissionV1::Content];
    changed.binding.session_binding_policy_digest = changed.policy.digest().unwrap();
    let stale = fixture_revision_record(changed.clone());
    assert!(projection.apply(&stale, 2, "tenant-A").is_err());
    assert_eq!(projection.snapshots, before);
    assert_eq!(projection.last_seq, 1);
    changed.expected_revision = 1;
    changed.expected_policy_revision = 1;
    changed.binding.revision = 2;
    changed.policy.revision = 2;
    changed.binding.session_binding_policy_revision = 2;
    changed.binding.session_binding_policy_digest = changed.policy.digest().unwrap();
    let changed = fixture_revision_record(changed);
    projection.apply(&changed, 2, "tenant-A").unwrap();
    assert_eq!(
        changed.snapshot.policy.policy_id,
        first.snapshot.policy.policy_id
    );
    assert_ne!(changed.snapshot.policy_digest, first.snapshot.policy_digest);
    assert!(
        compare_retained_binding(
            &changed.snapshot,
            &first.snapshot.binding,
            &first.snapshot.binding_digest
        )
        .is_err()
    );
}

#[test]
fn authenticated_credential_tuple_must_match_generation_namespace_config_and_closure() {
    let binding = provision().binding;
    let exact = CredentialTuple {
        tenant_id: &binding.tenant_id,
        credential_id: &binding.credential_id,
        credential_generation: 1,
        closure_id: &binding.closure_id,
        closure_revision: 1,
        control_namespace_uuid: &binding.control_namespace_uuid,
        config_public_digest: &binding.control_config_public_digest,
    };
    compare_authenticated_tuple(&binding, &exact).unwrap();
    for changed in [
        CredentialTuple {
            tenant_id: "tenant-B",
            ..exact
        },
        CredentialTuple {
            credential_generation: 2,
            ..exact
        },
        CredentialTuple {
            control_namespace_uuid: "11000000-0000-4000-8000-000000000099",
            ..exact
        },
        CredentialTuple {
            closure_revision: 2,
            ..exact
        },
        CredentialTuple {
            config_public_digest: "0000000000000000000000000000000000000000000000000000000000000000",
            ..exact
        },
    ] {
        assert!(compare_authenticated_tuple(&binding, &changed).is_err());
    }
    let mut revoked = binding.clone();
    revoked.active = false;
    assert!(compare_authenticated_tuple(&revoked, &exact).is_err());
}

#[test]
fn retained_target_cannot_remap_broker_profile_source_project_routing_or_policy() {
    let command = provision();
    let snapshot = fixture_record(command.clone()).snapshot;
    compare_retained_binding(&snapshot, &command.binding, &snapshot.binding_digest).unwrap();
    for (field, replacement) in [
        (
            "broker_id",
            serde_json::json!("11000000-0000-4000-8000-000000000099"),
        ),
        ("broker_profile_id", serde_json::json!("other-profile")),
        ("source_id", serde_json::json!("other-workspace-source")),
        ("project_id", serde_json::json!("other-project")),
        ("routing_policy_revision", serde_json::json!(2)),
        ("session_binding_policy_revision", serde_json::json!(2)),
        ("source_policy_digest", serde_json::json!("0".repeat(64))),
    ] {
        let mut value = serde_json::to_value(&command.binding).unwrap();
        value[field] = replacement;
        let altered: RunnerReviewBindingV1 = serde_json::from_value(value).unwrap();
        assert!(
            compare_retained_binding(&snapshot, &altered, &altered.digest().unwrap()).is_err(),
            "{field}"
        );
    }
    assert!(compare_retained_binding(&snapshot, &command.binding, &"0".repeat(64)).is_err());
}

// Real Store encoder/appender and SQLite replay, entirely rolled back. This
// proves no modern mutation, source ownership, migration, readiness or admission.
#[test]
fn store_event_replay_and_exact_retry_are_descriptive_inside_rolled_back_transaction() {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let parent = std::fs::canonicalize(temp.path()).unwrap();
    let store = Store::open_for_tenant(
        &parent.join("audit.sqlite"),
        "tenant-A",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let mut db = store.db.lock().unwrap();
    let before = audit_head(&db).unwrap();
    let before_bytes: i64 = db
        .query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row.get(0))
        .unwrap();
    {
        let tx = store.begin_checked(&mut db).unwrap();
        assert_eq!(
            require_modern(&tx)
                .unwrap_err()
                .downcast_ref::<ReviewBindingFailureV1>(),
            Some(&ReviewBindingFailureV1::MigrationRequired)
        );
        for (_, _, sql) in SCHEMA {
            tx.execute_batch(sql).unwrap();
        }
        let first = fixture_record(provision());
        let mut event = Event::new(first.change.kind());
        event.id = first.change.operation_id().into();
        event.coverage = "native".into();
        event.data = serde_json::to_value(&first).unwrap();
        let original = serde_json::to_value(&event).unwrap();
        let (event, payload) = store.prepare_event(event).unwrap();
        assert_eq!(serde_json::to_value(&event).unwrap(), original);
        let stored = store
            .append_identity_in_transaction(&tx, event, payload)
            .unwrap();
        write_snapshot(&tx, &first.snapshot, stored.seq, None).unwrap();
        let decoded = record(&stored, "tenant-A").unwrap();
        assert_eq!(decoded, first);
        let projection = replay(&tx, "tenant-A").unwrap();
        assert_eq!(projection.snapshots.len(), 1);
        let durable_before = audit_head(&tx).unwrap();
        let retry = historical_result(
            &stored,
            &first.change,
            "tenant-A",
            &first.authority_source_id,
        )
        .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.receipt.audit_seq, stored.seq);
        assert_eq!(retry.receipt.audit_hash, stored.hash);
        assert_eq!(retry.snapshot, first.snapshot);
        let mut divergent = provision();
        divergent.operator_delegation_id = "11000000-0000-4000-8000-000000000099".into();
        let error = historical_result(
            &stored,
            &BindingChangeV1::Provision { command: divergent },
            "tenant-A",
            &first.authority_source_id,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<ReviewBindingFailureV1>(),
            Some(&ReviewBindingFailureV1::Conflict)
        );
        assert_eq!(audit_head(&tx).unwrap(), durable_before);
        verify_rows(&tx, &projection).unwrap();
        assert!(record(&stored, "tenant-B").is_err());
        assert_eq!(audit_head(&tx).unwrap(), durable_before);
        drop(tx); // Actual SQLite rollback restores the original Store prefix.
    }
    assert_eq!(audit_head(&db).unwrap(), before);
    assert_eq!(
        db.query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        before_bytes
    );
    let table_exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='runner_review_bindings')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!table_exists);
    assert!(Store::check_integrity(&db).unwrap().valid);
}

#[test]
fn policy_account_revalidation_refuses_revoked_or_missing_current_capability_without_mutating() {
    use crate::identity::{HumanAccountV1, MembershipV1};
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE identity_accounts(principal_key TEXT PRIMARY KEY,payload_json TEXT NOT NULL);CREATE TABLE identity_memberships(membership_id TEXT PRIMARY KEY,principal_key TEXT NOT NULL,organization_scope TEXT NOT NULL,payload_json TEXT NOT NULL);").unwrap();
    let policy = provision().policy;
    for (index, principal) in std::iter::once(&policy.owner)
        .chain(policy.shares.iter().map(|share| &share.principal))
        .enumerate()
    {
        let account = HumanAccountV1 {
            version: 1,
            principal: principal.clone(),
            display_name: format!("Fixture person {index}"),
            revision: 1,
            active: true,
        };
        let membership = MembershipV1 {
            version: 1,
            membership_id: format!("11000000-0000-4000-8000-{:012}", index + 40),
            principal: principal.clone(),
            tenant_id: policy.tenant_id.clone(),
            organization_scope: policy.organization_scope.clone(),
            revision: 1,
            active: true,
            capabilities: [
                HumanCapabilityV1::ReadContent,
                HumanCapabilityV1::ApproveLaunch,
            ]
            .into_iter()
            .collect(),
        };
        db.execute(
            "INSERT INTO identity_accounts VALUES(?1,?2)",
            params![
                principal.key().unwrap(),
                serde_json::to_string(&account).unwrap()
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO identity_memberships VALUES(?1,?2,?3,?4)",
            params![
                membership.membership_id,
                principal.key().unwrap(),
                policy.organization_scope,
                serde_json::to_string(&membership).unwrap()
            ],
        )
        .unwrap();
    }
    validate_policy_accounts(&db, &policy).unwrap();
    let original: Vec<String> = db
        .prepare("SELECT payload_json FROM identity_memberships ORDER BY membership_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for change in ["revoke", "capability", "tenant"] {
        let mut membership: MembershipV1 = serde_json::from_str(&original[0]).unwrap();
        match change {
            "revoke" => membership.active = false,
            "capability" => {
                membership
                    .capabilities
                    .remove(&HumanCapabilityV1::ApproveLaunch);
            }
            "tenant" => membership.tenant_id = "tenant-B".into(),
            _ => unreachable!(),
        }
        let payload = serde_json::to_string(&membership).unwrap();
        db.execute(
            "UPDATE identity_memberships SET payload_json=?1 WHERE membership_id=?2",
            params![payload, membership.membership_id],
        )
        .unwrap();
        let before = db.total_changes();
        assert!(validate_policy_accounts(&db, &policy).is_err(), "{change}");
        assert_eq!(db.total_changes(), before);
    }
    let owner_membership: MembershipV1 = serde_json::from_str(&original[0]).unwrap();
    db.execute(
        "UPDATE identity_memberships SET payload_json=?1 WHERE membership_id=?2",
        params![original[0], owner_membership.membership_id],
    )
    .unwrap();
    let key = policy.owner.key().unwrap();
    let payload: String = db
        .query_row(
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    let mut account: HumanAccountV1 = serde_json::from_str(&payload).unwrap();
    account.active = false;
    db.execute(
        "UPDATE identity_accounts SET payload_json=?1 WHERE principal_key=?2",
        params![serde_json::to_string(&account).unwrap(), key],
    )
    .unwrap();
    let before = db.total_changes();
    assert!(validate_policy_accounts(&db, &policy).is_err());
    assert_eq!(db.total_changes(), before);
    db.execute(
        "DELETE FROM identity_accounts WHERE principal_key=?1",
        [&key],
    )
    .unwrap();
    let before = db.total_changes();
    assert!(validate_policy_accounts(&db, &policy).is_err());
    assert_eq!(db.total_changes(), before);
}
