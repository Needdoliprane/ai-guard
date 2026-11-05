//! Store-backed V2 authorization proofs; no API server or Kubernetes runtime.
use super::*;
use crate::audit::kubernetes_admission_state_tests::Fixture;
use crate::control::QuotaPolicyV1;
use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const V1_HASH: &str = "6ad99a96bbcad047edccbc460cdf777a55464d26bdf892f67c95e3fae956c08d";
const V2_ADMISSION_HASH: &str = "885c8e1a8339e82ce0f26e407580d53ae448a4a62c49c65b24621799c43dc049";
const V2_RECONCILE_HASH: &str = "dae60e3aa1284187607b1475a0d828379774976842724dc2ec487fba501de803";
const POLICY_GENESIS_KIND: &str = "control.kubernetes.create.policy.genesis";
const CREATE_GENESIS_KIND: &str = "control.kubernetes.create.genesis";
const EXPIRY: &str = "2100-01-01T00:00:00.000Z";

const V1_WIRE: &str = r#"{"version":1,"tenant_id":"tenant-a","control_namespace_uuid":"11111111-1111-4111-8111-111111111111","broker_id":"22222222-2222-4222-8222-222222222222","incarnation_id":"33333333-3333-4333-8333-333333333333","credential_id":"44444444-4444-4444-8444-444444444444","binding_revision":3,"grant_revision":4,"origin_seq":123,"origin_hash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","authority_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","intent_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","quota":{"version":1,"max_active_workers":3,"max_cpu_millis":6000,"max_memory_bytes":3221225472,"max_timeout_seconds":3600},"issued_at":"2026-11-05T16:00:00.000Z","expires_at":"2026-11-05T16:00:17.000Z","decision_hash":"6ad99a96bbcad047edccbc460cdf777a55464d26bdf892f67c95e3fae956c08d"}"#;
const V2_ADMISSION_WIRE: &str = r#"{"version":2,"tenant_id":"tenant-a","control_namespace_uuid":"11111111-1111-4111-8111-111111111111","broker_id":"22222222-2222-4222-8222-222222222222","incarnation_id":"33333333-3333-4333-8333-333333333333","credential_id":"44444444-4444-4444-8444-444444444444","binding_revision":3,"grant_revision":4,"origin_seq":123,"origin_hash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","authority_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","intent_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","quota":{"version":1,"max_active_workers":3,"max_cpu_millis":6000,"max_memory_bytes":3221225472,"max_timeout_seconds":3600},"issued_at":"2026-11-05T16:00:00.000Z","expires_at":"2026-11-05T16:00:17.000Z","decision_hash":"885c8e1a8339e82ce0f26e407580d53ae448a4a62c49c65b24621799c43dc049","basis":{"scope":"admission","source":"tenant_configuration","policy_snapshot":{"version":1,"decision_ttl_seconds":17,"approvals":[{"authority_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","image":"worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","profile":"kubernetes_held_pod_v1"}]},"policy_hash":"28f8497e773ebcc506e1eb88070d20efe11a644df8b9e22b448599c0eddbf5ce"}}"#;
const V2_RECONCILE_WIRE: &str = r#"{"version":2,"tenant_id":"tenant-a","control_namespace_uuid":"11111111-1111-4111-8111-111111111111","broker_id":"22222222-2222-4222-8222-222222222222","incarnation_id":"33333333-3333-4333-8333-333333333333","credential_id":"44444444-4444-4444-8444-444444444444","binding_revision":3,"grant_revision":4,"origin_seq":123,"origin_hash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","authority_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","intent_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","quota":{"version":1,"max_active_workers":3,"max_cpu_millis":6000,"max_memory_bytes":3221225472,"max_timeout_seconds":3600},"issued_at":"2026-11-05T16:00:00.000Z","expires_at":"2026-11-05T16:00:17.000Z","decision_hash":"dae60e3aa1284187607b1475a0d828379774976842724dc2ec487fba501de803","basis":{"scope":"reconcile","reservation_seq":99,"reservation_hash":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","admission_decision_hash":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"}}"#;

