//! Actual SQLite control facts and private journal files. Runtime observations
//! are synthetic; no AcceptedJob, engine, CLI child or external service exists.
use super::*;
use crate::{
    audit::Store,
    broker::discovery::RoleLease,
    control::*,
    model::{CommandSpec, DecisionRequest, Event},
    policy::{self, Policy},
    redaction::Redactor,
    relay::admission::*,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path};

#[path = "../../../../tests/support/relay_admission_fixture.rs"]
mod relay_admission_fixture;
use relay_admission_fixture::*;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const MATERIAL: &str = "journal-history-public-workload-fixture-0123456789";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: Directory,
    _role: RoleLease,
    scope: JournalScopeV1,
    literals: Vec<String>,
    store: Store,
    fact: ChannelFact,
    terminal: ChannelTerminalV2,
}
impl Fixture {
    fn new() -> Self {
        Self::with_reserved(false)
    }
    fn with_reserved(reserve: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let store = fresh(temporary.path(), "tenant-a", 10000);
        let worker = running_now(&store);
        let (credential, revision) = bind(&store, &worker, MATERIAL);
        let admission: RelayChannelAdmissionV1 =
            serde_json::from_value(request(&store, &worker, &credential, revision, MATERIAL))
                .unwrap();
        let core = reserve.then(|| store.reserve_relay_channel(&admission).unwrap().channel);
        let db = Connection::open_with_flags(
            temporary.path().join("tenant-a.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut statement = db
            .prepare("SELECT payload_json FROM execution_arms WHERE worker_id=?1 AND attempt_id=?2")
            .unwrap();
        let arms: Vec<ArmV1> = statement
            .query_map(
                rusqlite::params![worker.worker_id, worker.attempt.attempt_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        let arm = arms
            .into_iter()
            .find(|arm| matches!(arm.subject, ArmSubjectV1::Worker))
            .unwrap();
        drop(statement);
        drop(db);
        let mut observed = worker.last_observation.clone().unwrap();
        observed.observed_at = chrono::Utc::now().to_rfc3339();
        observed.running = Some(false);
        observed.status = RuntimeStatusV1::Exited;
        observed.finished_at = Some(chrono::Utc::now().to_rfc3339());
        observed.exit_code = Some(0);
        let request = mutation(
            &worker,
            WorkerTransitionV1::Observe {
                observation: observed,
            },
        );
        let result = store.mutate_worker(&request).unwrap();
        // A later terminal mutation advances Worker revision independently of
        // the historical Observe receipt, exactly as the production contract.
        let current = store
            .mutate_worker(&mutation(
                &result.worker,
                WorkerTransitionV1::CleanupResult {
                    removed: true,
                    error: None,
                },
            ))
            .unwrap();
        let terminal = ChannelTerminalV2 {
            request,
            receipt: result.receipt,
            worker_revision: current.worker.revision,
        };
        let root = Directory::open_existing(temporary.path()).unwrap();
        let role = RoleLease::acquire(&root).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::new_v4(),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::parse_str(&admission.control_namespace_uuid).unwrap(),
            "relay_shared",
        )
        .unwrap();
        let fact = ChannelFact {
            version: 1,
            job_id: Uuid::new_v4(),
            source_profile_id: "fixture".into(),
            endpoint: admission.endpoint.clone(),
            source_head_revision: 1,
            source_head_hash: HASH_A.into(),
            arm_id: arm.operation_id,
            original_clock: arm.clock,
            original_deadline_ns_decimal: arm.deadline_ns_decimal,
            phase: AttachmentPhase::AwaitingIssuedWorkload,
            observed_at: chrono::Utc::now().to_rfc3339(),
            error: None,
            admission: Some(admission),
            claim: None,
            core,
            transition: None,
            local_joined: false,
        };
        Self {
            _temporary: temporary,
            root,
            _role: role,
            scope,
            literals: vec![MATERIAL.into()],
            store,
            fact,
            terminal,
        }
    }
    fn open(&self) -> ChannelJournal {
        ChannelJournal::open(&self.root, &self.scope, &self.literals).unwrap()
    }
    fn begin(&self, journal: &mut ChannelJournal, fact: ChannelFact) -> ChannelHistoryV2 {
        let mut fact = fact;
        fact.phase = AttachmentPhase::Unknown;
        fact.error = fact.error.or(Some(RelayOwnerError::Unknown));
        fact.local_joined = true;
        journal
            .append_v2(
                fact.job_id,
                ChannelChangeV2::BeginRetirement {
                    fact,
                    terminal: self.terminal.clone(),
                },
                &self.literals,
            )
            .unwrap()
    }
    fn proof(&self, pending: Option<RelayChannelPendingV1>) -> RelayChannelReconciliationV1 {
        self.store
            .reconcile_relay_channel(&RelayChannelReconcileV1 {
                version: 1,
                admission: self.fact.admission.clone().unwrap(),
                pending,
            })
            .unwrap()
    }
    fn write_v1(&self, facts: &[ChannelFact]) -> Vec<Vec<u8>> {
        drop(self.open());
        let directory = self.root.persistent_child(CHILD, false).unwrap().unwrap();
        let mut previous = None;
        facts
            .iter()
            .enumerate()
            .map(|(index, fact)| {
                let bytes = super::super::super::journal::canonical(
                    &json!({"version":1,"sequence":index+1,"previous_sha256":previous,"fact":fact}),
                )
                .unwrap();
                let file =
                    write(&directory, &name(index as u64 + 1), &bytes, &self.literals).unwrap();
                previous = Some(file.digest().to_owned());
                bytes
            })
            .collect()
    }
    fn old_close(&self) -> RelayChannelTransitionV1 {
        let admission = self.fact.admission.as_ref().unwrap();
        RelayChannelTransitionV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            channel_id: admission.channel_id.clone(),
            epoch: admission.epoch,
            expected_revision: 1,
            admission_input_hash: admission.input_hash().unwrap(),
            transition: RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
                worker_revision: self.terminal.worker_revision,
                terminal_observation_id: self.terminal.request.operation_id.clone(),
            },
        }
    }
}

#[test]
fn local_only_finalize_requires_unique_terminal_begin_and_complete_empty_index() {
    let f = Fixture::new();
    let mut journal = f.open();
    let mut fact = f.fact.clone();
    fact.admission = None;
    journal.append(fact.clone(), &f.literals).unwrap();
    let mut closed = fact.clone();
    closed.local_joined = true;
    closed.phase = AttachmentPhase::Closed;
    assert!(journal.append(closed.clone(), &f.literals).is_err());
    assert!(
        journal
            .append_v2(
                fact.job_id,
                ChannelChangeV2::Finalize {
                    fact: closed.clone()
                },
                &f.literals
            )
            .is_err()
    );
    let begun = f.begin(&mut journal, fact.clone());
    assert!(
        begun.terminal.as_ref().unwrap().worker_revision
            > begun.terminal.as_ref().unwrap().receipt.revision.unwrap()
    );
    let count = journal.records.len();
    f.begin(&mut journal, begun.fact.clone());
    assert_eq!(journal.records.len(), count);
    let mut wrong = f.terminal.clone();
    wrong.worker_revision += 1;
    assert!(
        journal
            .append_v2(
                fact.job_id,
                ChannelChangeV2::BeginRetirement {
                    fact: begun.fact.clone(),
                    terminal: wrong
                },
                &f.literals
            )
            .is_err()
    );
    let mut late = begun.fact.clone();
    late.error = Some(RelayOwnerError::Capacity);
    assert!(journal.append(late, &f.literals).is_err());
    closed = begun.fact;
    closed.phase = AttachmentPhase::Closed;
    closed.error = None;
    journal
        .append_v2(
            fact.job_id,
            ChannelChangeV2::Finalize {
                fact: closed.clone(),
            },
            &f.literals,
        )
        .unwrap();
    let count = journal.records.len();
    journal
        .append_v2(
            fact.job_id,
            ChannelChangeV2::Finalize { fact: closed },
            &f.literals,
        )
        .unwrap();
    assert_eq!(journal.records.len(), count);
    drop(journal);
    let history = f.open().history(fact.job_id).unwrap().unwrap();
    assert_eq!(history.fact.phase, AttachmentPhase::Closed);
    assert!(!history.legacy_closed);
    assert!(history.operations.is_empty());
}

#[test]
fn v1_replaced_requests_are_indexed_and_uuid_content_is_unique() {
    let f = Fixture::new();
    let mut first = f.fact.clone();
    first.transition = Some(f.old_close());
    let mut second = first.clone();
    second.transition.as_mut().unwrap().operation_id = Uuid::new_v4().to_string();
    second.transition.as_mut().unwrap().transition = RelayChannelTransitionKindV1::CancelReserved;
    let bytes = f.write_v1(&[first.clone(), second.clone()]);
    let mut journal = f.open();
    let history = journal.history(first.job_id).unwrap().unwrap();
    assert_eq!(history.operations.len(), 2);
    assert_eq!(
        history.operations[0].request.operation_id(),
        first.transition.as_ref().unwrap().operation_id
    );
    assert_eq!(
        history.operations[1].request.operation_id(),
        second.transition.as_ref().unwrap().operation_id
    );
    let begun = f.begin(&mut journal, second.clone());
    let response = f.proof(None);
    // Exact terminal absence resolves every retained operation, without a Core
    // snapshot, including the transition replaced in the old V1 fact.
    let resolved = journal
        .append_v2(
            first.job_id,
            ChannelChangeV2::History {
                pending_operation_id: None,
                response: response.clone(),
            },
            &f.literals,
        )
        .unwrap();
    assert!(resolved.operations.iter().all(|op| op.resolution.is_some()));
    assert!(resolved.fact.core.is_none());
    let count = journal.records.len();
    journal
        .append_v2(
            first.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(first.transition.as_ref().unwrap().operation_id.clone()),
                response,
            },
            &f.literals,
        )
        .unwrap();
    assert_eq!(journal.records.len(), count);
    let mut closed = resolved.fact;
    closed.phase = AttachmentPhase::Closed;
    closed.error = None;
    journal
        .append_v2(
            first.job_id,
            ChannelChangeV2::Finalize { fact: closed },
            &f.literals,
        )
        .unwrap();
    drop(journal);
    let directory = f.root.persistent_child(CHILD, false).unwrap().unwrap();
    for (index, bytes) in bytes.iter().enumerate() {
        assert_eq!(
            directory
                .persistent_read(&name(index as u64 + 1), MAX_RECORD as u64, &[])
                .unwrap()
                .unwrap()
                .bytes(),
            bytes
        );
    }
    assert_eq!(
        f.open()
            .history(first.job_id)
            .unwrap()
            .unwrap()
            .operations
            .len(),
        2
    );
    let g = Fixture::new();
    let mut a = g.fact.clone();
    a.transition = first.transition;
    // Bind both old rows to this second fixture before introducing only the
    // UUID/content divergence. They otherwise remain structurally valid V1.
    let admission = a.admission.as_ref().unwrap();
    let t = a.transition.as_mut().unwrap();
    t.channel_id = admission.channel_id.clone();
    t.admission_input_hash = admission.input_hash().unwrap();
    let mut b = a.clone();
    b.transition.as_mut().unwrap().transition = RelayChannelTransitionKindV1::CancelReserved;
    g.write_v1(&[a, b]);
    assert!(ChannelJournal::open(&g.root, &g.scope, &g.literals).is_err());
    assert_eq!(begun.ordinary_records, 2);
}

