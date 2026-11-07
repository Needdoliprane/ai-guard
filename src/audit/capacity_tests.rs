use super::*;
use crate::identity::*;
use std::os::unix::fs::PermissionsExt;

fn files() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn fresh(path: &Path, count: u64) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), count).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v4"))
        .unwrap();
    store
}
fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "capacity-fixture".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}
fn account(store: &Store, principal: &PrincipalRefV1) {
    store
        .mutate_identity(&mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Capacity reader".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
}
fn principal() -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn read(store: &Store, principal: &PrincipalRefV1) -> anyhow::Result<()> {
    store
        .human_profile(principal, &Redactor::new(vec![]))
        .map(|_| ())
}
fn assert_failure(result: anyhow::Result<()>, expected: AuditCapacityFailureV1) {
    assert_eq!(
        result.unwrap_err().downcast_ref::<AuditCapacityFailureV1>(),
        Some(&expected)
    );
}
fn limited(count: u64, tenant: u64, person: u64) -> AuditCapacityPolicyV1 {
    let mut policy = AuditCapacityPolicyV1::resolved(count);
    policy.human_tenant.events = tenant;
    policy.human_principal.events = person;
    policy
}

#[test]
fn principal_and_tenant_limits_are_exact_and_leave_another_reader() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 100)
        .with_capacity_policy(limited(100, 4, 2))
        .unwrap();
    let people = [principal(), principal(), principal()];
    for person in &people {
        account(&store, person);
    }
    for _ in 0..2 {
        read(&store, &people[0]).unwrap();
    }
    let head = store.head().unwrap();
    assert_failure(
        read(&store, &people[0]),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
    assert_eq!(store.head().unwrap(), head);
    for _ in 0..2 {
        read(&store, &people[1]).unwrap();
    }
    assert_failure(
        read(&store, &people[2]),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
    assert!(store.healthy());
}

#[test]
fn prepared_utf8_bytes_are_counted_exactly_and_survive_policy_change() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 100);
    let person = PrincipalRefV1::HumanOidc {
        issuer: "https://issuer.example.test".into(),
        subject: "élève".into(),
    };
    account(&store, &person);
    read(&store, &person).unwrap();
    let payload: String = store.db.lock().unwrap().query_row(
        "SELECT payload FROM events WHERE kind='control.identity.access' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert!(payload.len() > payload.chars().count());
    let bytes = u64::try_from(payload.len()).unwrap();
    let mut policy = AuditCapacityPolicyV1::resolved(100);
    policy.human_tenant.bytes = bytes * 3;
    policy.human_principal.bytes = bytes * 2;
    let store = store.with_capacity_policy(policy).unwrap();
    read(&store, &person).unwrap(); // Inclusive byte boundary, including the first read.
    assert_failure(
        read(&store, &person),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
    let cache = store.supervision_lease.capacity.lock().unwrap();
    assert_eq!(cache.tenant.bytes, bytes * 2);
    assert_eq!(cache.principals[&person.key().unwrap()].bytes, bytes * 2);
}

#[test]
fn reads_leave_control_and_identity_reserves_and_kind_cannot_spend_identity() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 64);
    let person = principal();
    account(&store, &person);
    for _ in 0..8 {
        read(&store, &person).unwrap();
    }
    assert_failure(
        read(&store, &person),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
    // A public event with a privileged-looking kind still has OrdinaryWrite.
    while store.head().unwrap() < 40 {
        store.append(Event::new("security.decision")).unwrap();
    }
    let mut imitation = Event::new("security.decision");
    imitation.source = "gateway".into();
    imitation.data = json!({"capacity_class":"identity_maintenance", "action":"revoke"});
    let rejected = store.append(imitation).map(|_| ());
    assert_failure(
        rejected,
        AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted,
    );
    assert!(
        store
            .append(Event::new("control.identity.mutation"))
            .is_err()
    );
    // Only the trusted append path may spend the remaining control share.
    let control = Event::new("command.completed");
    let first = store.append_control_event(control.clone()).unwrap();
    while store.head().unwrap() < 56 {
        store
            .append_control_event(Event::new("command.completed"))
            .unwrap();
    }
    assert_failure(
        store
            .append_control_event(Event::new("command.completed"))
            .map(|_| ()),
        AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted,
    );
    assert_eq!(store.append_control_event(control).unwrap().seq, first.seq);
    assert_eq!(store.head().unwrap(), 56);
    let revoke = mutation(IdentityChangeV1::Account {
        expected_revision: 1,
        account: HumanAccountV1 {
            version: 1,
            principal: person,
            display_name: "Capacity reader".into(),
            revision: 2,
            active: false,
        },
    });
    let receipt = store.mutate_identity(&revoke).unwrap();
    assert_eq!(receipt.audit_seq, 57);
    assert_eq!(
        store.mutate_identity(&revoke).unwrap().audit_seq,
        receipt.audit_seq
    );
    assert_eq!(
        store.head().unwrap(),
        57,
        "exact replay never costs another event"
    );
}

