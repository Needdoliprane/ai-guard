//! B1: the production worker replay validator must use the shared quota at
//! admission_seq, even after a V2 reservation closes. Only cloned records are
//! tightened below; no database row, auth proof or audit hash is rewritten.
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    control::{kubernetes_session::*, *},
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    runtime::kubernetes::session_profile::*,
};
use uuid::Uuid;

const MEMORY_MIB: u64 = 1_048_576;

fn reserve_session(f: &Fixture, quota: &QuotaPolicyV1) -> KubernetesSessionReservationIntentV2 {
    let path = f._directory.path().join("tenant.sqlite");
    f.store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    f.store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    f.store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    let binding = KubernetesSessionBindingV2::new(
        &f.intent.authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid: f.intent.control_namespace_uuid.parse().unwrap(),
            broker_id: f.intent.broker_id.parse().unwrap(),
            incarnation_id: f.intent.incarnation_id.parse().unwrap(),
            session_id: Uuid::new_v4(),
            attempt_id: Uuid::new_v4(),
        },
    )
    .unwrap();
    let provider = ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport: ClientTransport::Batch,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::new_v4(),
        credential_names: vec![crate::provider::BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    };
    let args = [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
        "--prompt",
        "public B1 quota test",
    ]
    .map(str::to_owned);
    let profile = KubernetesSessionProfileV2::build(
        &f.intent.authority,
        &binding,
        &f.intent.image,
        KubernetesSessionResourcesV2 {
            cpu_millis: 500,
            memory_mib: 128,
            temporary_mib: 8,
            home_mib: 8,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &crate::workspace::Manifest::new(),
    )
    .unwrap();
    let intent = KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        &f.intent.authority,
        &profile,
    )
    .unwrap();
    let policy = KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: f.intent.authority.authority_id().to_string(),
            authority_fingerprint: f.intent.authority.fingerprint(),
            image: f.intent.image.clone(),
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    };
    let auth = f
        .store
        .authorize_kubernetes_session_reservation(&f.identity, &intent, &policy, quota)
        .unwrap();
    let reserved = f
        .store
        .reserve_kubernetes_session(&f.identity, &auth, &intent, &policy, quota)
        .unwrap();
    assert!(!reserved.replayed);
    assert_eq!(reserved.snapshot.state.revision, 1);
    assert!(reserved.snapshot.state.quota_held);
    assert_eq!(
        reserved.snapshot.state.effect_fence,
        KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }
    );
    intent
}

fn admit_worker(f: &Fixture, quota: &QuotaPolicyV1) -> (WorkerV1, i64, ControlRecord) {
    let session_id = Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = policy::Policy::default();
    let allow = policy::evaluate(&policy, &request);
    assert_eq!(allow.outcome, Outcome::Allow);
    let decision = f
        .store
        .decision(&request, Some((allow, json!(null))))
        .unwrap()
        .unwrap();
    let admission = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: "c".repeat(64),
        admission: AdmissionV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            receipt: DecisionReceiptV1 {
                version: 1,
                request_id: request.request_id.clone(),
                request_hash: policy::request_hash(&request),
                phase: DecisionPhaseV1::Initial,
                audit_seq: decision.seq,
                audit_hash: decision.hash,
            },
            launch: LaunchSpecV1 {
                version: 1,
                session_id,
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: Uuid::new_v4().to_string(),
                image_ref: "worker:fixture".into(),
                image_id: format!("sha256:{}", "a".repeat(64)),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1024 * MEMORY_MIB,
                pids_limit: 256,
                timeout_seconds: 90,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: "b".repeat(64),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: vec![],
                provider_resolution: None,
                egress: None,
            },
            decision_request: request,
        },
    };
    let admitted = f
        .store
        .admit_supervised(
            &admission,
            &policy,
            policy::evaluate(&policy, &admission.admission.decision_request),
            json!(null),
            quota,
        )
        .unwrap();
    assert!(admitted.accepted);
    assert!(!admitted.replayed);
    let worker = admitted.worker.unwrap();
    let seq = admitted.receipt.audit_seq;
    let db = f.store.db.lock().unwrap();
    let stored = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
            [seq],
            row_event,
        )
        .unwrap();
    assert_eq!(stored.event.kind, "control.supervision.transition");
    let record: ControlRecord =
        serde_json::from_value(stored.event.data["worker_record"].clone()).unwrap();
    assert_eq!(
        record.admission_policy.as_ref().unwrap()["quota"],
        serde_json::to_value(quota).unwrap()
    );
    assert_eq!(record.worker.as_ref().unwrap().worker_id, worker.worker_id);
    // The exact validator used by normal replay, with its real retained record.
    validate_admission_input(&db, &worker, &record, seq).unwrap();
    (worker, seq, record)
}

