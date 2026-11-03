//! Logical budget errors must preserve source authority and partial coverage.
use super::read_execution_tests::{human_get, local_identity_state};
use super::*;
use crate::identity::PrincipalRefV1;

fn exhaust_profile_budget(store: &Store, principal: &PrincipalRefV1) {
    let before = store.integrity().unwrap();
    let mut accepted = 0;
    let mut refused = false;
    for _ in 0..=store.capacity_policy().human_principal.events {
        match store.human_profile(principal, &Redactor::new(vec![])) {
            Ok(profile) => {
                assert!(profile.account.is_some());
                accepted += 1;
            }
            Err(error) => {
                assert_eq!(
                    error.downcast_ref::<AuditCapacityFailureV1>(),
                    Some(&AuditCapacityFailureV1::HumanReadBudgetExhausted)
                );
                refused = true;
                break;
            }
        }
    }
    assert!(refused);
    let after = store.integrity().unwrap();
    assert!(after.valid && store.healthy());
    assert_eq!(after.event_count, before.event_count + accepted);
}

#[tokio::test]
async fn exhausted_neighbor_profile_stays_partial_but_source_budget_refuses_identity() {
    let (_directory, state, token) = local_identity_state();
    let a = Arc::clone(
        &state
            .tenants
            .iter()
            .find(|t| t.tenant_id == "tenant-a")
            .unwrap()
            .store,
    );
    let b = Arc::clone(
        &state
            .tenants
            .iter()
            .find(|t| t.tenant_id == "tenant-b")
            .unwrap()
            .store,
    );
    let router = gateway_router(state);
    let (status, initial) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["contexts_complete"], true);
    assert_eq!(initial["contexts"].as_array().unwrap().len(), 2);
    let principal: PrincipalRefV1 = serde_json::from_value(initial["principal"].clone()).unwrap();

    exhaust_profile_budget(&a, &principal);
    let a_head = a.integrity().unwrap();
    let (status, partial) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(partial["contexts_complete"], false);
    let contexts = partial["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["tenant_id"], "tenant-b");
    assert_eq!(a.integrity().unwrap().head_hash, a_head.head_hash);

    exhaust_profile_budget(&b, &principal);
    let b_head = b.integrity().unwrap();
    let (status, refused) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(refused["error"]["retry_action"], "contact_operator");
    assert!(refused.get("contexts").is_none());
    assert_eq!(a.integrity().unwrap().head_hash, a_head.head_hash);
    assert_eq!(b.integrity().unwrap().head_hash, b_head.head_hash);
}