#[test]
fn persisted_proofs_revalidate_scope_and_exact_terminal_receipt() {
    let f = Fixture::new();
    let mut journal = f.open();
    journal.append(f.fact.clone(), &f.literals).unwrap();
    let mut alias = f.fact.clone();
    alias.job_id = Uuid::new_v4();
    assert!(
        matches!(
            journal.append(alias, &f.literals),
            Err(RelayOwnerError::Scope)
        ),
        "an admission UUID cannot alias another job"
    );
    f.begin(&mut journal, f.fact.clone());
    let proof = f.proof(None);
    let invalid_request = RelayChannelReconcileV1 {
        version: 0,
        admission: f.fact.admission.clone().unwrap(),
        pending: None,
    };
    assert!(
        crate::runner::relay_admission::validate_relay_channel_history(
            f.scope.tenant_id(),
            &invalid_request,
            &proof
        )
        .is_err(),
        "persisted proofs must validate the request before correlation"
    );
    let count = journal.records.len();
    let mut wrong = proof.clone();
    wrong.tenant_id = "tenant-b".into();
    assert!(
        journal
            .append_v2(
                f.fact.job_id,
                ChannelChangeV2::History {
                    pending_operation_id: None,
                    response: wrong
                },
                &f.literals
            )
            .is_err()
    );
    let mut wrong = proof.clone();
    if let RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal {
        terminal_observation_receipt,
        ..
    } = &mut wrong.outcome
    {
        terminal_observation_receipt.operation_id = Uuid::new_v4().to_string();
    }
    assert!(
        journal
            .append_v2(
                f.fact.job_id,
                ChannelChangeV2::History {
                    pending_operation_id: None,
                    response: wrong
                },
                &f.literals
            )
            .is_err()
    );
    assert_eq!(journal.records.len(), count);
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: None,
                response: proof,
            },
            &f.literals,
        )
        .unwrap();
    drop(journal);
    assert!(
        f.open()
            .history(f.fact.job_id)
            .unwrap()
            .unwrap()
            .admission_proof
            .is_some()
    );
}