#[test]
fn head_and_byte_reserve_boundaries_are_checked_in_each_dimension() {
    let cache = capacity::Cache::default();
    let prepared = capacity::Prepared {
        head: (0, "0".repeat(64)),
        namespace: None,
        tenant: AuditCapacityV1::default(),
        principals: Default::default(),
        workload_tenant: AuditCapacityV1::default(),
        workload_principals: Default::default(),
    };
    let policy = AuditCapacityPolicyV1 {
        version: 1,
        human_tenant: AuditCapacityV1 {
            events: 50,
            bytes: 500,
        },
        human_principal: AuditCapacityV1 {
            events: 12,
            bytes: 125,
        },
        control_reserve: AuditCapacityV1 {
            events: 25,
            bytes: 250,
        },
        identity_reserve: AuditCapacityV1 {
            events: 12,
            bytes: 125,
        },
    };
    let physical = AuditCapacityV1 {
        events: 100,
        bytes: 1000,
    };
    let person = principal();
    for (total, cost) in [
        (
            AuditCapacityV1 {
                events: 62,
                bytes: 0,
            },
            AuditCapacityV1 {
                events: 1,
                bytes: 1,
            },
        ),
        (
            AuditCapacityV1 {
                events: 0,
                bytes: 624,
            },
            AuditCapacityV1 {
                events: 1,
                bytes: 1,
            },
        ),
    ] {
        cache
            .admit(
                &prepared,
                &policy,
                physical,
                total,
                cost,
                capacity::Class::HumanRead(&person),
            )
            .unwrap();
        let over = total.add(cost).unwrap();
        assert_failure(
            cache.admit(
                &prepared,
                &policy,
                physical,
                over,
                cost,
                capacity::Class::HumanRead(&person),
            ),
            AuditCapacityFailureV1::HumanReadBudgetExhausted,
        );
        cache
            .admit(
                &prepared,
                &policy,
                physical,
                total,
                cost,
                capacity::Class::OrdinaryWrite,
            )
            .unwrap();
        assert_failure(
            cache.admit(
                &prepared,
                &policy,
                physical,
                over,
                cost,
                capacity::Class::OrdinaryWrite,
            ),
            AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted,
        );
    }
    for total in [
        AuditCapacityV1 {
            events: 87,
            bytes: 0,
        },
        AuditCapacityV1 {
            events: 0,
            bytes: 874,
        },
    ] {
        let cost = AuditCapacityV1 {
            events: 1,
            bytes: 1,
        };
        cache
            .admit(
                &prepared,
                &policy,
                physical,
                total,
                cost,
                capacity::Class::ControlWrite,
            )
            .unwrap();
        assert_failure(
            cache.admit(
                &prepared,
                &policy,
                physical,
                total.add(cost).unwrap(),
                cost,
                capacity::Class::ControlWrite,
            ),
            AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted,
        );
    }
}