#[test]
fn worker_admission_replay_counts_v2_at_its_prefix_after_close_and_reopen() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1 {
        version: 1,
        max_active_workers: 2,
        max_cpu_millis: 2500,
        max_memory_bytes: 1152 * MEMORY_MIB,
        max_timeout_seconds: 90,
    };
    let intent = reserve_session(&f, &quota);
    let (worker, admission_seq, record) = admit_worker(&f, &quota);
    let held = f.store.worker_quota(&quota).unwrap();
    assert_eq!(
        (
            held.held_workers,
            held.held_cpu_millis,
            held.held_memory_bytes
        ),
        (2, 2500, 1152 * MEMORY_MIB)
    );
    assert!(f.store.integrity().unwrap().valid);
    let close = KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: intent.reservation_id().into(),
        reservation_input_hash: intent.input_hash("tenant-a").unwrap(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::OperatorCancelled,
    };
    let auth = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            intent.reservation_id(),
            Some(&close),
        )
        .unwrap();
    let closed = f
        .store
        .close_kubernetes_session_before_effect(&f.identity, &auth, &close)
        .unwrap();
    assert!(!closed.replayed);
    assert!(closed.receipt.audit_seq > admission_seq);
    assert!(!closed.snapshot.state.quota_held);
    let head = f.store.head().unwrap();
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let store =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
    let current = store.worker_quota(&quota).unwrap();
    assert_eq!(
        (
            current.held_workers,
            current.held_cpu_millis,
            current.held_memory_bytes
        ),
        (1, 2000, 1024 * MEMORY_MIB)
    );
    assert_eq!(store.head().unwrap(), head);

    let mut unexpected_acceptances = Vec::new();
    {
        let db = store.db.lock().unwrap();
        let version: i64 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 11);
        validate_admission_input(&db, &worker, &record, admission_seq).unwrap();
        // Same immutable worker/decision/input_hash/prefix, but looser legal
        // historical limits. A generic mismatch check must not become the
        // reason that all tightened-quota candidates are rejected.
        let mut relaxed = record.clone();
        relaxed.admission_policy.as_mut().unwrap()["quota"] = serde_json::to_value(QuotaPolicyV1 {
            version: 1,
            max_active_workers: 3,
            max_cpu_millis: 2501,
            max_memory_bytes: 1152 * MEMORY_MIB + 1,
            max_timeout_seconds: 91,
        })
        .unwrap();
        validate_admission_input(&db, &worker, &relaxed, admission_seq)
            .expect("looser historical quota must pass at the same actual admission prefix");
        // The four independently tightened historical limits are all still
        // legal QuotaPolicyV1 values. Only a private clone changes; its linked
        // worker, decision, input hash and admission prefix remain exact.
        for (field, value) in [
            ("max_active_workers", 1u64),
            ("max_cpu_millis", 2499),
            ("max_memory_bytes", 1152 * MEMORY_MIB - 1),
            ("max_timeout_seconds", 89),
        ] {
            let mut candidate = record.clone();
            candidate.admission_policy.as_mut().unwrap()["quota"][field] = json!(value);
            let changed: QuotaPolicyV1 = serde_json::from_value(
                candidate.admission_policy.as_ref().unwrap()["quota"].clone(),
            )
            .unwrap();
            changed.validate().unwrap();
            if validate_admission_input(&db, &worker, &candidate, admission_seq).is_ok() {
                unexpected_acceptances.push(field);
            }
        }
    }
    assert_eq!(store.head().unwrap(), head);
    let after = store.worker_quota(&quota).unwrap();
    assert_eq!(
        (
            after.held_workers,
            after.held_cpu_millis,
            after.held_memory_bytes
        ),
        (1, 2000, 1024 * MEMORY_MIB)
    );
    assert!(store.integrity().unwrap().valid);
    eprintln!(
        "schema11_b1_preconditions_verified real_store=11 broker_binding=authenticated grant=real v2_reserve=500mCPU/128MiB worker=2000mCPU/1024MiB historical_quota=2/2500mCPU/1152MiB/90s relaxed_quota=accepted close_after_admission=true reopen=inactive audit=unchanged"
    );
    assert!(
        unexpected_acceptances.is_empty(),
        "schema11_worker_historical_quota_missing: replay accepted tightened historical quota fields {unexpected_acceptances:?} at the real worker admission prefix"
    );
}
