use super::*;

fn prepared() -> Prepared {
    Prepared {
        head: (0, "0".repeat(64)),
        namespace: None,
        tenant: AuditCapacityV1::default(),
        principals: HashMap::new(),
        workload_tenant: AuditCapacityV1::default(),
        workload_principals: HashMap::new(),
    }
}

#[test]
fn workload_defaults_fit_restrictive_existing_f1_policies_without_disabling_schema5() {
    let physical = PHYSICAL_BYTES;
    let control = AuditCapacityPolicyV1 {
        version: 1,
        human_tenant: AuditCapacityV1 {
            events: 2,
            bytes: 2,
        },
        human_principal: AuditCapacityV1 {
            events: 1,
            bytes: 1,
        },
        control_reserve: AuditCapacityV1 {
            events: 61,
            bytes: physical - 3,
        },
        identity_reserve: AuditCapacityV1 {
            events: 1,
            bytes: 1,
        },
    };
    control.validate(64).unwrap();
    let auto = WorkloadAuditPolicyV1::resolved(64, &control);
    assert_eq!(
        auto.tenant,
        AuditCapacityV1 {
            events: 2,
            bytes: 2
        }
    );
    assert_eq!(
        auto.principal,
        AuditCapacityV1 {
            events: 1,
            bytes: 1
        }
    );
    auto.validate(64, &control).unwrap();
    assert!(
        WorkloadAuditPolicyV1::resolved(64, &AuditCapacityPolicyV1::resolved(64))
            .validate(64, &control)
            .is_err()
    );
    let small = WorkloadAuditPolicyV1::resolved(7, &AuditCapacityPolicyV1::resolved(7));
    assert!(!small.enabled());
    assert_eq!(small.tenant, AuditCapacityV1::default());
    assert_eq!(small.principal, AuditCapacityV1::default());
    let mut value = serde_json::to_value(&auto).unwrap();
    value["tenant"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<WorkloadAuditPolicyV1>(value).is_err());
}

#[test]
fn workload_count_and_byte_limits_protect_both_reserves_and_other_principals() {
    let cache = Cache::default();
    let mut prefix = prepared();
    let control = AuditCapacityPolicyV1 {
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
    let policy = WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1 {
            events: 20,
            bytes: 200,
        },
        principal: AuditCapacityV1 {
            events: 2,
            bytes: 20,
        },
    };
    let physical = AuditCapacityV1 {
        events: 100,
        bytes: 1000,
    };
    let cost = AuditCapacityV1 {
        events: 1,
        bytes: 2,
    };
    let a = WorkloadAuditPrincipalV1::Broker {
        broker_id: uuid::Uuid::new_v4().to_string(),
    };
    let b = WorkloadAuditPrincipalV1::Broker {
        broker_id: uuid::Uuid::new_v4().to_string(),
    };
    for used in [
        AuditCapacityV1 {
            events: 2,
            bytes: 0,
        },
        AuditCapacityV1 {
            events: 0,
            bytes: 19,
        },
    ] {
        prefix.workload_principals.insert(a.key().unwrap(), used);
        let error = cache
            .admit_workload(
                &prefix,
                &control,
                &policy,
                physical,
                AuditCapacityV1::default(),
                cost,
                &a,
            )
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<AuditCapacityFailureV1>(),
            Some(&AuditCapacityFailureV1::WorkloadAuditBudgetExhausted)
        );
        cache
            .admit_workload(
                &prefix,
                &control,
                &policy,
                physical,
                AuditCapacityV1::default(),
                cost,
                &b,
            )
            .unwrap();
    }
    prefix.workload_principals.clear();
    for total in [
        AuditCapacityV1 {
            events: 63,
            bytes: 0,
        },
        AuditCapacityV1 {
            events: 0,
            bytes: 624,
        },
    ] {
        assert!(
            cache
                .admit_workload(&prefix, &control, &policy, physical, total, cost, &b)
                .is_err()
        );
        cache
            .admit(
                &prefix,
                &control,
                physical,
                total,
                cost,
                Class::OrdinaryWrite,
            )
            .unwrap();
        cache
            .admit(
                &prefix,
                &control,
                physical,
                total,
                cost,
                Class::IdentityMaintenance,
            )
            .unwrap();
    }
    let disabled = WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1::default(),
        principal: AuditCapacityV1::default(),
    };
    let error = cache
        .admit_workload(
            &prefix,
            &control,
            &disabled,
            physical,
            AuditCapacityV1::default(),
            cost,
            &b,
        )
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::WorkloadAuditDisabled)
    );
}
