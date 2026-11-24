//! `egress_store_*`: the egress journal on real private SQLite tenant stores.
//! Workers are driven to Running through the real typed transitions of the
//! shared software fixture; their runtime facts stay synthetic.
use super::*;
use crate::{
    control::{egress_decision::*, *},
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::RelayEndpointV1,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

#[allow(dead_code)]
#[path = "../../tests/support/relay_admission_fixture.rs"]
mod software_workers;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const NOW: i64 = 1_800_000_000_000;

/// A real observe-profile admission launched with broker egress.
fn egress_admission(store: &Store) -> AdmissionV1 {
    let mut admission = software_workers::decision_admission(store);
    let request = DecisionRequest {
        profile: "observe".into(),
        request_id: Uuid::new_v4().to_string(),
        ..admission.decision_request.clone()
    };
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&Policy::default(), &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    admission.receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash,
    };
    admission.launch.profile = "observe".into();
    admission.launch.egress = Some(EgressLaunchV1::broker_proxy(3128).unwrap());
    admission.decision_request = request;
    admission
}

fn binding(worker: &WorkerV1) -> EgressBindingV1 {
    EgressBindingV1 {
        session_id: worker.launch.session_id.clone(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        channel_id: IdV1::random(),
        epoch: 1,
        context_digest: crate::egress::ids::DigestV1::of(b"fixture"),
    }
}

fn request(binding: &EgressBindingV1, authority: &str) -> EgressDecideRequestV1 {
    EgressDecideRequestV1 {
        version: 1,
        decision_id: IdV1::random(),
        binding: binding.clone(),
        flow_id: 1,
        authority: authority.into(),
    }
}

fn channel_open(binding: &EgressBindingV1) -> EgressObserveRequestV1 {
    EgressObserveRequestV1 {
        version: 1,
        observation_id: IdV1::random(),
        observation: EgressObservationV1::ChannelOpened {
            binding: binding.clone(),
        },
    }
}

fn configured_policy() -> EgressTenantPolicyV1 {
    EgressTenantPolicyV1 {
        version: 1,
        revision: 1,
        allow: vec!["fixture.egress.test:8443".into()],
        grantable: EgressGrantableV1::PublicNames,
        grantable_destinations: vec![],
        max_grant_seconds: 300,
    }
}

fn policy() -> EgressPolicySnapshot {
    configured_policy().snapshot().unwrap()
}

fn store(directory: &Path, tenant: &str) -> Store {
    software_workers::fresh(directory, tenant, 10_000)
}

/// A new private (0700, canonical, symlink-free) parent for tenant stores.
fn private_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap()
}

#[test]
fn sqlite_sibling_keeps_egress_policy_when_gateway_expands_redaction() {
    let directory = private_directory();
    let payload_limit = crate::audit::DEFAULT_PHYSICAL_BYTES * 2;
    let store = std::sync::Arc::new(
        store(directory.path(), "tenant-a")
            .with_physical_bytes(payload_limit)
            .unwrap()
            .with_egress_policy(&configured_policy())
            .unwrap(),
    );
    let masking_union = Redactor::new(vec!["synthetic-neighbor-material".into()]);
    let rebound = store.with_additional_redaction(&masking_union).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&store, &rebound));
    assert_eq!(rebound.egress_policy().digest, store.egress_policy().digest);
    assert_eq!(
        rebound.egress_policy().revision,
        store.egress_policy().revision
    );
    assert_eq!(rebound.physical_bytes(), payload_limit);
}

#[test]
fn egress_store_allow_is_durable_bound_to_the_running_egress_worker_and_idempotent() {
    let directory = private_directory();
    let store = store(directory.path(), "tenant-a");
    let worker = software_workers::running_from(&store, egress_admission(&store));
    assert_eq!(worker.state, WorkerStateV1::Running);
    let request = request(&binding(&worker), "fixture.egress.test:8443");
    let decision = store.egress_decide(&request, &policy(), NOW).unwrap();
    assert_eq!(decision.outcome, EgressOutcomeV1::Allow);
    assert!(decision.allows(&request));
    assert!(decision.receipt.audit_seq > 0);
    // Exact retry: same receipt, no new event.
    let head = store.head().unwrap();
    assert_eq!(
        store.egress_decide(&request, &policy(), NOW + 5).unwrap(),
        decision
    );
    assert_eq!(store.head().unwrap(), head);
    // Same id, other content: conflict, nothing appended.
    let mut divergent = request.clone();
    divergent.authority = "other.example.test:443".into();
    let error = store.egress_decide(&divergent, &policy(), NOW).unwrap_err();
    assert_eq!(
        error.downcast_ref::<EgressJournalError>(),
        Some(&EgressJournalError::Conflict)
    );
    assert_eq!(store.head().unwrap(), head);
    let events = store
        .egress_session_events(&worker.launch.session_id, 10)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.kind, DECISION_KIND);
    assert_eq!(events[0].event.provenance, "observed");
    assert_eq!(events[0].event.coverage, "partial");
}

