//! Synthetic status/ACK facts and real Store/loopback/journal guards.
//! No engine, CLI or child process; the terminal journal parent uses HTTP/SDK.
use super::*;
use crate::broker::relay_owner::{journal::ChannelJournal, tests::arm};

fn fact(state: RelayChannelStateV1) -> ChannelFact {
    let arm = arm();
    let endpoint = RelayEndpointV1 {
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        generation: 1,
    };
    let admission = RelayChannelAdmissionV1 {
        version: 1,
        operation_id: Uuid::from_u128(20).to_string(),
        channel_id: Uuid::from_u128(21).to_string(),
        epoch: 1,
        control_namespace_uuid: Uuid::from_u128(22).to_string(),
        endpoint: endpoint.clone(),
        credential_id: Uuid::from_u128(23).to_string(),
        binding_revision: 1,
        credential_sha256_digest: "1".repeat(64),
        session_id: Uuid::from_u128(24).to_string(),
        expected_worker_revision: 1,
        inspection: arm.inspection.clone(),
        started_at: "2026-11-08T00:00:00Z".into(),
        runtime_source: RelayRuntimeSourceV1::BrokerReportedRuntime,
        authority_binding_hash: arm.authority_binding_hash.clone(),
        configuration_fingerprint_version: arm.configuration_fingerprint_version,
        configuration_fingerprint: arm.configuration_fingerprint.clone(),
        expires_at: "2026-11-08T00:01:00Z".into(),
    };
    let core = RelayChannelSnapshotV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        control_namespace_uuid: admission.control_namespace_uuid.clone(),
        channel_id: admission.channel_id.clone(),
        epoch: 1,
        admission_operation_id: admission.operation_id.clone(),
        admission_input_hash: admission.input_hash().unwrap(),
        endpoint: endpoint.clone(),
        credential_id: admission.credential_id.clone(),
        binding_revision: 1,
        session_id: admission.session_id.clone(),
        expires_at: admission.expires_at.clone(),
        revision: match state {
            RelayChannelStateV1::Reserved => 1,
            RelayChannelStateV1::CancelledBeforeClaim
            | RelayChannelStateV1::KnownRefusedBeforeSpawn
            | RelayChannelStateV1::AcceptedBeforeExec => 2,
            RelayChannelStateV1::Ready
            | RelayChannelStateV1::Closing
            | RelayChannelStateV1::Unknown
            | RelayChannelStateV1::Closed => 3,
        },
        state,
    };
    ChannelFact {
        version: 1,
        job_id: Uuid::from_u128(25),
        source_profile_id: "fixture".into(),
        endpoint,
        source_head_revision: 1,
        source_head_hash: "2".repeat(64),
        arm_id: arm.operation_id,
        original_clock: arm.clock,
        original_deadline_ns_decimal: arm.deadline_ns_decimal,
        phase: AttachmentPhase::AwaitingIssuedWorkload,
        observed_at: "2026-11-08T00:00:00Z".into(),
        error: None,
        admission: Some(admission),
        claim: None,
        core: Some(core),
        transition: None,
        local_joined: false,
    }
}

fn request(
    fact: &ChannelFact,
    transition: RelayChannelTransitionKindV1,
) -> RelayChannelTransitionV1 {
    let admission = fact.admission.as_ref().unwrap();
    RelayChannelTransitionV1 {
        version: 1,
        operation_id: Uuid::from_u128(26).to_string(),
        channel_id: admission.channel_id.clone(),
        epoch: admission.epoch,
        expected_revision: fact.core.as_ref().unwrap().revision,
        admission_input_hash: admission.input_hash().unwrap(),
        transition,
    }
}

fn close() -> RelayChannelTransitionKindV1 {
    RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
        worker_revision: 10,
        terminal_observation_id: Uuid::from_u128(27).to_string(),
    }
}

#[test]
fn pending_retirement_is_unknown_even_when_an_old_fact_has_no_error() {
    for transition in [
        RelayChannelTransitionKindV1::CancelReserved,
        RelayChannelTransitionKindV1::KnownRefusedBeforeSpawn,
        close(),
    ] {
        let mut pending = fact(RelayChannelStateV1::Reserved);
        pending.transition = Some(request(&pending, transition));
        for phase in [
            AttachmentPhase::AwaitingIssuedWorkload,
            AttachmentPhase::Admitted,
        ] {
            pending.phase = phase;
            assert_eq!(
                attachment_status_phase(&pending, None),
                RelayAttachmentPhaseV1::Unknown,
                "a durable retirement intent was published as normal waiting/admission"
            );
            assert!(reconciliation_required(Some(&pending), None));
        }
    }
}

#[test]
fn retirement_intent_preserves_the_error_and_requires_reconciliation_before_ack() {
    let mut pending = fact(RelayChannelStateV1::Reserved);
    pending.error = Some(RelayOwnerError::InvalidMaterial);
    prepare_retirement_intent(&mut pending);
    assert_eq!(pending.error, Some(RelayOwnerError::InvalidMaterial));
    assert_eq!(pending.phase, AttachmentPhase::Unknown);
    assert!(pending.local_joined);
    assert!(reconciliation_required(Some(&pending), pending.error));

    pending.error = None;
    prepare_retirement_intent(&mut pending);
    assert_eq!(pending.error, Some(RelayOwnerError::Unknown));
}

