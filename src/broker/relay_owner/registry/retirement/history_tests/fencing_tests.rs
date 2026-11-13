//! Exact historical pending, lost real response and interrupted V2 writes.
//! All Core results below come from the real Store/gateway; no exec capability.
use super::*;
use crate::broker::relay_owner::journal::{
    ChannelChangeV2, ChannelHistoryV2, ChannelOperationPurposeV2, ChannelResolutionV2,
    ChannelTerminalV2,
};

fn literals() -> Vec<String> {
    vec![RUNNER.into(), VIEWER.into(), WORKLOAD.into()]
}

async fn history(f: &Fixture) -> ChannelHistoryV2 {
    f.retained.io.join_pending().await.unwrap();
    f.retained
        .io
        .channel_history(f.entry.source.job_id)
        .await
        .unwrap()
        .unwrap()
}

async fn append(f: &Fixture, change: ChannelChangeV2) -> ChannelHistoryV2 {
    f.retained
        .io
        .save_channel_change(f.entry.source.job_id, change, literals())
        .await
        .unwrap()
}

fn terminal(f: &Fixture) -> ChannelTerminalV2 {
    ChannelTerminalV2 {
        request: f.terminal_request.clone(),
        receipt: f.terminal_result.receipt.clone(),
        worker_revision: f.terminal_result.worker.revision,
    }
}

fn records(f: &Fixture) -> usize {
    f.business
        .persistent_child("relay_channels_v1", false)
        .unwrap()
        .unwrap()
        .persistent_entries(64)
        .unwrap()
        .iter()
        .filter(|entry| entry.name.starts_with("fact-"))
        .count()
}

async fn lookup(
    f: &Fixture,
    pending: Option<RelayChannelPendingV1>,
) -> RelayChannelReconciliationV1 {
    f.retained
        .source
        .client
        .reconcile_relay_channel(&RelayChannelReconcileV1 {
            version: 1,
            admission: f.retained.fact.admission.clone().unwrap(),
            pending,
        })
        .await
        .unwrap()
}

async fn begin_with_exact_claim_and_unresolved_ready(f: &Fixture) -> ChannelHistoryV2 {
    let original = history(f).await;
    assert_eq!(original.operations.len(), 2);
    assert!(original.terminal.is_none());
    let mut fact = original.fact;
    prepare_retirement_intent(&mut fact);
    append(
        f,
        ChannelChangeV2::BeginRetirement {
            fact,
            terminal: terminal(f),
        },
    )
    .await;

    let admission = lookup(f, None).await;
    append(
        f,
        ChannelChangeV2::History {
            pending_operation_id: None,
            response: admission,
        },
    )
    .await;
    let claim = f.retained.fact.claim.clone().unwrap();
    let recorded = lookup(f, Some(RelayChannelPendingV1::Claim(claim.clone()))).await;
    assert!(
        matches!(&recorded.outcome, RelayChannelHistoryOutcomeV1::Historical {
        channel: RelayChannelSnapshotV1 { state: RelayChannelStateV1::AcceptedBeforeExec, revision: 2, .. },
        pending: Some(RelayChannelPendingHistoryV1 {
            resolution: RelayChannelPendingResolutionV1::Recorded { receipt }, .. }), ..
    } if receipt.operation_id == claim.operation_id)
    );
    append(
        f,
        ChannelChangeV2::History {
            pending_operation_id: Some(claim.operation_id),
            response: recorded,
        },
    )
    .await;
    let unresolved = lookup(
        f,
        Some(RelayChannelPendingV1::Transition(f.old_close.clone())),
    )
    .await;
    assert!(
        matches!(
            &unresolved.outcome,
            RelayChannelHistoryOutcomeV1::Historical {
                pending: Some(RelayChannelPendingHistoryV1 {
                    resolution: RelayChannelPendingResolutionV1::Unresolved,
                    ..
                }),
                ..
            }
        ),
        "positive control: an absent Ready at its applicable CAS must remain pending"
    );
    let before = records(f);
    let result = append(
        f,
        ChannelChangeV2::History {
            pending_operation_id: Some(f.old_close.operation_id.clone()),
            response: unresolved,
        },
    )
    .await;
    assert_eq!(
        records(f),
        before,
        "Unresolved without snapshot progress spent a terminal record"
    );
    result
}