#[test]
fn historical_occupancy_above_ordinary_reserve_remains_readable_and_replayable() {
    let dir = files();
    let path = dir.path().join("historical.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3"))
        .unwrap();
    // Pre-v5 appends used physical capacity. Keep that valid historical chain;
    // the new admission rule must not reinterpret it as corruption or clear it.
    while store.head().unwrap() < 41 {
        store.append(Event::new("fixture.historical")).unwrap();
    }
    let historical = Event::new("fixture.historical");
    let original = store.append(historical.clone()).unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v4"))
        .unwrap();
    assert_eq!(store.head().unwrap(), 43);
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    assert!(reopened.integrity().unwrap().valid && reopened.healthy());
    assert!(!reopened.events(&EventQuery::default()).unwrap().is_empty());
    let replay = reopened.append(historical).unwrap();
    assert_eq!((replay.seq, replay.hash), (original.seq, original.hash));
    assert_eq!(reopened.head().unwrap(), 43);
    assert_failure(
        reopened.append(Event::new("fixture.new")).map(|_| ()),
        AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted,
    );
    assert_eq!(
        reopened
            .append_control_event(Event::new("command.completed"))
            .unwrap()
            .seq,
        44
    );
    assert!(reopened.integrity().unwrap().valid && reopened.healthy());
}

#[test]
fn small_defaults_disable_reads_without_latch_and_pre_v5_keeps_physical_capacity() {
    for count in 1..8 {
        assert!(!AuditCapacityPolicyV1::resolved(count).human_reads_enabled());
    }
    let dir = files();
    let store = fresh(&dir.path().join("small.sqlite"), 7);
    let person = principal();
    account(&store, &person);
    assert_failure(
        read(&store, &person),
        AuditCapacityFailureV1::HumanReadDisabled,
    );
    assert!(store.healthy());
    assert!(store.head().unwrap() < 7);
    let legacy = Store::open_for_tenant(
        &dir.path().join("legacy.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        16,
    )
    .unwrap();
    while legacy.head().unwrap() < 16 {
        legacy.append(Event::new("fixture.legacy")).unwrap();
    }
    let error = legacy.append(Event::new("fixture.full")).unwrap_err();
    assert!(error.downcast_ref::<AuditCapacityFailureV1>().is_none());
    assert!(legacy.healthy());
}

#[test]
fn physical_full_still_precedes_logical_refusal_and_exact_replay() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 16);
    let person = principal();
    account(&store, &person);
    let mut last = None;
    while store.head().unwrap() < 16 {
        let p = principal();
        let change = mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: p,
                display_name: "Fixture".into(),
                revision: 1,
                active: true,
            },
        });
        store.mutate_identity(&change).unwrap();
        last = Some(change);
    }
    assert!(
        read(&store, &person)
            .unwrap_err()
            .downcast_ref::<AuditCapacityFailureV1>()
            .is_none()
    );
    store.mutate_identity(last.as_ref().unwrap()).unwrap();
    assert_eq!(store.head().unwrap(), 16);
    assert!(store.healthy());
}

#[test]
fn rollback_never_publishes_uncommitted_read_usage() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 64);
    let person = principal();
    account(&store, &person);
    let head = store.head().unwrap();
    {
        let mut db = store.db.lock().unwrap();
        let tx = store.begin_checked(&mut db).unwrap();
        store
            .record_human_read(&tx, &person, None, "identity", 1, &Redactor::new(vec![]))
            .unwrap();
        assert_eq!(
            store
                .supervision_lease
                .capacity
                .lock()
                .unwrap()
                .tenant
                .events,
            0
        );
        // Deliberately abandon the real SQLite transaction.
    }
    assert_eq!(store.head().unwrap(), head);
    for _ in 0..8 {
        read(&store, &person).unwrap();
    }
    assert_failure(
        read(&store, &person),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
}