#[test]
fn terminal_ack_requires_the_compatible_state_and_exact_next_revision() {
    for (transition, before_state, state, next_revision) in [
        (
            RelayChannelTransitionKindV1::CancelReserved,
            RelayChannelStateV1::Reserved,
            RelayChannelStateV1::CancelledBeforeClaim,
            2,
        ),
        (
            RelayChannelTransitionKindV1::KnownRefusedBeforeSpawn,
            RelayChannelStateV1::Reserved,
            RelayChannelStateV1::KnownRefusedBeforeSpawn,
            2,
        ),
        (
            close(),
            RelayChannelStateV1::AcceptedBeforeExec,
            RelayChannelStateV1::Closed,
            3,
        ),
    ] {
        let mut pending = fact(before_state);
        pending.phase = AttachmentPhase::Unknown;
        pending.error = Some(RelayOwnerError::InvalidMaterial);
        pending.local_joined = true;
        let request = request(&pending, transition);
        let before = crate::broker::journal::canonical(&pending).unwrap();
        let mut core = pending.core.clone().unwrap();
        assert!(
            super::super::attachment::acknowledged_transition(&pending, &request, core.clone())
                .is_err()
        );
        core.state = state;
        assert!(
            super::super::attachment::acknowledged_transition(&pending, &request, core.clone())
                .is_err()
        );
        core.revision = next_revision;
        let acknowledged =
            super::super::attachment::acknowledged_transition(&pending, &request, core.clone())
                .unwrap();
        assert_eq!(acknowledged.phase, AttachmentPhase::Closed);
        assert_eq!(acknowledged.error, None);
        assert_eq!(
            crate::broker::journal::canonical(&pending).unwrap(),
            before,
            "an uncommitted acknowledgement changed the retained fact"
        );
        core.revision = next_revision + 1;
        assert!(
            super::super::attachment::acknowledged_transition(&pending, &request, core).is_err()
        );
    }
}

#[tokio::test]
async fn journal_accepts_only_joined_error_free_explicit_core_terminals() {
    use crate::broker::relay_owner::journal::ChannelChangeV2;
    // V2 Closed requires the full real prefix: Begin, exact Store/HTTP proofs
    // and every old pending resolved. The original state/error/join/scope
    // negatives below remain, now exercised through Finalize rather than Fact.
    for state in [
        RelayChannelStateV1::Closed,
        RelayChannelStateV1::CancelledBeforeClaim,
        RelayChannelStateV1::KnownRefusedBeforeSpawn,
    ] {
        let (mut f, witnessed) = super::history_tests::Fixture::finalizable(state).await;
        f.retained.io.join_pending().await.unwrap();
        let scope = f.retained.tenant.scope.clone();
        let literals = f.retained.tenant.literals.lock().unwrap().clone();
        // No IO producer runs after this drain; the same RoleLease/outbox flock
        // stays retained while the direct journal guard/reopen is exercised.
        let mut journal = ChannelJournal::open(&f.business, &scope, &literals).unwrap();
        let witness = journal.history(witnessed.fact.job_id).unwrap().unwrap();
        assert!(witness.terminal.is_some());
        assert!(
            witness
                .operations
                .iter()
                .all(|operation| operation.resolution.is_some())
        );
        assert_eq!(
            crate::broker::journal::canonical(&witness.fact).unwrap(),
            crate::broker::journal::canonical(&witnessed.fact).unwrap(),
            "valid private material and exact pre-finalization history must survive reopen"
        );
        let job = witness.fact.job_id;
        let mut terminal = witness.fact;
        terminal.phase = AttachmentPhase::Closed;
        terminal.local_joined = true;
        terminal.error = None;
        for active in [
            RelayChannelStateV1::Reserved,
            RelayChannelStateV1::AcceptedBeforeExec,
            RelayChannelStateV1::Ready,
            RelayChannelStateV1::Closing,
            RelayChannelStateV1::Unknown,
        ] {
            let mut invalid = terminal.clone();
            invalid.core.as_mut().unwrap().state = active;
            assert!(matches!(
                journal.append_v2(job, ChannelChangeV2::Finalize { fact: invalid }, &literals),
                Err(RelayOwnerError::Scope)
            ));
        }
        let mut invalid = terminal.clone();
        invalid.local_joined = false;
        assert!(matches!(
            journal.append_v2(job, ChannelChangeV2::Finalize { fact: invalid }, &literals),
            Err(RelayOwnerError::Scope)
        ));
        let mut invalid = terminal.clone();
        invalid.error = Some(RelayOwnerError::Unknown);
        assert!(matches!(
            journal.append_v2(job, ChannelChangeV2::Finalize { fact: invalid }, &literals),
            Err(RelayOwnerError::Scope)
        ));
        let mut invalid = terminal.clone();
        invalid.core.as_mut().unwrap().endpoint.worker_id = Uuid::from_u128(29).to_string();
        assert!(matches!(
            journal.append_v2(job, ChannelChangeV2::Finalize { fact: invalid }, &literals),
            Err(RelayOwnerError::Scope)
        ));
        if state == RelayChannelStateV1::Closed {
            journal.append_v2(job, ChannelChangeV2::Finalize { fact: terminal.clone() }, &literals)
                .expect("the already supported Closed terminal must pass before reserved terminal cases");
        } else {
            journal
                .append_v2(
                    job,
                    ChannelChangeV2::Finalize {
                        fact: terminal.clone(),
                    },
                    &literals,
                )
                .unwrap();
        }
        terminal.observed_at = "2026-11-08T00:00:01Z".into();
        let before = journal.history(job).unwrap().unwrap().terminal_records;
        assert_eq!(
            journal
                .append_v2(job, ChannelChangeV2::Finalize { fact: terminal }, &literals)
                .unwrap()
                .terminal_records,
            before
        );
        drop(journal);
        assert_eq!(
            ChannelJournal::open(&f.business, &scope, &literals)
                .unwrap()
                .history(job)
                .unwrap()
                .unwrap()
                .fact
                .phase,
            AttachmentPhase::Closed,
        );
        f.close().await;
    }
}