fn policy_for(intent: &KubernetesCreateIntentV1) -> KubernetesHeldCreatePolicyV1 {
    KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 17,
        approvals: vec![KubernetesHeldCreateApprovalV1 {
            authority_fingerprint: intent.authority.fingerprint(),
            image: intent.image.clone(),
            profile: intent.profile.clone(),
        }],
    }
}

fn claim(intent: &KubernetesCreateIntentV1) -> KubernetesCreateClaimV1 {
    KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    }
}

fn cancel(
    intent: &KubernetesCreateIntentV1,
    revision: u64,
    epoch: u64,
) -> KubernetesCreateCancelV1 {
    KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: revision,
        expected_create_epoch: epoch,
    }
}

fn assert_failure<T>(result: anyhow::Result<T>, expected: KubernetesCreateFailureV1) {
    let error = result.err().expect("the fixture request must be refused");
    assert_eq!(
        error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&expected)
    );
}

fn stored_event(store: &Store, kind: &str, create_id: Option<&str>) -> super::super::StoredEvent {
    let events = store
        .events(&EventQuery {
            kind: Some(kind.to_owned()),
            ..Default::default()
        })
        .expect("read the exact real audit event");
    let mut matches = events.into_iter().filter(|stored| {
        create_id.is_none_or(|wanted| stored.event.data["intent"]["create_id"] == wanted)
    });
    let event = matches.next().expect("real event exists");
    assert!(matches.next().is_none(), "fixture selected one exact event");
    event
}

fn migrate_fixture_to_schema9(fixture: &Fixture) {
    let backup = fixture._directory.path().join("before-policy-v9.sqlite");
    fixture
        .store
        .migrate_kubernetes_policy_v9(&backup)
        .expect("the fixture's real schema8 Store migrates to schema9");
}

#[test]
fn authorization_codec_preserves_v1_and_matches_independent_v2_goldens() {
    // The V1 fixture is the historical product type and locks both its exact wire
    // bytes and tuple commitment before V2 support changes the in-memory enum.
    let v1: AuthorizationEvidence = serde_json::from_str(V1_WIRE).unwrap();
    assert_eq!(serde_json::to_string(&v1).unwrap(), V1_WIRE);
    assert_eq!(v1.commitment().unwrap(), V1_HASH);

    let admission: AuthorizationEvidence = serde_json::from_str(V2_ADMISSION_WIRE)
        .expect("the closed version dispatcher must decode the independent V2 admission vector");
    assert_eq!(
        serde_json::to_string(&admission).unwrap(),
        V2_ADMISSION_WIRE
    );
    assert_eq!(admission.commitment().unwrap(), V2_ADMISSION_HASH);

    let reconcile: AuthorizationEvidence = serde_json::from_str(V2_RECONCILE_WIRE)
        .expect("the closed version dispatcher must decode the independent V2 reconcile vector");
    assert_eq!(
        serde_json::to_string(&reconcile).unwrap(),
        V2_RECONCILE_WIRE
    );
    assert_eq!(reconcile.commitment().unwrap(), V2_RECONCILE_HASH);

    let mut malformed_basis: Value = serde_json::from_str(V2_ADMISSION_WIRE).unwrap();
    malformed_basis["basis"]["unrecognized"] = json!(true);
    assert!(serde_json::from_value::<AuthorizationEvidence>(malformed_basis).is_err());
    let mut crossed_version: Value = serde_json::from_str(V2_ADMISSION_WIRE).unwrap();
    crossed_version["version"] = json!(1);
    assert!(serde_json::from_value::<AuthorizationEvidence>(crossed_version).is_err());
}