#[test]
fn egress_store_connection_correlates_with_its_durable_decision() {
    let directory = private_directory();
    let store = store(directory.path(), "tenant-a");
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let request = request(&binding(&worker), "fixture.egress.test:8443");
    let decision = store.egress_decide(&request, &policy(), NOW).unwrap();
    assert!(decision.allows(&request));

    let observation = EgressObserveRequestV1 {
        version: 1,
        observation_id: IdV1::random(),
        observation: EgressObservationV1::Connection {
            binding: request.binding.clone(),
            decision_id: decision.decision_id.clone(),
            flow_id: request.flow_id,
            connected: true,
            failure: None,
            peer: Some("192.0.2.1:8443".into()),
            resolver: "fixture".into(),
        },
    };
    let observed = store.egress_observe(&observation).unwrap();
    assert!(observed.audit_seq > decision.receipt.audit_seq);
    let head = store.head().unwrap();
    assert_eq!(store.egress_observe(&observation).unwrap(), observed);
    assert_eq!(store.head().unwrap(), head);

    let mut different_flow = observation.clone();
    different_flow.observation_id = IdV1::random();
    let EgressObservationV1::Connection { flow_id, .. } = &mut different_flow.observation else {
        unreachable!("fixture is a connection observation");
    };
    *flow_id = request.flow_id + 1;
    let error = store.egress_observe(&different_flow).unwrap_err();
    assert_eq!(
        error.downcast_ref::<EgressJournalError>(),
        Some(&EgressJournalError::Invalid)
    );
    assert_eq!(store.head().unwrap(), head);

    let events = store
        .egress_session_events(&request.binding.session_id, 10)
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(receipt(&events[0]), decision.receipt);
    assert_eq!(receipt(&events[1]), observed);
    assert_eq!(events[1].event.kind, "egress.connection");
    assert_eq!(
        events[1].event.data,
        serde_json::to_value(&observation.observation).unwrap()
    );
}

#[test]
fn egress_store_retention_preserves_decision_and_channel_receipts() {
    use crate::audit::retention::{
        AuditRetentionPolicyV1, ReclaimClassV1, RetentionApplyRequest, verify_archive_chain,
    };
    use std::collections::BTreeSet;

    let directory = private_directory();
    let path = directory.path();
    let store = store(path, "tenant-a");
    for (version, migrate) in [
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ]
    .into_iter()
    .enumerate()
    {
        migrate(
            &store,
            &path.join(format!("before-v{}.sqlite", version + 7)),
        )
        .unwrap();
    }
    store
        .install_audit_retention(&path.join("before-retention.sqlite"))
        .unwrap();
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let binding = binding(&worker);
    let open = channel_open(&binding);
    let opening = store.egress_observe(&open).unwrap();
    let request = request(&binding, "fixture.egress.test:8443");
    let policy = policy();
    let decision = store.egress_decide(&request, &policy, NOW).unwrap();
    assert_eq!(decision.outcome, EgressOutcomeV1::Allow);
    store.append(Event::new("tool.metric")).unwrap();
    store.append(Event::new("tool.metric")).unwrap();
    store.append(Event::new("tool.metric")).unwrap();

    let retention = AuditRetentionPolicyV1 {
        version: 1,
        min_age_seconds: 86_400,
        keep_recent_events: 1,
        max_segment_events: 1_000,
        classes: vec![ReclaimClassV1::Ordinary],
    };
    let now = chrono::Utc::now() + chrono::Duration::days(2);
    let kept = BTreeSet::new();
    let plan = store
        .audit_retention_plan(&retention, now, None, &kept)
        .unwrap();
    assert!(plan.plan.last_seq > decision.receipt.audit_seq);
    let archive = path.join("archive");
    let applied = store
        .apply_audit_retention(&RetentionApplyRequest {
            policy: &retention,
            now,
            through_seq: plan.plan.last_seq,
            expect_plan_sha256: &plan.plan_sha256,
            archive_dir: &archive,
            kept_positions: &kept,
        })
        .unwrap();
    assert!(applied.reclaimed_events > 0);
    drop(store);

    let reopened = Store::open_existing_for_tenant(
        &path.join("tenant-a.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    let archived = [archive];
    let verified = reopened.verify_audit_archives(&archived).unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
    let ledger = reopened.audit_retention_ledger().unwrap();
    assert_eq!(
        verify_archive_chain("tenant-a", &ledger, &archived).verdict,
        "verified"
    );

    let events = reopened
        .egress_session_events(&binding.session_id, 10)
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| receipt(event) == decision.receipt),
        "egress_decision_retention_lost"
    );
    assert!(events.iter().any(|event| receipt(event) == opening));
    assert!(
        plan.plan
            .kept_positions
            .contains(&decision.receipt.audit_seq)
    );
    let head = reopened.head().unwrap();
    assert_eq!(reopened.egress_observe(&open).unwrap(), opening);
    assert_eq!(
        reopened.egress_decide(&request, &policy, NOW + 1).unwrap(),
        decision
    );
    assert_eq!(reopened.head().unwrap(), head);
}