pub(super) async fn prepare_finalizable(f: &Fixture) -> ChannelHistoryV2 {
    let original = history(f).await;
    let mut fact = original.fact;
    prepare_retirement_intent(&mut fact);
    append(
        f,
        ChannelChangeV2::BeginRetirement {
            fact,
            terminal: terminal(f),
        },
    )
    .await;
    let admission = lookup(f, None).await;
    append(
        f,
        ChannelChangeV2::History {
            pending_operation_id: None,
            response: admission,
        },
    )
    .await;
    for operation in original.operations {
        let response = lookup(f, Some(operation.request.clone())).await;
        append(
            f,
            ChannelChangeV2::History {
                pending_operation_id: Some(operation.request.operation_id().into()),
                response,
            },
        )
        .await;
    }
    let current = history(f).await;
    if current.fact.core.as_ref().unwrap().state.active() {
        let exact = cleanup(f);
        append(
            f,
            ChannelChangeV2::RetirementIntent {
                request: exact.clone(),
            },
        )
        .await;
        let response = f.store.transition_relay_channel(&exact).unwrap();
        append(
            f,
            ChannelChangeV2::Mutation {
                operation_id: exact.operation_id,
                response,
            },
        )
        .await;
        let pending = history(f)
            .await
            .operations
            .into_iter()
            .filter(|operation| operation.resolution.is_none())
            .collect::<Vec<_>>();
        for operation in pending {
            let response = lookup(f, Some(operation.request.clone())).await;
            append(
                f,
                ChannelChangeV2::History {
                    pending_operation_id: Some(operation.request.operation_id().into()),
                    response,
                },
            )
            .await;
        }
    }
    let result = history(f).await;
    assert!(
        result
            .operations
            .iter()
            .all(|operation| operation.resolution.is_some())
    );
    assert!(!result.fact.core.as_ref().unwrap().state.active());
    assert_ne!(result.fact.phase, AttachmentPhase::Closed);
    result
}

fn cleanup(f: &Fixture) -> RelayChannelTransitionV1 {
    let admission = f.retained.fact.admission.as_ref().unwrap();
    RelayChannelTransitionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        channel_id: admission.channel_id.clone(),
        epoch: admission.epoch,
        expected_revision: 2,
        admission_input_hash: admission.input_hash().unwrap(),
        transition: RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
            worker_revision: f.terminal_result.worker.revision,
            terminal_observation_id: f.terminal_request.operation_id.clone(),
        },
    }
}

fn assert_one_exact_cleanup(
    h: &ChannelHistoryV2,
    expected: Option<&RelayChannelTransitionV1>,
) -> RelayChannelTransitionV1 {
    let cleanup: Vec<_> = h
        .operations
        .iter()
        .filter(|operation| matches!(&operation.purpose, ChannelOperationPurposeV2::Retirement))
        .collect();
    assert_eq!(
        cleanup.len(),
        1,
        "cleanup retry created a second durable identity"
    );
    let RelayChannelPendingV1::Transition(request) = &cleanup[0].request else {
        panic!("retirement operation became a claim");
    };
    if let Some(expected) = expected {
        assert_eq!(request, expected);
    }
    request.clone()
}