#[test]
fn admission_cap_binds_the_exact_current_policy_without_allocating() {
    let f = Fixture::new();
    migrate_fixture_to_schema9(&f);
    let policy = policy_for(&f.intent);
    let quota = QuotaPolicyV1::default();
    let head_before = f.store.head().unwrap();
    let usage_before = f.store.worker_quota(&quota).unwrap();
    assert_eq!(usage_before.held_workers, 0);

    let authorization = f
        .store
        .authorize_kubernetes_create_admission(&f.identity, &f.intent, &policy, &quota)
        .expect("a matching exact tenant policy on schema9 should create admission evidence");
    let evidence = serde_json::to_value(&authorization.evidence).unwrap();
    assert_eq!(evidence["version"], 2);
    assert_eq!(evidence["basis"]["scope"], "admission");
    assert_eq!(evidence["basis"]["source"], "tenant_configuration");
    assert_eq!(
        evidence["basis"]["policy_snapshot"],
        serde_json::to_value(policy.canonicalized().unwrap()).unwrap()
    );
    assert_eq!(
        evidence["basis"]["policy_hash"],
        policy.policy_hash().unwrap()
    );
    assert_eq!(evidence["quota"], serde_json::to_value(&quota).unwrap());
    let issued =
        crate::broker_registry::parse_time(evidence["issued_at"].as_str().unwrap()).unwrap();
    let expires =
        crate::broker_registry::parse_time(evidence["expires_at"].as_str().unwrap()).unwrap();
    assert_eq!(expires, issued + chrono::Duration::seconds(17));
    assert_eq!(f.store.head().unwrap(), head_before);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 0);

    let mut rejected_policy = policy.clone();
    rejected_policy.approvals[0].image = format!("other@sha256:{}", "b".repeat(64));
    assert_failure(
        f.store.authorize_kubernetes_create_admission(
            &f.identity,
            &f.intent,
            &rejected_policy,
            &quota,
        ),
        KubernetesCreateFailureV1::PermissionDenied,
    );

    let mut different_image = f.intent.clone();
    different_image.image = format!("other@sha256:{}", "b".repeat(64));
    assert!(!policy.approves(&different_image).unwrap());
    let mut different_authority = f.intent.clone();
    different_authority.authority = crate::runtime::kubernetes::KubernetesAuthorityV1::new(
        Uuid::new_v4(),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap();
    assert!(!policy.approves(&different_authority).unwrap());
    assert_failure(
        f.store.authorize_kubernetes_create_admission(
            &f.identity,
            &different_authority,
            &policy_for(&different_authority),
            &quota,
        ),
        KubernetesCreateFailureV1::PermissionDenied,
    );
    assert_eq!(f.store.head().unwrap(), head_before);

    // A four-minute future limit leaves setup margin without sleeping or
    // treating elapsed time as a latency measurement.
    let binding_limit =
        crate::broker_registry::format_time(Utc::now() + ChronoDuration::minutes(4));
    let binding_limited = Fixture::new_expiries(&binding_limit, "2090-01-01T00:00:00.000Z");
    migrate_fixture_to_schema9(&binding_limited);
    let mut long_ttl = policy_for(&binding_limited.intent);
    long_ttl.decision_ttl_seconds = 300;
    let binding_cap = binding_limited
        .store
        .authorize_kubernetes_create_admission(
            &binding_limited.identity,
            &binding_limited.intent,
            &long_ttl,
            &quota,
        )
        .expect("binding-limited admission evidence is valid");
    assert_eq!(
        serde_json::to_value(&binding_cap.evidence).unwrap()["expires_at"],
        binding_limit
    );

    let grant_limit = crate::broker_registry::format_time(Utc::now() + ChronoDuration::minutes(4));
    let grant_limited = Fixture::new_expiries("2090-01-01T00:00:00.000Z", &grant_limit);
    migrate_fixture_to_schema9(&grant_limited);
    let mut long_ttl = policy_for(&grant_limited.intent);
    long_ttl.decision_ttl_seconds = 300;
    let grant_cap = grant_limited
        .store
        .authorize_kubernetes_create_admission(
            &grant_limited.identity,
            &grant_limited.intent,
            &long_ttl,
            &quota,
        )
        .expect("grant-limited admission evidence is valid");
    assert_eq!(
        serde_json::to_value(&grant_cap.evidence).unwrap()["expires_at"],
        grant_limit
    );
}

