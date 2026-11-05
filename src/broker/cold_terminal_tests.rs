use super::*;
use crate::{
    audit::{EventQuery, Store},
    broker::{
        intent::ExecutionIntentV1,
        journal::{
            ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1, JournalScopeV1,
        },
        outbox::OutboxEventV1,
        terminal_facts::{BatchResultFactsV1, CoverageFactsV1},
    },
    control::{ArmedStartClaimV1, WorkerStateV1, WorkerV1},
    control_fs::Directory,
    redaction::Redactor,
    runner::broker::terminal_pending_seed_for_cold_audit_test,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn facts<'a>(
    scope: &'a JournalScopeV1,
    intent: &'a ExecutionIntentV1,
    checkpoint: &'a BrokerCheckpointV4,
) -> RetainedTerminalFacts<'a> {
    RetainedTerminalFacts {
        scope,
        intent,
        checkpoint,
    }
}

fn assert_conflict(
    label: &str,
    scope: &JournalScopeV1,
    intent: &ExecutionIntentV1,
    checkpoint: &BrokerCheckpointV4,
    worker: &WorkerV1,
) {
    assert_eq!(
        check_retained_terminal(facts(scope, intent, checkpoint), worker),
        Err(JournalError::Conflict),
        "retained fact mutation {label} must be rejected as a conflict"
    );
}

fn changed_event_checkpoint(
    checkpoint: &BrokerCheckpointV4,
    operation_id: Uuid,
    intent_hash: &str,
    event_id: Uuid,
    mutate: impl FnOnce(&mut crate::model::Event),
) -> BrokerCheckpointV4 {
    let mut changed = false;
    let mut mutate = Some(mutate);
    let mut events = Vec::with_capacity(checkpoint.events().len());
    for entry in checkpoint.events() {
        if entry.event_id() == event_id {
            changed = true;
            let mut event = entry.event().clone();
            mutate.take().expect("selected event ID is unique")(&mut event);
            let replacement =
                OutboxEventV1::new(operation_id, intent_hash, event).expect("valid changed Event");
            events.push(match entry.audit_seq() {
                Some(seq) => replacement
                    .acknowledged(seq)
                    .expect("preserve the existing exact ACK"),
                None => replacement,
            });
        } else {
            events.push(entry.clone());
        }
    }
    assert!(changed, "the selected real pending event must exist");
    BrokerCheckpointV4::new(checkpoint.lifecycle().clone(), events)
        .expect("changed event and input hash form a valid V4 checkpoint")
}

fn mutate_operation_json(
    checkpoint: &BrokerCheckpointV4,
    operation_kind: &str,
    mutate: impl FnOnce(&mut Value),
) -> BrokerCheckpointV4 {
    let mut value = serde_json::to_value(checkpoint).unwrap();
    let operations = value["lifecycle"]["broker"]["supervision"]["operations"]
        .as_array_mut()
        .expect("V4 checkpoint includes retained supervision operations");
    let operation = operations
        .iter_mut()
        .find(|operation| {
            operation["kind"].as_str() == Some(operation_kind)
                && (operation_kind != "arm"
                    || operation["request"]["subject"]["kind"].as_str() == Some("worker"))
        })
        .unwrap_or_else(|| panic!("seed includes a {operation_kind} operation"));
    mutate(operation);
    BrokerCheckpointV4::from_bytes(&serde_json::to_vec(&value).unwrap())
        .expect("operation mutation retains a valid V4 checkpoint shape")
}

fn mutate_checkpoint_json(
    checkpoint: &BrokerCheckpointV4,
    mutate: impl FnOnce(&mut Value),
) -> BrokerCheckpointV4 {
    let mut value = serde_json::to_value(checkpoint).unwrap();
    mutate(&mut value);
    BrokerCheckpointV4::from_bytes(&serde_json::to_vec(&value).unwrap())
        .expect("checkpoint mutation remains a valid V4 shape")
}