#[test]
fn legacy_sixteen_records_keep_terminal_reserve_and_new_jobs_reserve_forty_eight() {
    let f = Fixture::new();
    let mut fact = f.fact.clone();
    fact.admission = None;
    let mut facts = Vec::new();
    for index in 0..16 {
        let mut next = fact.clone();
        next.error = if index % 2 == 0 {
            None
        } else {
            Some(RelayOwnerError::Unknown)
        };
        facts.push(next);
    }
    f.write_v1(&facts);
    let mut journal = f.open();
    assert_eq!(
        journal
            .history(fact.job_id)
            .unwrap()
            .unwrap()
            .ordinary_records,
        16
    );
    assert!(matches!(
        journal.append(fact.clone(), &f.literals),
        Err(RelayOwnerError::Capacity)
    ));
    let begun = f.begin(&mut journal, facts.last().unwrap().clone());
    assert_eq!(begun.terminal_records, 1);
    let mut closed = begun.fact;
    closed.error = None;
    closed.phase = AttachmentPhase::Closed;
    journal
        .append_v2(
            fact.job_id,
            ChannelChangeV2::Finalize { fact: closed },
            &f.literals,
        )
        .unwrap();
    drop(journal);
    let history = f.open().history(fact.job_id).unwrap().unwrap();
    assert_eq!(
        (history.ordinary_records, history.terminal_records),
        (16, 2)
    );

    let g = Fixture::new();
    let mut journal = g.open();
    let mut fact = g.fact.clone();
    fact.admission = None;
    for _ in 0..MAX_FACTS / PER_JOB {
        fact.job_id = Uuid::new_v4();
        journal.append(fact.clone(), &g.literals).unwrap();
    }
    let count = journal.records.len();
    fact.job_id = Uuid::new_v4();
    assert!(matches!(
        journal.append(fact, &g.literals),
        Err(RelayOwnerError::Capacity)
    ));
    assert_eq!(journal.records.len(), count);
    drop(journal);
    assert_eq!(g.open().reserved(), MAX_FACTS);
}