fn assert_exact_final_prefix(
    f: &Fixture,
    h: &ChannelHistoryV2,
    expected: &RelayChannelTransitionV1,
) {
    assert!(!h.legacy_closed);
    assert_eq!(h.fact.phase, AttachmentPhase::Closed);
    assert!(h.fact.local_joined);
    assert_eq!(h.fact.error, None);
    assert_eq!(f.retained.fact.phase, AttachmentPhase::Closed);
    assert_eq!(f.status().phase, Some(RelayAttachmentPhaseV1::Closed));
    assert!(!f.status().reconciliation_required);
    assert!(
        h.operations
            .iter()
            .all(|operation| operation.resolution.is_some())
    );
    assert_one_exact_cleanup(h, Some(expected));
    let old = h
        .operations
        .iter()
        .find(|operation| operation.request.operation_id() == f.old_close.operation_id)
        .unwrap();
    assert_eq!(
        old.request,
        RelayChannelPendingV1::Transition(f.old_close.clone())
    );
    assert!(
        matches!(&old.resolution, Some(ChannelResolutionV2::History { response })
        if matches!(&response.outcome, RelayChannelHistoryOutcomeV1::Historical {
            pending: Some(RelayChannelPendingHistoryV1 {
                resolution: RelayChannelPendingResolutionV1::InapplicableCas, .. }), .. }))
    );
    let claim = f.retained.fact.claim.as_ref().unwrap();
    let claimed = h
        .operations
        .iter()
        .find(|operation| operation.request.operation_id() == claim.operation_id)
        .unwrap();
    assert_eq!(claimed.request, RelayChannelPendingV1::Claim(claim.clone()));
    assert!(
        matches!(&claimed.resolution, Some(ChannelResolutionV2::History { response })
        if matches!(&response.outcome, RelayChannelHistoryOutcomeV1::Historical {
            pending: Some(RelayChannelPendingHistoryV1 {
                resolution: RelayChannelPendingResolutionV1::Recorded { receipt }, .. }), ..
        } if receipt.operation_id == claim.operation_id))
    );
    let journal = f
        .business
        .persistent_child("relay_channels_v1", false)
        .unwrap()
        .unwrap();
    assert_eq!(
        journal
            .persistent_read("fact-00000000000000000001.json", 16_384, &[])
            .unwrap()
            .unwrap()
            .bytes(),
        f.old_record.as_slice()
    );
}

