//! Real gateway authentication, usage leases and SQLite through the S2 wrapper.
//! No auth/capability constructor or forced database state is used here.
use super::*;
use crate::api::{GatewayState, TenantGateway, with_authenticated_control_recovery_for_test};
use axum::http::{HeaderMap, HeaderValue, header};
use std::sync::Arc;

const RUNNER_A: &str = "public-session-confirmation-tenant-a-runner-20261110";
const VIEWER_A: &str = "public-session-confirmation-tenant-a-viewer-20261110";
const RUNNER_B: &str = "public-session-confirmation-tenant-b-runner-20261110";
const VIEWER_B: &str = "public-session-confirmation-tenant-b-viewer-20261110";

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {RUNNER_A}")).unwrap(),
    );
    headers
}

fn recovery_policy(
    intent: &KubernetesSessionReservationIntentV2,
) -> KubernetesSessionControlRecoveryPolicyV2 {
    KubernetesSessionControlRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec![intent.ids().unwrap().broker_id.to_string()],
    }
}

fn observe_target(
    intent: &KubernetesSessionReservationIntentV2,
) -> KubernetesSessionControlRecoveryTargetV2 {
    KubernetesSessionControlRecoveryTargetV2::ObserveReserved {
        reservation_id: intent.reservation_id().into(),
        reservation_input_hash: intent.input_hash("tenant-a").unwrap(),
    }
}

fn before_effect<'a>(
    authorization: KubernetesSessionControlRecoveryAuthorizationV2<'a>,
) -> ControlRecoverBeforeEffectV2<'a> {
    let KubernetesSessionControlRecoveryAuthorizationV2::BeforeEffect(cap) = authorization else {
        panic!("the real ObserveReserved issuer must return before-effect evidence")
    };
    cap
}

fn ledger(store: &Store) -> anyhow::Result<serde_json::Value> {
    let events = store.events(&EventQuery {
        limit: Some(1000),
        ..EventQuery::default()
    })?;
    // These fresh databases are much smaller than the query bound. Preserve the
    // complete event vector, not only a counter or the reservation's last event.
    assert!(events.len() < 1000);
    assert!(store.integrity()?.valid);
    Ok(json!({
        "head": store.head()?,
        "events": events,
        "quota": store.worker_quota(&QuotaPolicyV1::default())?,
    }))
}

#[tokio::test]
async fn s2_confirmation_rechecks_a_real_retired_epoch_without_changing_the_reservation() {
    // If this wrapper reused a previous observation instead of acquiring the
    // current auth usage lease, the post-retirement denial below would fail.
    let f = Fixture::new();
    prepare(&f);
    let intent = intent(&f, Uuid::new_v4());
    let first = reserve(&f, &intent, &QuotaPolicyV1::default());
    let policy = recovery_policy(&intent);
    let target = observe_target(&intent);
    let before = ledger(&f.store).unwrap();
    let _directory = f._directory;
    let identity = f.identity;
    let store = Arc::new(f.store);
    let state = GatewayState::new(
        Arc::clone(&store),
        RUNNER_A.into(),
        VIEWER_A.into(),
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy.clone())])
    .unwrap();
    let retirement = state.control_recovery_retirement();
    with_authenticated_control_recovery_for_test(&state, headers(), move |auth, selected| {
        assert_eq!(auth.tenant_id(), "tenant-a");
        let cap = before_effect(
            selected.authorize_kubernetes_session_control_recovery(auth, &target, &policy)?,
        );
        let confirmed =
            selected.confirm_kubernetes_session_control_before_effect(auth, &cap, &policy)?;
        assert_eq!(confirmed.tenant_id(), "tenant-a");
        assert_eq!(
            serde_json::to_value(confirmed.snapshot())?,
            serde_json::to_value(&first.snapshot)?
        );
        assert_eq!(confirmed.audit_seq(), first.receipt.audit_seq);
        assert_eq!(confirmed.audit_hash(), first.receipt.audit_hash);
        let committed = ledger(&selected)?;
        assert_eq!(committed["quota"]["held_workers"], 1);

        retirement.retire();
        // The completed confirmation retained facts, not a permanent usage lease.
        assert!(retirement.drained());
        let error = selected
            .confirm_kubernetes_session_control_before_effect(auth, &cap, &policy)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "control recovery authentication unavailable"
        );
        assert!(retirement.drained());
        let observe = selected.authorize_kubernetes_session_reconciliation(
            &identity,
            intent.reservation_id(),
            None,
        )?;
        let still_reserved = selected.confirm_kubernetes_session_before_effect(
            &identity,
            &observe,
            intent.reservation_id(),
        )?;
        assert_eq!(
            serde_json::to_value(still_reserved.snapshot())?,
            serde_json::to_value(confirmed.snapshot())?
        );
        assert_eq!(still_reserved.audit_seq(), confirmed.audit_seq());
        assert_eq!(still_reserved.audit_hash(), confirmed.audit_hash());
        assert_eq!(ledger(&selected)?, committed);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(ledger(&store).unwrap(), before);
}