#[test]
fn full_canonical_envelope_is_bounded_before_any_private_file_mutation() {
    let f = Fixture::new();
    let mut journal = f.open();
    let mut fact = f.fact.clone();
    let admission = fact.admission.as_mut().unwrap();
    // This remains a valid local admission DTO, not a claim that the Core
    // accepted the changed synthetic image identity. It tests encoding only.
    admission.inspection.image_id.clear();
    let empty_size = serde_json::to_vec(admission).unwrap().len();
    admission.inspection.image_id = "x".repeat(8192 - empty_size);
    admission.validate().unwrap();
    let inspection = admission.inspection.clone();
    journal.append(fact.clone(), &f.literals).unwrap();
    let mut terminal = f.terminal.clone();
    if let WorkerTransitionV1::Observe { observation } = &mut terminal.request.transition {
        observation.inspection = inspection;
    }
    terminal.receipt.input_hash =
        content_hash("ai-guard:worker-mutation:v1", &terminal.request).unwrap();
    fact.local_joined = true;
    fact.phase = AttachmentPhase::Unknown;
    fact.error = Some(RelayOwnerError::Unknown);
    let change = ChannelChangeV2::BeginRetirement {
        fact: fact.clone(),
        terminal,
    };
    let bytes = super::super::super::journal::canonical(&RecordV2 {
        version: 2,
        sequence: 2,
        previous_sha256: journal.records.last().map(|file| file.digest().into()),
        job_id: fact.job_id,
        observed_at: chrono::Utc::now().to_rfc3339(),
        change: change.clone(),
    })
    .unwrap();
    assert!(
        bytes.len() > MAX_RECORD,
        "fixture must exceed the FULL envelope, not only the DTO"
    );
    let count = journal.records.len();
    assert!(matches!(
        journal.append_v2(fact.job_id, change, &f.literals),
        Err(RelayOwnerError::Capacity)
    ));
    assert_eq!(journal.records.len(), count);
    assert!(
        journal
            .history(fact.job_id)
            .unwrap()
            .unwrap()
            .terminal
            .is_none()
    );
}