#[test]
fn reconciliation_uses_the_original_reservations_when_inactive_and_over_quota() {
    let Fixture {
        _directory,
        store,
        identity,
        intent,
    } = Fixture::new();
    let original_cap = store
        .authorize_held_create_for_test(&identity, &intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    let (reservation_receipt, reserved, replayed) = store
        .reserve_kubernetes_create(&identity, &original_cap, &intent)
        .unwrap();
    assert!(!replayed);
    let claim = claim(&intent);
    store
        .claim_kubernetes_create(&identity, &original_cap, &claim)
        .unwrap();

    let mut cancel_intent = intent.clone();
    cancel_intent.create_id = Uuid::new_v4().to_string();
    cancel_intent.session_id = Uuid::new_v4().to_string();
    let cancel_cap = store
        .authorize_held_create_for_test(
            &identity,
            &cancel_intent,
            &QuotaPolicyV1::default(),
            EXPIRY,
        )
        .unwrap();
    store
        .reserve_kubernetes_create(&identity, &cancel_cap, &cancel_intent)
        .unwrap();

    let db_path = _directory.path().join("tenant.sqlite");
    let backup = _directory.path().join("before-policy-v9.sqlite");
    store.migrate_kubernetes_policy_v9(&backup).unwrap();
    drop(store);
    let store =
        Store::open_existing_for_tenant(&db_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert!(
        !store.supervision_state().unwrap().active,
        "reopen does not recreate supervision"
    );

    let lowered_quota = QuotaPolicyV1 {
        max_active_workers: 0,
        max_cpu_millis: 0,
        max_memory_bytes: 0,
        ..QuotaPolicyV1::default()
    };
    let use_cap = store
        .authorize_kubernetes_create_reconciliation(&identity, &intent.create_id, &lowered_quota)
        .expect(
            "the exact real schema8 reservation remains reconcilable after migration and reopen",
        );
    let evidence = serde_json::to_value(&use_cap.evidence).unwrap();
    assert_eq!(evidence["basis"]["scope"], "reconcile");
    assert_eq!(
        evidence["basis"]["reservation_seq"],
        reservation_receipt.audit_seq
    );
    assert_eq!(
        evidence["basis"]["reservation_hash"],
        reservation_receipt.audit_hash
    );
    assert_eq!(reserved.state, KubernetesCreateStateV1::Reserved);
    assert_eq!(evidence["quota"]["max_active_workers"], 0);

    let result = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        claim_operation_id: claim.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        },
    };
    let committed = store
        .record_kubernetes_create_result(&identity, &use_cap, &result)
        .unwrap();
    assert!(committed.result() == &result);
    assert_eq!(
        committed.snapshot().state,
        KubernetesCreateStateV1::OwnershipUnknown
    );
    assert!(committed.snapshot().quota_held);
    assert_eq!(store.worker_quota(&lowered_quota).unwrap().held_workers, 2);

    let cancel_use_cap = store
        .authorize_kubernetes_create_reconciliation(
            &identity,
            &cancel_intent.create_id,
            &lowered_quota,
        )
        .unwrap();
    let cancellation = cancel(&cancel_intent, 1, 1);
    let (_, cancelled, replayed) = store
        .cancel_kubernetes_create(&identity, &cancel_use_cap, &cancellation)
        .unwrap();
    assert!(!replayed);
    assert_eq!(
        cancelled.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert_eq!(store.worker_quota(&lowered_quota).unwrap().held_workers, 1);
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn v2_scope_is_checked_before_replay_lookup_and_schema_boundaries() {
    let Fixture {
        _directory,
        store,
        identity,
        intent,
    } = Fixture::new();
    let legacy_cap = store
        .authorize_held_create_for_test(&identity, &intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    store
        .reserve_kubernetes_create(&identity, &legacy_cap, &intent)
        .unwrap();
    let request = claim(&intent);
    let (historical_claim, _) = store
        .claim_kubernetes_create(&identity, &legacy_cap, &request)
        .unwrap();
    let backup_path = _directory.path().join("before-policy-v9.sqlite");
    store.migrate_kubernetes_policy_v9(&backup_path).unwrap();

    let policy = policy_for(&intent);
    let admission_cap = store
        .authorize_kubernetes_create_admission(
            &identity,
            &intent,
            &policy,
            &QuotaPolicyV1::default(),
        )
        .expect("create an actual V2 Admission capability after schema9 migration");
    let reconcile_cap = store
        .authorize_kubernetes_create_reconciliation(
            &identity,
            &intent.create_id,
            &QuotaPolicyV1::default(),
        )
        .expect("create a fresh exact V2 Reconcile capability for the existing reservation");
    let head = store.head().unwrap();

    let observed = store
        .observe_kubernetes_create_claim(&identity, &reconcile_cap, &request)
        .unwrap();
    assert_eq!(
        observed.disposition,
        KubernetesCreateDispositionV1::ObserveOnly
    );
    assert_eq!(observed.receipt, historical_claim.receipt);
    assert!(observed.snapshot == historical_claim.snapshot);
    assert_eq!(store.head().unwrap(), head);

    let error = store
        .claim_kubernetes_create(&identity, &reconcile_cap, &request)
        .err()
        .expect("Reconcile capability cannot mutate an existing Claim");
    assert_eq!(
        error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&KubernetesCreateFailureV1::PermissionDenied)
    );
    let error = store
        .observe_kubernetes_create_claim(&identity, &admission_cap, &request)
        .err()
        .expect("Admission capability cannot read historical claims");
    assert_eq!(
        error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&KubernetesCreateFailureV1::PermissionDenied)
    );
    let error = store
        .reserve_kubernetes_create(&identity, &legacy_cap, &intent)
        .err()
        .expect("a V1 capability cannot replay after schema9");
    assert_eq!(
        error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&KubernetesCreateFailureV1::MigrationRequired)
    );
    assert_eq!(store.head().unwrap(), head);

    let schema8 =
        Store::open_existing_for_tenant(&backup_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    let error = schema8
        .reserve_kubernetes_create(&identity, &admission_cap, &intent)
        .err()
        .expect("a V2 capability cannot be applied to schema8");
    assert_eq!(
        error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&KubernetesCreateFailureV1::MigrationRequired)
    );
    assert_eq!(
        schema8.head().unwrap(),
        head - 1,
        "the schema8 copy has the pre-marker prefix only"
    );
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
enum BasisForRehash {
    Admission {
        source: AdmissionSourceForRehash,
        policy_snapshot: KubernetesHeldCreatePolicyV1,
        policy_hash: String,
    },
    Reconcile {
        reservation_seq: i64,
        reservation_hash: String,
        admission_decision_hash: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionSourceForRehash {
    TenantConfiguration,
}

fn rehash_v2_decision(decision: &mut Value) {
    let version = decision["version"].as_u64().unwrap() as u32;
    let tenant_id = decision["tenant_id"].as_str().unwrap().to_owned();
    let namespace = decision["control_namespace_uuid"]
        .as_str()
        .unwrap()
        .to_owned();
    let broker = decision["broker_id"].as_str().unwrap().to_owned();
    let incarnation = decision["incarnation_id"].as_str().unwrap().to_owned();
    let credential = decision["credential_id"].as_str().unwrap().to_owned();
    let binding_revision = decision["binding_revision"].as_u64().unwrap();
    let grant_revision = decision["grant_revision"].as_u64().unwrap();
    let origin_seq = decision["origin_seq"].as_i64().unwrap();
    let origin_hash = decision["origin_hash"].as_str().unwrap().to_owned();
    let fingerprint = decision["authority_fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let intent_hash = decision["intent_hash"].as_str().unwrap().to_owned();
    let quota: QuotaPolicyV1 = serde_json::from_value(decision["quota"].clone()).unwrap();
    let issued_at = decision["issued_at"].as_str().unwrap().to_owned();
    let expires_at = decision["expires_at"].as_str().unwrap().to_owned();
    let basis: BasisForRehash = serde_json::from_value(decision["basis"].clone()).unwrap();
    decision["decision_hash"] = json!(
        crate::control::content_hash(
            "ai-guard:kubernetes-create-authorization:v2",
            &(
                version,
                &tenant_id,
                &namespace,
                &broker,
                &incarnation,
                &credential,
                binding_revision,
                grant_revision,
                origin_seq,
                &origin_hash,
                &fingerprint,
                &intent_hash,
                &quota,
                &issued_at,
                &expires_at,
                &basis,
            ),
        )
        .unwrap()
    );
}

fn rewrite_event_payload(store: &Store, seq: i64, mutate: impl FnOnce(&mut Value)) {
    let db = store.db.lock().unwrap();
    let (received_at, previous_hash, payload): (String, String, String) = db
        .query_row(
            "SELECT received_at,prev_hash,payload FROM events WHERE seq=?1",
            [seq],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let mut event: Value = serde_json::from_str(&payload).unwrap();
    mutate(&mut event);
    let event: Event = serde_json::from_value(event).unwrap();
    let payload = serde_json::to_string(&event).unwrap();
    let hash = super::super::super::digest(seq, &received_at, &previous_hash, &payload);
    db.execute(
        "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
        params![payload, hash, seq],
    )
    .unwrap();
    db.execute("UPDATE quota SET bytes=(SELECT COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events) WHERE id=1", []).unwrap();
}

fn assert_diagnostic_only_after_rehashed_history(store: &Store) {
    let before = {
        let db = store.db.lock().unwrap();
        db.query_row(
            "SELECT seq,hash,(SELECT count(*) FROM events),(SELECT bytes FROM quota WHERE id=1) FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
        ).unwrap()
    };
    match store.integrity() {
        Ok(integrity) => assert!(
            !integrity.valid,
            "the self-consistent mutant is not valid history"
        ),
        Err(error) => assert_eq!(
            error.downcast_ref::<KubernetesCreateFailureV1>(),
            Some(&KubernetesCreateFailureV1::Unavailable)
        ),
    }
    assert!(
        !store.healthy(),
        "invalid history leaves Store diagnostic-only"
    );
    assert!(
        store
            .append(Event::new(
                "fixture.must_not_append_after_invalid_v2_history"
            ))
            .is_err()
    );
    let after = {
        let db = store.db.lock().unwrap();
        db.query_row(
            "SELECT seq,hash,(SELECT count(*) FROM events),(SELECT bytes FROM quota WHERE id=1) FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
        ).unwrap()
    };
    assert_eq!(
        after, before,
        "diagnostic-only history does not append or alter quota"
    );
}

#[test]
fn v2_history_requires_post_marker_origin_and_the_exact_original_reservation() {
    // First mutant: move a real V2 Admission event to the exact authenticated
    // schema8 head immediately before the policy marker, then rehash it.
    let f = Fixture::new();
    let backup = f._directory.path().join("before-policy-v9-origin.sqlite");
    f.store.migrate_kubernetes_policy_v9(&backup).unwrap();
    let policy = policy_for(&f.intent);
    let admission = f
        .store
        .authorize_kubernetes_create_admission(
            &f.identity,
            &f.intent,
            &policy,
            &QuotaPolicyV1::default(),
        )
        .expect("positive exact policy capability");
    let (_, _, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &admission, &f.intent)
        .unwrap();
    assert!(!replayed);
    assert!(
        f.store.integrity().unwrap().valid,
        "unmodified V2 event is the positive witness"
    );
    let marker = stored_event(&f.store, POLICY_GENESIS_KIND, None);
    let genesis = stored_event(&f.store, CREATE_GENESIS_KIND, None);
    assert!(genesis.seq < marker.seq);
    let pre_marker_seq = marker.event.data["previous_seq"].as_i64().unwrap();
    let pre_marker_hash = marker.event.data["previous_hash"].as_str().unwrap();
    assert!(genesis.seq < pre_marker_seq && pre_marker_seq < marker.seq);
    let actual_pre_marker_hash: String = f
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT hash FROM events WHERE seq=?1",
            [pre_marker_seq],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actual_pre_marker_hash, pre_marker_hash);
    let reserve = stored_event(
        &f.store,
        "control.kubernetes.create.reserved",
        Some(&f.intent.create_id),
    );
    rewrite_event_payload(&f.store, reserve.seq, |event| {
        let decision = &mut event["data"]["control_decision"];
        decision["origin_seq"] = json!(pre_marker_seq);
        decision["origin_hash"] = json!(pre_marker_hash);
        rehash_v2_decision(decision);
    });
    assert_diagnostic_only_after_rehashed_history(&f.store);

    // Second mutant: the Result for create A is rehashed to cite a different,
    // real Reservation B and its actual Admission decision.
    let Fixture {
        _directory,
        store,
        identity,
        intent,
    } = Fixture::new();
    let seed = v1_authorization_for_store(&store, &identity, &intent);
    store
        .reserve_kubernetes_create(&identity, &seed, &intent)
        .unwrap();
    let request = claim(&intent);
    store
        .claim_kubernetes_create(&identity, &seed, &request)
        .unwrap();
    let mut other = intent.clone();
    other.create_id = Uuid::new_v4().to_string();
    other.session_id = Uuid::new_v4().to_string();
    let other_seed = store
        .authorize_held_create_for_test(&identity, &other, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    store
        .reserve_kubernetes_create(&identity, &other_seed, &other)
        .unwrap();
    let backup = _directory.path().join("before-policy-v9-reference.sqlite");
    store.migrate_kubernetes_policy_v9(&backup).unwrap();
    let reconcile = store
        .authorize_kubernetes_create_reconciliation(
            &identity,
            &intent.create_id,
            &QuotaPolicyV1::default(),
        )
        .expect("exact original reservation produces reconcile capability");
    let result = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        claim_operation_id: request.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        },
    };
    store
        .record_kubernetes_create_result(&identity, &reconcile, &result)
        .unwrap();
    assert!(
        store.integrity().unwrap().valid,
        "unmodified exact reconciliation result is the positive witness"
    );
    let other_reserve = stored_event(
        &store,
        "control.kubernetes.create.reserved",
        Some(&other.create_id),
    );
    let other_record: Value = serde_json::from_value(other_reserve.event.data.clone()).unwrap();
    let other_decision = &other_record["control_decision"];
    let result_event = stored_event(
        &store,
        "control.kubernetes.create.result",
        Some(&intent.create_id),
    );
    rewrite_event_payload(&store, result_event.seq, |event| {
        let decision = &mut event["data"]["control_decision"];
        decision["basis"]["reservation_seq"] = json!(other_reserve.seq);
        decision["basis"]["reservation_hash"] = json!(other_reserve.hash);
        decision["basis"]["admission_decision_hash"] = other_decision["decision_hash"].clone();
        rehash_v2_decision(decision);
    });
    assert_diagnostic_only_after_rehashed_history(&store);
}

fn v1_authorization_for_store(
    store: &Store,
    identity: &crate::broker_registry::BrokerIdentityV1,
    intent: &KubernetesCreateIntentV1,
) -> KubernetesCreateAuthorization {
    store
        .authorize_held_create_for_test(identity, intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap()
}
