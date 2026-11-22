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

fn policy() -> EgressPolicySnapshot {
    EgressTenantPolicyV1 {
        version: 1,
        revision: 1,
        allow: vec!["fixture.egress.test:8443".into()],
        grantable: EgressGrantableV1::PublicNames,
        grantable_destinations: vec![],
        max_grant_seconds: 300,
    }
    .snapshot()
    .unwrap()
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
fn egress_store_observations_are_idempotent_and_the_namespace_is_reserved() {
    let directory = private_directory();
    let store = store(directory.path(), "tenant-a");
    let worker = software_workers::running_from(&store, egress_admission(&store));
    let observed = EgressObserveRequestV1 {
        version: 1,
        observation_id: IdV1::random(),
        observation: EgressObservationV1::ChannelOpened {
            binding: binding(&worker),
        },
    };
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