fn assert_one_core_close_and_only_original_claim(f: &Fixture, cleanup: &RelayChannelTransitionV1) {
    let events = f.relay_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["data"]["input"]["action"] == "claim")
            .count(),
        1,
        "only the real setup claim may exist; recovery must never mint another"
    );
    let closes: Vec<_> = events
        .iter()
        .filter(|event| {
            event["data"]["input"]["request"]["transition"]["kind"] == "close_exact_worker_terminal"
        })
        .collect();
    assert_eq!(closes.len(), 1);
    assert_eq!(
        closes[0]["data"]["input"]["request"]["operation_id"],
        cleanup.operation_id
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| { event["data"]["input"]["request"]["transition"]["kind"] == "ready" })
            .count(),
        0,
        "uncertain old Ready was replayed as a mutation"
    );
    assert_eq!(f.calls.claims.load(Ordering::SeqCst), 0);
    assert_eq!(f.calls.reserves.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pending_ready_and_recorded_claim_survive_lost_cleanup_ack_without_second_effect() {
    let mut f = Fixture::accepted_with_old_ready().await;
    let ready = lookup(
        &f,
        Some(RelayChannelPendingV1::Transition(f.old_close.clone())),
    )
    .await;
    assert!(matches!(
        ready.outcome,
        RelayChannelHistoryOutcomeV1::Historical {
            channel: RelayChannelSnapshotV1 {
                state: RelayChannelStateV1::AcceptedBeforeExec,
                revision: 2,
                ..
            },
            pending: Some(RelayChannelPendingHistoryV1 {
                resolution: RelayChannelPendingResolutionV1::Unresolved,
                ..
            }),
            ..
        }
    ));
    f.calls
        .lose_next_transition_reply
        .store(true, Ordering::SeqCst);
    let worker = f.terminal_result.worker.clone();
    let first = f
        .registry
        .retire_confirmed_terminal(
            &f.entry,
            &mut f.retained,
            &worker,
            &f.terminal_request,
            &f.terminal_result,
        )
        .await;
    assert_eq!(first, Err(RelayOwnerError::Unknown));
    assert_eq!(f.calls.lost_replies.load(Ordering::SeqCst), 1);
    assert_ne!(f.retained.fact.phase, AttachmentPhase::Closed);
    assert_ne!(
        f.entry.view.lock().unwrap().as_ref().unwrap().phase,
        AttachmentPhase::Closed
    );
    let pending = history(&f).await;
    let exact = assert_one_exact_cleanup(&pending, None);
    assert_ne!(exact.operation_id, f.old_close.operation_id);
    assert_ne!(
        exact.operation_id,
        f.retained.fact.claim.as_ref().unwrap().operation_id
    );
    assert_one_core_close_and_only_original_claim(&f, &exact);
    assert_eq!(f.calls.transitions.load(Ordering::SeqCst), 1);

    f.calls.refuse_histories.store(false, Ordering::SeqCst);
    f.registry
        .retire_confirmed_terminal(
            &f.entry,
            &mut f.retained,
            &worker,
            &f.terminal_request,
            &f.terminal_result,
        )
        .await
        .unwrap();
    let complete = history(&f).await;
    assert_exact_final_prefix(&f, &complete, &exact);
    assert_one_core_close_and_only_original_claim(&f, &exact);
    assert_eq!(
        f.calls.transitions.load(Ordering::SeqCst),
        1,
        "the real committed cleanup must be found by lookup, never posted twice"
    );
    let before = records(&f);
    f.registry
        .retire_confirmed_terminal(
            &f.entry,
            &mut f.retained,
            &worker,
            &f.terminal_request,
            &f.terminal_result,
        )
        .await
        .unwrap();
    assert_eq!(records(&f), before);
    assert_eq!(f.calls.transitions.load(Ordering::SeqCst), 1);
    f.server.take().unwrap().close().await;
}

#[tokio::test]
async fn cancelled_cleanup_intent_is_joined_and_reused_before_any_core_effect() {
    let mut f = Fixture::accepted_with_old_ready().await;
    begin_with_exact_claim_and_unresolved_ready(&f).await;
    let exact = cleanup(&f);
    let before = records(&f);
    let (started, mut ready) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    f.retained.io.pause_next(started, finish);
    let mut save = Box::pin(f.retained.io.save_channel_change(
        f.entry.source.job_id,
        ChannelChangeV2::RetirementIntent {
            request: exact.clone(),
        },
        literals(),
    ));
    tokio::select! {
        result = &mut save => panic!("intent save completed before its barrier; success={}", result.is_ok()),
        result = &mut ready => result.unwrap(),
    }
    drop(save);
    assert_eq!(
        f.retained.fact.transition.as_ref(),
        Some(&f.old_close),
        "cancelled cleanup observer replaced the older Attachment intention"
    );
    let worker = f.terminal_result.worker.clone();
    let mut resumed = Box::pin(f.registry.retire_confirmed_terminal(
        &f.entry,
        &mut f.retained,
        &worker,
        &f.terminal_request,
        &f.terminal_result,
    ));
    assert!(futures_util::poll!(&mut resumed).is_pending());
    assert_eq!(
        f.calls.transitions.load(Ordering::SeqCst),
        0,
        "consumer dispatched before the retained real IO write completed"
    );
    release.send(()).unwrap();
    resumed.await.unwrap();
    let complete = history(&f).await;
    assert_exact_final_prefix(&f, &complete, &exact);
    assert!(records(&f) > before);
    assert_one_core_close_and_only_original_claim(&f, &exact);
    assert_eq!(f.calls.transitions.load(Ordering::SeqCst), 1);
    f.server.take().unwrap().close().await;
}

pub(super) async fn assert_cancelled_real_history_proof_and_finalize() {
    let mut f = Fixture::accepted_with_old_ready().await;
    begin_with_exact_claim_and_unresolved_ready(&f).await;
    let exact = cleanup(&f);
    append(
        &f,
        ChannelChangeV2::RetirementIntent {
            request: exact.clone(),
        },
    )
    .await;
    let real_result = f.store.transition_relay_channel(&exact).unwrap();
    assert_eq!(real_result.channel.state, RelayChannelStateV1::Closed);
    let proof = lookup(&f, Some(RelayChannelPendingV1::Transition(exact.clone()))).await;
    assert!(
        matches!(&proof.outcome, RelayChannelHistoryOutcomeV1::Historical {
        pending: Some(RelayChannelPendingHistoryV1 {
            resolution: RelayChannelPendingResolutionV1::Recorded { receipt }, .. }), ..
    } if receipt == &real_result.receipt)
    );

    let before = records(&f);
    let (started, mut ready) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    f.retained.io.pause_next(started, finish);
    let mut save = Box::pin(f.retained.io.save_channel_change(
        f.entry.source.job_id,
        ChannelChangeV2::History {
            pending_operation_id: Some(exact.operation_id.clone()),
            response: proof,
        },
        literals(),
    ));
    tokio::select! {
        result = &mut save => panic!("history save completed before its barrier; success={}", result.is_ok()),
        result = &mut ready => result.unwrap(),
    }
    drop(save);
    assert_ne!(
        f.entry.view.lock().unwrap().as_ref().unwrap().phase,
        AttachmentPhase::Closed
    );
    assert_eq!(f.status().phase, Some(RelayAttachmentPhaseV1::Unknown));
    assert!(f.status().reconciliation_required);
    release.send(()).unwrap();
    let proven = history(&f).await;
    assert_eq!(records(&f), before + 1);
    let cleanup_proof = proven
        .operations
        .iter()
        .find(|operation| operation.request.operation_id() == exact.operation_id)
        .unwrap();
    assert!(cleanup_proof.resolution.is_some());
    assert_ne!(proven.fact.phase, AttachmentPhase::Closed);

    let old = lookup(
        &f,
        Some(RelayChannelPendingV1::Transition(f.old_close.clone())),
    )
    .await;
    let proven = append(
        &f,
        ChannelChangeV2::History {
            pending_operation_id: Some(f.old_close.operation_id.clone()),
            response: old,
        },
    )
    .await;
    assert!(
        proven
            .operations
            .iter()
            .all(|operation| operation.resolution.is_some())
    );
    let mut closed = proven.fact;
    closed.phase = AttachmentPhase::Closed;
    closed.error = None;
    let before = records(&f);
    let (started, mut ready) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    f.retained.io.pause_next(started, finish);
    let mut save = Box::pin(f.retained.io.save_channel_change(
        f.entry.source.job_id,
        ChannelChangeV2::Finalize { fact: closed },
        literals(),
    ));
    tokio::select! {
        result = &mut save => panic!("Finalize completed before its barrier; success={}", result.is_ok()),
        result = &mut ready => result.unwrap(),
    }
    drop(save);
    assert_ne!(
        f.entry.view.lock().unwrap().as_ref().unwrap().phase,
        AttachmentPhase::Closed
    );
    assert_eq!(f.status().phase, Some(RelayAttachmentPhaseV1::Unknown));
    assert!(f.status().reconciliation_required);
    release.send(()).unwrap();
    f.retained.io.join_pending().await.unwrap();
    assert_ne!(
        f.entry.view.lock().unwrap().as_ref().unwrap().phase,
        AttachmentPhase::Closed,
        "an unobserved durable write was published without its retained observer"
    );
    let worker = f.terminal_result.worker.clone();
    f.registry
        .retire_confirmed_terminal(
            &f.entry,
            &mut f.retained,
            &worker,
            &f.terminal_request,
            &f.terminal_result,
        )
        .await
        .unwrap();
    assert_exact_final_prefix(&f, &history(&f).await, &exact);
    assert_eq!(
        records(&f),
        before + 1,
        "recovery duplicated an already durable Finalize"
    );
    assert_eq!(
        f.entry.view.lock().unwrap().as_ref().unwrap().phase,
        AttachmentPhase::Closed
    );
    assert_one_core_close_and_only_original_claim(&f, &exact);
    assert_eq!(
        f.calls.transitions.load(Ordering::SeqCst),
        0,
        "the setup Core mutation was already recorded; recovery must only look up proofs"
    );
    let before = records(&f);
    f.registry
        .retire_confirmed_terminal(
            &f.entry,
            &mut f.retained,
            &worker,
            &f.terminal_request,
            &f.terminal_result,
        )
        .await
        .unwrap();
    assert_eq!(
        records(&f),
        before,
        "repetition duplicated the durable terminal"
    );
    assert_exact_final_prefix(&f, &history(&f).await, &exact);
    f.server.take().unwrap().close().await;
}

#[tokio::test]
async fn cancelled_prefix_read_keeps_old_claim_unjoined_until_admissible_evidence() {
    let mut f = Fixture::accepted_with_unjoined_old_claim().await;
    assert!(f.retained.fact.claim.is_some());
    assert!(!f.retained.fact.local_joined);
    assert!(f.entry.handoff_started.load(Ordering::SeqCst));
    f.entry.stop.send_replace(false);
    let (started, mut ready) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    f.retained.io.pause_next(started, finish);
    let mut preparing = Box::pin(f.registry.prepare_entry(
        &f.entry,
        f.retained.source.clone(),
        false,
    ));
    tokio::select! {
        result = &mut preparing => panic!("prefix read completed before its barrier; success={}", result.is_ok()),
        result = &mut ready => result.unwrap(),
    }
    drop(preparing);
    assert!(f.entry.handoff_started.load(Ordering::SeqCst));
    f.entry.stop.send_replace(true);
    release.send(()).unwrap();
    let mut recovered = f
        .registry
        .reload_retained_history(&f.entry)
        .await
        .unwrap()
        .unwrap();
    assert!(recovered.fact.claim.is_some());
    assert!(
        !recovered.fact.local_joined,
        "absence of a local task after an interrupted read invented an old exec join"
    );
    let worker = f.terminal_result.worker.clone();
    assert_eq!(
        f.registry
            .retire_confirmed_terminal(
                &f.entry,
                &mut recovered,
                &worker,
                &f.terminal_request,
                &f.terminal_result
            )
            .await,
        Err(RelayOwnerError::Unknown)
    );
    assert_ne!(recovered.fact.phase, AttachmentPhase::Closed);
    assert_eq!(f.calls.histories.load(Ordering::SeqCst), 0);
    assert_eq!(f.calls.transitions.load(Ordering::SeqCst), 0);
    assert_eq!(f.calls.claims.load(Ordering::SeqCst), 0);
    f.close().await;

    // Positive control: a fully scoped legacy prefix with no claim intention
    // proves this owner never issued an exec. No fresh permit is constructed.
    let mut no_claim = Fixture::reserved_without_claim_or_join().await;
    let recovered = no_claim
        .registry
        .prepare_entry(&no_claim.entry, no_claim.retained.source.clone(), true)
        .await
        .unwrap()
        .unwrap();
    assert!(recovered.fact.claim.is_none());
    assert!(recovered.fact.local_joined);
    assert!(!no_claim.entry.handoff_started.load(Ordering::SeqCst));
    assert!(recovered.transport.is_none());
    assert!(recovered.owner.is_none());
    assert_eq!(no_claim.calls.histories.load(Ordering::SeqCst), 0);
    assert_eq!(no_claim.calls.transitions.load(Ordering::SeqCst), 0);
    assert_eq!(no_claim.calls.claims.load(Ordering::SeqCst), 0);
    no_claim.close().await;
}