#[test]
fn new_observations_cannot_regress_head_and_old_proofs_replay_without_writes() {
    let f = Fixture::with_reserved(true);
    let close = f.old_close();
    let mut first = f.fact.clone();
    first.transition = Some(close.clone());
    let admission = first.admission.as_ref().unwrap();
    let claim = RelayChannelClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        channel_id: admission.channel_id.clone(),
        epoch: admission.epoch,
        expected_revision: 1,
        admission_input_hash: admission.input_hash().unwrap(),
    };
    first.claim = Some(claim.clone());
    let mut second = first.clone();
    let ready = RelayChannelTransitionV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 2,
        transition: RelayChannelTransitionKindV1::Ready,
        ..close.clone()
    };
    second.transition = Some(ready.clone());
    // A structurally valid V1 future-CAS row is data, not an assertion that the
    // old writer could produce it. At revision2/Cancelled it is provably invalid.
    f.write_v1(&[first, second.clone()]);
    let mut journal = f.open();
    f.begin(&mut journal, second);
    let old_close = f.proof(Some(RelayChannelPendingV1::Transition(close.clone())));
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(close.operation_id.clone()),
                response: old_close.clone(),
            },
            &f.literals,
        )
        .unwrap();
    let cancel = RelayChannelTransitionV1 {
        operation_id: Uuid::new_v4().to_string(),
        transition: RelayChannelTransitionKindV1::CancelReserved,
        ..close
    };
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::RetirementIntent {
                request: cancel.clone(),
            },
            &f.literals,
        )
        .unwrap();
    let committed = f.store.transition_relay_channel(&cancel).unwrap();
    let delayed_claim = f.proof(Some(RelayChannelPendingV1::Claim(claim.clone())));
    let cancel_proof = f.proof(Some(RelayChannelPendingV1::Transition(cancel.clone())));
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(cancel.operation_id.clone()),
                response: cancel_proof,
            },
            &f.literals,
        )
        .unwrap();
    f.store
        .append(Event::new("journal_history_fixture_note"))
        .unwrap();
    let ready_proof = f.proof(Some(RelayChannelPendingV1::Transition(ready.clone())));
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(ready.operation_id),
                response: ready_proof,
            },
            &f.literals,
        )
        .unwrap();
    let count = journal.records.len();
    assert!(
        journal
            .append_v2(
                f.fact.job_id,
                ChannelChangeV2::History {
                    pending_operation_id: Some(claim.operation_id.clone()),
                    response: delayed_claim
                },
                &f.literals
            )
            .is_err()
    );
    assert_eq!(journal.records.len(), count);
    let mut closed = journal.history(f.fact.job_id).unwrap().unwrap().fact;
    closed.phase = AttachmentPhase::Closed;
    closed.error = None;
    assert!(
        journal
            .append_v2(
                f.fact.job_id,
                ChannelChangeV2::Finalize { fact: closed },
                &f.literals
            )
            .is_err(),
        "the pending claim must not disappear on a rejected proof"
    );
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(claim.operation_id.clone()),
                response: f.proof(Some(RelayChannelPendingV1::Claim(claim))),
            },
            &f.literals,
        )
        .unwrap();
    let count = journal.records.len();
    assert!(
        journal
            .append_v2(
                f.fact.job_id,
                ChannelChangeV2::History {
                    pending_operation_id: Some(f.old_close().operation_id),
                    response: old_close.clone()
                },
                &f.literals
            )
            .is_err()
    );
    // The original request identity is present inside this retained proof.
    let RelayChannelHistoryOutcomeV1::Historical {
        pending: Some(pending),
        ..
    } = &old_close.outcome
    else {
        panic!("missing exact pending");
    };
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(pending.pending.operation_id().into()),
                response: old_close,
            },
            &f.literals,
        )
        .unwrap();
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::Mutation {
                operation_id: cancel.operation_id,
                response: committed,
            },
            &f.literals,
        )
        .unwrap();
    assert_eq!(journal.records.len(), count);
    let mut closed = journal.history(f.fact.job_id).unwrap().unwrap().fact;
    closed.phase = AttachmentPhase::Closed;
    closed.error = None;
    journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::Finalize { fact: closed },
            &f.literals,
        )
        .unwrap();
    drop(journal);
    assert_eq!(
        f.open().history(f.fact.job_id).unwrap().unwrap().fact.phase,
        AttachmentPhase::Closed
    );
}