#[test]
fn egress_store_refuses_workers_without_egress_unknown_or_foreign_bindings() {
    let directory = private_directory();
    let a = store(directory.path(), "tenant-a");
    let b = store(directory.path(), "tenant-b");
    let plain = software_workers::running_now(&a);
    let decided = a
        .egress_decide(
            &request(&binding(&plain), "fixture.egress.test:8443"),
            &policy(),
            NOW,
        )
        .unwrap();
    assert_eq!(
        (decided.outcome, decided.reason),
        (
            EgressOutcomeV1::Deny,
            EgressDecisionReasonV1::EgressNotLaunched
        )
    );
    // A worker of tenant B is unknown in tenant A's store.
    let foreign = software_workers::running_from(&b, egress_admission(&b));
    let decided = a
        .egress_decide(
            &request(&binding(&foreign), "fixture.egress.test:8443"),
            &policy(),
            NOW,
        )
        .unwrap();
    assert_eq!(
        (decided.outcome, decided.reason),
        (
            EgressOutcomeV1::Deny,
            EgressDecisionReasonV1::BindingMismatch
        )
    );
    // A wrong attempt of a real worker is a mismatch too.
    let own = software_workers::running_from(&a, egress_admission(&a));
    let mut wrong = binding(&own);
    wrong.attempt_id = Uuid::new_v4().to_string();
    let decided = a
        .egress_decide(&request(&wrong, "fixture.egress.test:8443"), &policy(), NOW)
        .unwrap();
    assert_eq!(decided.reason, EgressDecisionReasonV1::BindingMismatch);
    // Every refusal is itself durable and visible.
    assert!(
        a.egress_session_events(&own.launch.session_id, 10)
            .unwrap()
            .iter()
            .all(|e| e.event.kind == DECISION_KIND)
    );
}

#[test]
fn egress_store_unknown_destination_is_a_durable_review_and_renewal_closes_unknown_decisions() {
    let directory = private_directory();
    let store = store(directory.path(), "tenant-a");
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let binding = binding(&worker);
    let review = store
        .egress_decide(&request(&binding, "docs.example.test:443"), &policy(), NOW)
        .unwrap();
    assert_eq!(review.outcome, EgressOutcomeV1::Review);
    let allowed = request(&binding, "fixture.egress.test:8443");
    let decision = store.egress_decide(&allowed, &policy(), NOW).unwrap();
    let renew = EgressRenewRequestV1 {
        version: 1,
        binding: binding.clone(),
        decisions: vec![decision.decision_id.clone(), IdV1::random()],
    };
    let answer = store.egress_renew(&renew, &policy(), NOW + 1_000).unwrap();
    assert_eq!(
        answer.revoked.len(),
        1,
        "an unknown decision never keeps a flow open"
    );
    assert!(!answer.revoked.contains(&decision.decision_id));
    // Narrowing the operator policy revokes the policy-backed flow.
    let narrowed = EgressTenantPolicyV1 {
        version: 1,
        revision: 2,
        allow: vec![],
        grantable: EgressGrantableV1::None,
        grantable_destinations: vec![],
        max_grant_seconds: 60,
    }
    .snapshot()
    .unwrap();
    let answer = store.egress_renew(&renew, &narrowed, NOW + 2_000).unwrap();
    assert!(answer.revoked.contains(&decision.decision_id));
}

#[test]
fn egress_store_channel_open_requires_the_current_running_binding() {
    let directory = private_directory();
    let a = store(directory.path(), "tenant-a");
    let b = store(directory.path(), "tenant-b");
    let own = software_workers::running_from(&a, egress_admission(&a));
    let foreign = software_workers::running_from(&b, egress_admission(&b));
    let plain = software_workers::running_now(&a);
    let admission = software_workers::supervised_request(egress_admission(&a));
    let reserved = software_workers::admit_supervised(&a, &admission)
        .worker
        .unwrap();
    assert_eq!(reserved.state, WorkerStateV1::Reserved);

    let wrong_attempt = EgressBindingV1 {
        attempt_id: reserved.attempt.attempt_id.clone(),
        ..binding(&own)
    };
    let wrong_session = EgressBindingV1 {
        session_id: reserved.launch.session_id.clone(),
        ..binding(&own)
    };
    let head = a.head().unwrap();
    for rejected in [
        binding(&reserved),
        binding(&plain),
        binding(&foreign),
        wrong_attempt,
        wrong_session,
    ] {
        let refusal = a.egress_observe(&channel_open(&rejected)).unwrap_err();
        assert_eq!(
            refusal.downcast_ref::<EgressJournalError>(),
            Some(&EgressJournalError::Invalid)
        );
        assert_eq!(a.head().unwrap(), head);
    }
}