#[test]
fn reopen_siblings_external_commits_and_integrity_preserve_the_same_debit() {
    let dir = files();
    let path = dir.path().join("a.sqlite");
    let policy = limited(100, 8, 3);
    let store = fresh(&path, 100)
        .with_capacity_policy(policy.clone())
        .unwrap();
    let person = principal();
    account(&store, &person);
    let sibling = store
        .sibling_with_redactor(Redactor::new(vec!["fixture-redaction-union".into()]))
        .unwrap();
    assert_eq!(sibling.capacity_policy(), &policy);
    read(&sibling, &person).unwrap();
    let external = Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100)
        .unwrap()
        .with_capacity_policy(policy.clone())
        .unwrap();
    read(&external, &person).unwrap();
    assert!(store.integrity().unwrap().valid); // Can acknowledge the external data_version.
    read(&store, &person).unwrap();
    assert_failure(
        read(&sibling, &person),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
    drop((store, sibling, external));
    let reopened = Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100)
        .unwrap()
        .with_capacity_policy(policy)
        .unwrap();
    assert_failure(
        read(&reopened, &person),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
}

#[test]
fn stable_reads_only_scan_the_newly_committed_suffix() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 1000);
    let person = principal();
    account(&store, &person);
    for _ in 0..40 {
        store.append(Event::new("fixture.noise")).unwrap();
    }
    let before = store.supervision_lease.capacity.lock().unwrap().scanned;
    for _ in 0..3 {
        read(&store, &person).unwrap();
    }
    assert_eq!(
        store.supervision_lease.capacity.lock().unwrap().scanned - before,
        3
    );
}

#[test]
fn concurrent_siblings_share_one_principal_budget_without_overshoot() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 100)
        .with_capacity_policy(limited(100, 20, 5))
        .unwrap();
    let person = principal();
    account(&store, &person);
    let workers: Vec<_> = (0..4).map(|_| store.sibling().unwrap()).collect();
    let barrier = Arc::new(std::sync::Barrier::new(workers.len()));
    let successes: usize = std::thread::scope(|scope| {
        let joins: Vec<_> = workers
            .into_iter()
            .map(|worker| {
                let barrier = Arc::clone(&barrier);
                let person = &person;
                scope.spawn(move || {
                    barrier.wait();
                    let mut accepted = 0;
                    for _ in 0..5 {
                        match read(&worker, person) {
                            Ok(()) => accepted += 1,
                            Err(error) => assert_eq!(
                                error.downcast_ref::<AuditCapacityFailureV1>(),
                                Some(&AuditCapacityFailureV1::HumanReadBudgetExhausted)
                            ),
                        }
                    }
                    accepted
                })
            })
            .collect();
        joins.into_iter().map(|join| join.join().unwrap()).sum()
    });
    assert_eq!(successes, 5);
    assert_eq!(
        store
            .supervision_lease
            .capacity
            .lock()
            .unwrap()
            .tenant
            .events,
        5
    );
}

#[test]
fn policies_are_closed_checked_and_cannot_change_after_sharing() {
    let policy = AuditCapacityPolicyV1::resolved(64);
    policy.validate(64).unwrap();
    let mut value = serde_json::to_value(&policy).unwrap();
    value["human_principal"]["unknown"] = json!(true);
    assert!(serde_json::from_value::<AuditCapacityPolicyV1>(value).is_err());
    let mut bad = policy.clone();
    bad.control_reserve.events = u64::MAX;
    assert!(bad.validate(64).is_err());
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 64);
    let sibling = store.sibling().unwrap();
    assert!(store.with_capacity_policy(policy).is_err());
    assert!(sibling.healthy());
}