fn arm_claim_operations(checkpoint: &BrokerCheckpointV4) -> Value {
    let value = serde_json::to_value(checkpoint).unwrap();
    let operations = value["lifecycle"]["broker"]["supervision"]["operations"]
        .as_array()
        .expect("V4 checkpoint includes retained supervision operations");
    Value::Array(
        operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation["kind"].as_str(),
                    Some("arm" | "worker_claim" | "helper_claim")
                )
            })
            .cloned()
            .collect(),
    )
}

fn scope_variant(
    scope: &JournalScopeV1,
    tenant_id: &str,
    gateway_origin: &str,
    control_namespace_uuid: Uuid,
) -> JournalScopeV1 {
    JournalScopeV1::new(
        scope.installation_id(),
        tenant_id,
        gateway_origin,
        control_namespace_uuid,
        scope.profile_id(),
    )
    .expect("scope variant remains valid and canonical")
}

fn store_snapshot(store: &Store, worker_id: &str) -> Value {
    json!({
        "head": store.head().unwrap(),
        "worker": store.worker(worker_id).unwrap(),
        "usage": store.worker_usage().unwrap(),
        "result_events": store.events(&EventQuery {
            kind: Some("broker.batch.result".into()),
            ..Default::default()
        }).unwrap(),
        "coverage_events": store.events(&EventQuery {
            kind: Some("observer.coverage".into()),
            ..Default::default()
        }).unwrap(),
    })
}