#[test]
fn egress_store_stopping_revokes_renewal_and_preserves_the_opening_receipt() {
    let directory = private_directory();
    let store = store(directory.path(), "tenant-a");
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let binding = binding(&worker);
    let opening = channel_open(&binding);
    let receipt = store.egress_observe(&opening).unwrap();
    let policy = policy();
    let decision_request = request(&binding, "fixture.egress.test:8443");
    let decision = store
        .egress_decide(&decision_request, &policy, NOW)
        .unwrap();
    assert!(decision.allows(&decision_request));
    let renew = EgressRenewRequestV1 {
        version: 1,
        binding: binding.clone(),
        decisions: vec![decision.decision_id.clone()],
    };
    let live = store.egress_renew(&renew, &policy, NOW + 1).unwrap();
    assert!(live.revoked.is_empty() && live.expired.is_empty());

    let stopped = store
        .mutate_worker(&software_workers::mutation(
            &worker,
            WorkerTransitionV1::Stop,
        ))
        .unwrap()
        .worker;
    assert_eq!(stopped.state, WorkerStateV1::Stopping);
    let head = store.head().unwrap();
    assert_eq!(store.egress_observe(&opening).unwrap(), receipt);
    let closed = store.egress_renew(&renew, &policy, NOW + 2).unwrap();
    assert_eq!(closed.revoked, [decision.decision_id]);
    assert!(closed.expired.is_empty());
    let refusal = store.egress_observe(&channel_open(&binding)).unwrap_err();
    assert_eq!(
        refusal.downcast_ref::<EgressJournalError>(),
        Some(&EgressJournalError::Invalid)
    );
    assert_eq!(store.head().unwrap(), head);
    let terminal = EgressObserveRequestV1 {
        version: 1,
        observation_id: IdV1::random(),
        observation: EgressObservationV1::ChannelClosed {
            binding,
            reason: "owner_exit".into(),
            observations_lost: 0,
        },
    };
    assert!(store.egress_observe(&terminal).unwrap().audit_seq > head);
}

#[test]
fn egress_store_observations_are_idempotent_and_the_namespace_is_reserved() {
    let directory = private_directory();
    let store = store(directory.path(), "tenant-a");
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let observed = channel_open(&binding(&worker));
    let first = store.egress_observe(&observed).unwrap();
    assert_eq!(store.egress_observe(&observed).unwrap(), first);
    let mut divergent = observed.clone();
    divergent.observation = EgressObservationV1::ChannelClosed {
        binding: binding(&worker),
        reason: "helper_exit".into(),
        observations_lost: 0,
    };
    assert!(store.egress_observe(&divergent).is_err());
    // No ordinary ingestion path may write an egress fact.
    let mut forged = crate::model::Event::new("egress.decision");
    forged.session_id = Some(worker.launch.session_id.clone());
    assert!(store.append(forged).is_err());
}

#[test]
fn egress_store_masks_text_and_refuses_canonical_redaction_collisions() {
    let directory = private_directory();
    let store = std::sync::Arc::new(store(directory.path(), "tenant-a"));
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let binding = binding(&worker);
    let private_marker = binding.channel_id.as_str().to_owned();
    let masked = store
        .with_additional_redaction(&Redactor::new(vec![private_marker.clone()]))
        .unwrap();
    let head = store.head().unwrap();

    let observation = masked.egress_observe(&channel_open(&binding)).unwrap_err();
    assert_eq!(
        observation.downcast_ref::<EgressJournalError>(),
        Some(&EgressJournalError::Invalid)
    );
    assert_eq!(store.head().unwrap(), head);

    let decision = masked
        .egress_decide(
            &request(&binding, "fixture.egress.test:8443"),
            &policy(),
            NOW,
        )
        .unwrap_err();
    assert_eq!(
        decision.downcast_ref::<EgressJournalError>(),
        Some(&EgressJournalError::Invalid)
    );
    assert_eq!(store.head().unwrap(), head);
    assert!(
        store
            .egress_session_events(&worker.launch.session_id, 10)
            .unwrap()
            .is_empty()
    );

    let mut ordinary = Event::new("fixture.note");
    ordinary.data = json!({"text": private_marker});
    let note = masked.append(ordinary).unwrap();
    assert_eq!(note.event.data["text"], "[REDACTED]");
}