#[test]
fn rotating_a_credential_does_not_replenish_the_canonical_principal() {
    let dir = files();
    let store = fresh(&dir.path().join("a.sqlite"), 100)
        .with_capacity_policy(limited(100, 8, 2))
        .unwrap();
    let person = principal();
    account(&store, &person);
    let token = generate_local_credential().unwrap();
    let mut binding = LocalCredentialV1 {
        version: 1,
        credential_id: uuid::Uuid::new_v4().to_string(),
        principal: person.clone(),
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        revision: 1,
        active: true,
        expires_at: None,
    };
    store
        .mutate_identity(&mutation(IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: binding.clone(),
        }))
        .unwrap();
    for _ in 0..2 {
        read(&store, &person).unwrap();
    }
    let rotated = generate_local_credential().unwrap();
    let digest: [u8; 32] = Sha256::digest(rotated.as_bytes()).into();
    binding.sha256_digest = hex::encode(digest);
    binding.revision = 2;
    store
        .mutate_identity(&mutation(IdentityChangeV1::Credential {
            expected_revision: 1,
            binding,
        }))
        .unwrap();
    assert_eq!(
        store
            .authenticate_local_digest(&digest)
            .unwrap()
            .unwrap()
            .principal,
        person
    );
    assert_failure(
        read(&store, &person),
        AuditCapacityFailureV1::HumanReadBudgetExhausted,
    );
}

#[test]
fn late_provision_failure_rolls_back_every_event_projection_and_debit() {
    for initial_head in [14, 15] {
        let dir = files();
        let store = fresh(&dir.path().join("a.sqlite"), 16);
        while store.head().unwrap() < initial_head {
            account(&store, &principal());
        }
        let person = principal();
        let token = generate_local_credential().unwrap();
        let provision = LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                mutation(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: person.clone(),
                        display_name: "Atomic reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                mutation(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: uuid::Uuid::new_v4().to_string(),
                        principal: person.clone(),
                        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                mutation(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: uuid::Uuid::new_v4().to_string(),
                        principal: person.clone(),
                        tenant_id: "tenant-a".into(),
                        organization_scope: "fixture".into(),
                        revision: 1,
                        active: true,
                        capabilities: std::collections::BTreeSet::from([
                            HumanCapabilityV1::ReadMetadata,
                        ]),
                    },
                }),
            ],
        };
        assert!(store.provision_local_human(&provision).is_err());
        assert_eq!(store.head().unwrap(), initial_head);
        assert!(store.human_account(&person).unwrap().is_none());
        let cache = store.supervision_lease.capacity.lock().unwrap();
        assert_eq!(cache.tenant.events, 0);
        assert!(!cache.principals.contains_key(&person.key().unwrap()));
        assert!(store.healthy());
    }
}

#[test]
fn policy_files_are_private_bounded_and_closed() {
    let dir = files();
    let path = dir.path().join("capacity.json");
    let policy = AuditCapacityPolicyV1::resolved(64);
    std::fs::write(&path, serde_json::to_vec(&policy).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(Store::load_capacity_policy(&path).unwrap(), policy);
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(Store::load_capacity_policy(&link).is_err());
    std::fs::write(&path, vec![b' '; 16_385]).unwrap();
    assert!(Store::load_capacity_policy(&path).is_err());
}

#[test]
fn invalid_external_changes_are_not_admitted_or_published_as_read_usage() {
    let dir = files();
    let path = dir.path().join("a.sqlite");
    let store = fresh(&path, 64);
    let person = principal();
    account(&store, &person);
    read(&store, &person).unwrap();
    let before = store.supervision_lease.capacity.lock().unwrap().tenant;
    let external = Connection::open(&path).unwrap();
    external
        .execute(
            "UPDATE events SET kind='fixture.invalid' WHERE kind='control.identity.access'",
            [],
        )
        .unwrap();
    assert!(read(&store, &person).is_err());
    assert!(!store.healthy());
    assert_eq!(
        store.supervision_lease.capacity.lock().unwrap().tenant,
        before
    );
}