#[tokio::test]
async fn s2_confirmation_cannot_cross_from_the_authenticated_tenant_to_another_store() {
    // The expected authentication error distinguishes a lease refusal from a
    // later NotFound/schema error in the empty, independently migrated B Store.
    let f = Fixture::new();
    prepare(&f);
    let intent = intent(&f, Uuid::new_v4());
    let first = reserve(&f, &intent, &QuotaPolicyV1::default());
    let policy = recovery_policy(&intent);
    let target = observe_target(&intent);
    let path_b = f._directory.path().join("tenant-b.sqlite");
    let store_b =
        Store::open_for_tenant(&path_b, "tenant-b", Redactor::new(vec![]), 10_000).unwrap();
    store_b
        .migrate_control_v3(&path_b.with_extension("before3"))
        .unwrap();
    store_b
        .migrate_identity_v4(&path_b.with_extension("before4"))
        .unwrap();
    store_b
        .migrate_human_access_v5(&path_b.with_extension("before5"))
        .unwrap();
    store_b
        .migrate_workload_v6(&path_b.with_extension("before6"))
        .unwrap();
    store_b
        .migrate_human_brokers_v7(&path_b.with_extension("before7"))
        .unwrap();
    store_b
        .migrate_kubernetes_create_v8(&path_b.with_extension("before8"))
        .unwrap();
    store_b
        .migrate_kubernetes_policy_v9(&path_b.with_extension("before9"))
        .unwrap();
    store_b
        .migrate_human_workers_v10(&path_b.with_extension("before10"))
        .unwrap();
    store_b
        .migrate_kubernetes_session_reservations_v11(&path_b.with_extension("before11"))
        .unwrap();
    let _directory = f._directory;
    let store_a = Arc::new(f.store);
    let store_b = Arc::new(store_b);
    let before_a = ledger(&store_a).unwrap();
    let before_b = ledger(&store_b).unwrap();
    assert_eq!(before_a["quota"]["held_workers"], 1);
    assert_eq!(before_b["quota"]["held_workers"], 0);
    let state = GatewayState::for_tenants(
        vec![
            TenantGateway {
                tenant_id: "tenant-a".into(),
                store: Arc::clone(&store_a),
                runner_token: RUNNER_A.into(),
                viewer_token: VIEWER_A.into(),
            },
            TenantGateway {
                tenant_id: "tenant-b".into(),
                store: Arc::clone(&store_b),
                runner_token: RUNNER_B.into(),
                viewer_token: VIEWER_B.into(),
            },
        ],
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy.clone())])
    .unwrap();
    let foreign = Arc::clone(&store_b);
    // Gateway construction adds redaction through a distinct Store wrapper.
    // Compare the authenticated tenant and full persisted state, not Arc identity.
    let expected_a = before_a.clone();
    with_authenticated_control_recovery_for_test(&state, headers(), move |auth, selected| {
        assert_eq!(auth.tenant_id(), "tenant-a");
        assert_eq!(selected.tenant_id(), "tenant-a");
        assert_eq!(ledger(&selected)?, expected_a);
        let cap = before_effect(
            selected.authorize_kubernetes_session_control_recovery(auth, &target, &policy)?,
        );
        let confirmed =
            selected.confirm_kubernetes_session_control_before_effect(auth, &cap, &policy)?;
        assert_eq!(
            serde_json::to_value(confirmed.snapshot())?,
            serde_json::to_value(&first.snapshot)?
        );
        let committed_a = ledger(&selected)?;
        let committed_b = ledger(&foreign)?;
        let error = foreign
            .confirm_kubernetes_session_control_before_effect(auth, &cap, &policy)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "control recovery authentication unavailable"
        );
        // The rejected foreign call neither consumed A's evidence nor altered
        // either tenant's reservation/audit/quota. Re-observe through real S2.
        let current =
            selected.confirm_kubernetes_session_control_before_effect(auth, &cap, &policy)?;
        assert_eq!(
            serde_json::to_value(current.snapshot())?,
            serde_json::to_value(confirmed.snapshot())?
        );
        assert_eq!(current.audit_seq(), confirmed.audit_seq());
        assert_eq!(current.audit_hash(), confirmed.audit_hash());
        assert_eq!(ledger(&selected)?, committed_a);
        assert_eq!(ledger(&foreign)?, committed_b);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(ledger(&store_a).unwrap(), before_a);
    assert_eq!(ledger(&store_b).unwrap(), before_b);
}