#[tokio::test(flavor = "current_thread")]
async fn retained_terminal_checker_matches_real_facts_and_rejects_divergent_clones() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    assert!(seed.is_terminal());
    assert!(!seed.quota_held());
    assert_eq!(seed.allocation_count(), 1);
    assert!(seed.result_pending());
    assert!(!seed.result_acknowledged());
    assert_eq!(seed.coverage_audit_seq(), None);
    assert!(seed.retained_gateway_is_live());

    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let worker = store
        .worker(seed.worker_id())
        .unwrap()
        .expect("the central Store retains the exact terminal worker");
    assert_eq!(worker.worker_id, seed.worker_id());
    assert_eq!(worker.attempt.attempt_id, seed.attempt_id());
    assert_eq!(
        worker.attempt.container_id.as_deref(),
        Some(seed.container_id())
    );
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert!(!worker.allocation_inconsistent);

    let (scope, intent, checkpoint, journal_head_before) = {
        // Full Journal access is permitted here only to extract test facts;
        // production cold reconciliation remains stop-reader-only.
        let root = Directory::open_existing(seed.journal_root()).unwrap();
        let journal = Journal::open(
            &root,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap();
        let job = journal
            .load(seed.operation_id())
            .unwrap()
            .expect("the accepted journal job remains durable");
        let checkpoint = job
            .broker_checkpoint_v4()
            .expect("the accepted job retains its V4 outbox")
            .clone();
        (
            job.scope().clone(),
            job.intent().clone(),
            checkpoint,
            job.head().clone(),
        )
    };
    assert_eq!(scope.installation_id(), seed.scope().installation_id());
    assert_eq!(scope.tenant_id(), seed.scope().tenant_id());
    assert_eq!(scope.gateway_origin(), seed.scope().gateway_origin());
    assert_eq!(
        scope.control_namespace_uuid(),
        seed.scope().control_namespace_uuid()
    );
    assert_eq!(scope.profile_id(), seed.scope().profile_id());
    assert_eq!(intent.operation_id(), seed.operation_id());

    let pending = checkpoint
        .events()
        .iter()
        .filter(|entry| entry.audit_seq().is_none())
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 2, "the real V4 pending pair is unchanged");
    assert_eq!(pending[0].event().kind, "observer.coverage");
    assert_eq!(pending[0].event_id(), seed.coverage_event_id());
    assert_eq!(pending[0].input_hash(), seed.coverage_event_input_hash());
    assert_eq!(pending[1].event().kind, "broker.batch.result");
    assert_eq!(pending[1].event_id(), seed.event_id());
    assert_eq!(pending[1].input_hash(), seed.event_input_hash());
    let result_facts: BatchResultFactsV1 =
        serde_json::from_value(pending[1].event().data.clone()).unwrap();
    assert_eq!(result_facts.version, 1);
    assert_eq!(result_facts.worker_id, worker.worker_id);
    assert_eq!(result_facts.attempt_id, worker.attempt.attempt_id);
    assert_eq!(
        result_facts.observation.id.as_deref(),
        Some(seed.container_id())
    );
    let coverage_facts: CoverageFactsV1 =
        serde_json::from_value(pending[0].event().data.clone()).unwrap();
    assert_eq!(coverage_facts.version, 1);

    let snapshot_before = store_snapshot(&store, seed.worker_id());
    let calls_before = seed.runtime_calls();
    let decision_count_before = seed.current_decision_request_count();
    let arm_claim_before = arm_claim_operations(&checkpoint);
    assert_eq!(
        decision_count_before,
        seed.decision_request_count_snapshot()
    );

    assert_eq!(
        check_retained_terminal(facts(&scope, &intent, &checkpoint), &worker),
        Ok(()),
        "real retained terminal facts and the central Worker agree"
    );

    let mut worker_variants = Vec::new();
    let mut changed = worker.clone();
    changed.state = WorkerStateV1::Running;
    worker_variants.push(("nonterminal state", changed));
    let mut changed = worker.clone();
    changed.quota_held = true;
    worker_variants.push(("held quota", changed));
    let mut changed = worker.clone();
    changed.allocation_inconsistent = true;
    worker_variants.push(("inconsistent allocation", changed));
    let mut changed = worker.clone();
    changed.worker_id = Uuid::new_v4().to_string();
    worker_variants.push(("worker identity", changed));
    let mut changed = worker.clone();
    changed.attempt.attempt_id = Uuid::new_v4().to_string();
    worker_variants.push(("attempt identity", changed));
    let mut changed = worker.clone();
    changed.spec_hash = "c".repeat(64);
    worker_variants.push(("worker spec hash", changed));
    let mut changed = worker.clone();
    changed.launch.authority_id = Uuid::new_v4().to_string();
    worker_variants.push(("launch authority", changed));
    let mut changed = worker.clone();
    let replacement_authority = Uuid::new_v4().to_string();
    changed.launch.authority_id = replacement_authority.clone();
    changed
        .last_observation
        .as_mut()
        .expect("the terminal seed retains an observation")
        .inspection
        .authority_id = replacement_authority;
    worker_variants.push(("coupled launch and observed authority", changed));
    let mut changed = worker.clone();
    changed.attempt.container_id = Some("e".repeat(64));
    worker_variants.push(("container identity", changed));
    let mut changed = worker.clone();
    changed.observed_started_at = Some("2026-10-30T12:00:01.000000000Z".into());
    worker_variants.push(("observed start time", changed));
    let mut changed = worker.clone();
    changed.attempt.start_operation_id = Some(Uuid::new_v4().to_string());
    worker_variants.push(("start operation", changed));
    let mut changed = worker.clone();
    changed.attempt.start_emitter_id = Some(Uuid::new_v4().to_string());
    worker_variants.push(("start emitter", changed));
    for (label, changed) in worker_variants {
        assert_conflict(label, &scope, &intent, &checkpoint, &changed);
    }

    let scope_variants = [
        scope_variant(
            &scope,
            "other-tenant",
            scope.gateway_origin(),
            scope.control_namespace_uuid(),
        ),
        scope_variant(
            &scope,
            scope.tenant_id(),
            "http://127.0.0.1:1",
            scope.control_namespace_uuid(),
        ),
        scope_variant(
            &scope,
            scope.tenant_id(),
            scope.gateway_origin(),
            Uuid::new_v4(),
        ),
    ];
    for (label, changed) in [
        ("tenant scope", &scope_variants[0]),
        ("gateway origin", &scope_variants[1]),
        ("control namespace", &scope_variants[2]),
    ] {
        assert_conflict(label, changed, &intent, &checkpoint, &worker);
    }

    let mut changed_intent_value = serde_json::to_value(&intent).unwrap();
    changed_intent_value["operation_id"] = json!(Uuid::new_v4().to_string());
    let changed_intent =
        ExecutionIntentV1::from_bytes(&serde_json::to_vec(&changed_intent_value).unwrap())
            .expect("operation-only intent change remains a valid intent");
    assert_conflict(
        "intent operation identity",
        &scope,
        &changed_intent,
        &checkpoint,
        &worker,
    );

    let intent_hash = intent.intent_hash().unwrap();
    let mut changed = changed_event_checkpoint(
        &checkpoint,
        seed.operation_id(),
        &intent_hash,
        seed.event_id(),
        |event| {
            event.data["observation"]["id"] = json!("f".repeat(64));
        },
    );
    assert_conflict("retained observation", &scope, &intent, &changed, &worker);
    changed = changed_event_checkpoint(
        &checkpoint,
        seed.operation_id(),
        &intent_hash,
        seed.event_id(),
        |event| event.data["worker_id"] = json!(Uuid::new_v4().to_string()),
    );
    assert_conflict("result data", &scope, &intent, &changed, &worker);
    changed = changed_event_checkpoint(
        &checkpoint,
        seed.operation_id(),
        &intent_hash,
        seed.event_id(),
        |event| {
            event.provenance = if event.provenance == "observed" {
                "declared".into()
            } else {
                "observed".into()
            };
        },
    );
    assert_conflict("event provenance", &scope, &intent, &changed, &worker);
    changed = changed_event_checkpoint(
        &checkpoint,
        seed.operation_id(),
        &intent_hash,
        seed.event_id(),
        |event| {
            assert_eq!(event.source, "runner");
            event.source = "worker".into();
        },
    );
    assert_conflict("event source", &scope, &intent, &changed, &worker);
    changed = changed_event_checkpoint(
        &checkpoint,
        seed.operation_id(),
        &intent_hash,
        seed.event_id(),
        |event| event.agent_id = Some(Uuid::new_v4().to_string()),
    );
    assert_conflict("event agent", &scope, &intent, &changed, &worker);
    changed = changed_event_checkpoint(
        &checkpoint,
        seed.operation_id(),
        &intent_hash,
        seed.event_id(),
        |event| event.session_id = Some(Uuid::new_v4().to_string()),
    );
    assert_conflict("event scope", &scope, &intent, &changed, &worker);

    let missing_arm_ack = mutate_operation_json(&checkpoint, "arm", |operation| {
        operation["response"] = Value::Null;
    });
    assert_conflict(
        "missing acknowledged arm",
        &scope,
        &intent,
        &missing_arm_ack,
        &worker,
    );
    let missing_claim_ack = mutate_operation_json(&checkpoint, "worker_claim", |operation| {
        operation["response"] = Value::Null;
    });
    assert_conflict(
        "missing acknowledged worker claim",
        &scope,
        &intent,
        &missing_claim_ack,
        &worker,
    );
    let divergent_claim = mutate_operation_json(&checkpoint, "worker_claim", |operation| {
        operation["request"]["emitter_id"] = json!(Uuid::new_v4().to_string());
    });

    let running_claim = mutate_operation_json(&checkpoint, "worker_claim", |operation| {
        operation["response"]["worker"]["state"] = json!("running");
        operation["response"]["worker"]["quota_held"] = json!(true);
    });

    let checkpoint_wire = serde_json::to_value(&checkpoint).unwrap();
    let operations = checkpoint_wire["lifecycle"]["broker"]["supervision"]["operations"]
        .as_array()
        .expect("V4 checkpoint retains operations");
    let arm_operation = operations
        .iter()
        .find(|operation| {
            operation["kind"].as_str() == Some("arm")
                && operation["request"]["subject"]["kind"].as_str() == Some("worker")
        })
        .expect("seed retains the worker arm");
    let parent_revision = arm_operation["parent"]["revision"]
        .as_u64()
        .expect("arm parent revision is an unsigned integer");
    let claim_operation = operations
        .iter()
        .find(|operation| operation["kind"].as_str() == Some("worker_claim"))
        .expect("seed retains the worker claim");
    let claim_revision = claim_operation["response"]["worker"]["revision"]
        .as_u64()
        .expect("worker claim revision is an unsigned integer");
    assert!(
        parent_revision < claim_revision && claim_revision < worker.revision,
        "real terminal worker revision must leave a raised value in the accepted interval"
    );
    // The central revision is itself the inclusive upper endpoint required by
    // the checker's interval; it is above both the arm parent and old claim.
    let coupled_revision = worker.revision;
    let raised_claim_revision = mutate_operation_json(&checkpoint, "worker_claim", |operation| {
        operation["response"]["worker"]["revision"] = json!(coupled_revision);
        operation["response"]["receipt"]["revision"] = json!(coupled_revision);
    });

    let other_namespace = Uuid::new_v4();
    let coupled_namespace = mutate_checkpoint_json(&checkpoint, |value| {
        value["lifecycle"]["broker"]["supervision"]["control_namespace_uuid"] =
            json!(other_namespace);
        let operations = value["lifecycle"]["broker"]["supervision"]["operations"]
            .as_array_mut()
            .expect("checkpoint retains operations");
        let arm_operation = operations
            .iter_mut()
            .find(|operation| {
                operation["kind"].as_str() == Some("arm")
                    && operation["request"]["subject"]["kind"].as_str() == Some("worker")
            })
            .expect("seed retains the worker arm");
        arm_operation["response"]["receipt"]["control_namespace_uuid"] =
            json!(other_namespace.to_string());
        let claim_operation = operations
            .iter_mut()
            .find(|operation| operation["kind"].as_str() == Some("worker_claim"))
            .expect("seed retains the worker claim");
        claim_operation["request"]["arm_receipt"]["control_namespace_uuid"] =
            json!(other_namespace.to_string());
        let claim: ArmedStartClaimV1 = serde_json::from_value(claim_operation["request"].clone())
            .expect("namespace-adjusted worker claim remains typed and valid");
        let input_hash = claim
            .input_hash()
            .expect("namespace-adjusted worker claim has a canonical hash");
        claim_operation["response"]["receipt"]["input_hash"] = json!(input_hash);
    });
    let namespace_b = scope_variant(
        &scope,
        scope.tenant_id(),
        scope.gateway_origin(),
        other_namespace,
    );

    // Evaluate every clone before asserting, so one reproduced gap does not
    // hide whether the other coupled mutations reach the checker.
    let divergent_claim_results = [
        (
            "divergent historical claim emitter",
            check_retained_terminal(facts(&scope, &intent, &divergent_claim), &worker),
        ),
        (
            "worker claim Running with held quota",
            check_retained_terminal(facts(&scope, &intent, &running_claim), &worker),
        ),
        (
            "coupled worker and receipt revision at central revision",
            check_retained_terminal(facts(&scope, &intent, &raised_claim_revision), &worker),
        ),
        (
            "namespace B coupled through supervision, Arm receipt, and claim hash",
            check_retained_terminal(facts(&namespace_b, &intent, &coupled_namespace), &worker),
        ),
    ];
    assert!(
        divergent_claim_results
            .iter()
            .all(|(_, result)| matches!(result, Err(JournalError::Conflict))),
        "retained checks (fixed labels and outcomes): {divergent_claim_results:?}"
    );

    assert_eq!(store_snapshot(&store, seed.worker_id()), snapshot_before);
    assert_eq!(seed.runtime_calls(), calls_before);
    assert_eq!(seed.current_decision_request_count(), decision_count_before);

    // This final Full Journal read only verifies that the pure checker wrote
    // nothing. It is not evidence for a production cold-reader/NoBlob path.
    let root = Directory::open_existing(seed.journal_root()).unwrap();
    let journal = Journal::open(
        &root,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let job_after = journal
        .load(seed.operation_id())
        .unwrap()
        .expect("pure checker leaves the real accepted job durable");
    let checkpoint_after = job_after
        .broker_checkpoint_v4()
        .expect("accepted job still retains its V4 outbox");
    assert_eq!(job_after.head(), &journal_head_before);
    assert_eq!(arm_claim_operations(checkpoint_after), arm_claim_before);
}