#[test]
fn v1_downgrade_after_v2_and_legacy_seventeenth_record_are_refused() {
    let f = Fixture::new();
    let mut fact = f.fact.clone();
    fact.admission = None;
    let mut journal = f.open();
    journal.append(fact.clone(), &f.literals).unwrap();
    drop(journal);
    let directory = f.root.persistent_child(CHILD, false).unwrap().unwrap();
    let previous = directory
        .persistent_read(&name(1), MAX_RECORD as u64, &[])
        .unwrap()
        .unwrap();
    let bytes = super::super::super::journal::canonical(
        &json!({"version":1,"sequence":2,"previous_sha256":previous.digest(),"fact":fact}),
    )
    .unwrap();
    write(&directory, &name(2), &bytes, &f.literals).unwrap();
    assert!(ChannelJournal::open(&f.root, &f.scope, &f.literals).is_err());
    let g = Fixture::new();
    let mut fact = g.fact.clone();
    fact.admission = None;
    let facts = vec![fact; 17];
    g.write_v1(&facts);
    assert!(matches!(
        ChannelJournal::open(&g.root, &g.scope, &g.literals),
        Err(RelayOwnerError::Capacity)
    ));
}

#[test]
fn structurally_valid_legacy_future_cas_stays_unknown_after_core_terminal() {
    let f = Fixture::with_reserved(true);
    let mut future = f.old_close();
    future.expected_revision = 4;
    let mut fact = f.fact.clone();
    fact.transition = Some(future.clone());
    f.write_v1(&[fact.clone()]);
    let mut journal = f.open();
    f.begin(&mut journal, fact);
    let cancel = RelayChannelTransitionV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        transition: RelayChannelTransitionKindV1::CancelReserved,
        ..future.clone()
    };
    f.store.transition_relay_channel(&cancel).unwrap();
    let proof = f.proof(Some(RelayChannelPendingV1::Transition(future.clone())));
    let history = journal
        .append_v2(
            f.fact.job_id,
            ChannelChangeV2::History {
                pending_operation_id: Some(future.operation_id),
                response: proof,
            },
            &f.literals,
        )
        .unwrap();
    assert_eq!(
        history.fact.core.as_ref().unwrap().state,
        RelayChannelStateV1::CancelledBeforeClaim
    );
    assert_eq!(history.fact.phase, AttachmentPhase::Unknown);
    assert!(history.operations[0].resolution.is_none());
    let mut closed = history.fact;
    closed.phase = AttachmentPhase::Closed;
    closed.error = None;
    assert!(
        journal
            .append_v2(
                f.fact.job_id,
                ChannelChangeV2::Finalize { fact: closed },
                &f.literals
            )
            .is_err()
    );
    drop(journal);
    let reopened = f.open().history(f.fact.job_id).unwrap().unwrap();
    assert_eq!(reopened.fact.phase, AttachmentPhase::Unknown);
    assert!(reopened.operations[0].resolution.is_none());
}
