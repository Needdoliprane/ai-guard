use super::*;
use crate::audit::kubernetes_admission_state_tests::Fixture;
use crate::broker_registry::parse_time;
use crate::control::QuotaPolicyV1;
use uuid::Uuid;

#[test]
fn claim_publication_never_recreates_freshness_outside_its_original_window() {
    let f = Fixture::new();
    let authorization = f
        .store
        .authorize_held_create_for_test(
            &f.identity,
            &f.intent,
            &QuotaPolicyV1::default(),
            "2100-01-01T00:00:00.000Z",
        )
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let claim = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    let (response, _fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &claim)
        .unwrap();
    let window = response.receipt.emission_window.as_ref().unwrap();
    let start = parse_time(&window.issued_at).unwrap();
    let end = parse_time(&window.not_after).unwrap();
    // Synthetic publication times isolate ordering/bounds, never host latency.
    assert!(publication_is_eligible(false, Some(window), start));
    assert!(!publication_is_eligible(true, Some(window), start));
    assert!(!publication_is_eligible(false, Some(window), end));
    assert!(!publication_is_eligible(
        false,
        Some(window),
        start - chrono::Duration::milliseconds(1)
    ));
    assert!(!publication_is_eligible(
        false,
        Some(window),
        end + chrono::Duration::milliseconds(1)
    ));
    assert!(!publication_is_eligible(false, None, start));
}
