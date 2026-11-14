use super::*;
use crate::control_fs::PrivateFile;
use crate::human_authority::{
    HumanSourceKindV1, HumanSourcesConfigurationV1, OwnedHumanSources,
    startup::{HumanOwnedSourceOptions, HumanOwnedStartupBundle, HumanOwnedStartupRequest},
};
use crate::redaction::Redactor;
use crate::{
    audit::workload::owned_broker_identity::OwnedBrokerBindingStepV1,
    human_authority::{
        OwnedSourceToken,
        migration::{MigratedHumanSources, tests::Fixture},
    },
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

trait Bundle {
    fn owner(&self) -> &OwnedHumanSources;
    fn store(&self, id: Uuid) -> Option<&Store>;
}
impl Bundle for MigratedHumanSources {
    fn owner(&self) -> &OwnedHumanSources {
        MigratedHumanSources::owner(self)
    }
    fn store(&self, id: Uuid) -> Option<&Store> {
        MigratedHumanSources::store(self, id)
    }
}
impl Bundle for HumanOwnedStartupBundle {
    fn owner(&self) -> &OwnedHumanSources {
        &self.owner
    }
    fn store(&self, id: Uuid) -> Option<&Store> {
        self.stores.get(&id)
    }
}

/// Real drop/reopen of the same complete local union. No SQL fabricates the
/// post-rotation startup receipt or releases a retained old epoch.
fn request_for_reopen(
    closure: &crate::human_authority::HumanAuthorityClosureV1,
    manifest: PrivateFile,
) -> HumanOwnedStartupRequest {
    let files = closure
        .configuration
        .files
        .iter()
        .map(|file| {
            (
                file.kind,
                PrivateFile::open(&file.storage_binding.path(), 1_048_576).unwrap(),
            )
        })
        .collect();
    let configuration = HumanSourcesConfigurationV1::new(
        closure.sources.clone(),
        closure.configuration.clone(),
        files,
    )
    .unwrap();
    let sources = closure
        .sources
        .iter()
        .map(|source| {
            assert_eq!(source.kind, HumanSourceKindV1::TenantStore);
            let capacity = crate::audit::AuditCapacityPolicyV1::resolved(10_000);
            (
                source.source_id,
                HumanOwnedSourceOptions::TenantStore {
                    redactor: Redactor::new(vec![]),
                    max_events: 10_000,
                    workload: crate::audit::WorkloadAuditPolicyV1::resolved(10_000, &capacity),
                    capacity,
                    retention: crate::relay::RelayRetentionPolicyV1::default(),
                },
            )
        })
        .collect();
    HumanOwnedStartupRequest {
        manifest,
        configuration,
        sources,
    }
}
fn restart(bundle: MigratedHumanSources) -> HumanOwnedStartupBundle {
    let request = request_for_reopen(bundle.owner().closure(), bundle.manifest().clone());
    drop(bundle);
    HumanOwnedStartupBundle::open(request).unwrap()
}
fn restart_owned(
    bundle: HumanOwnedStartupBundle,
    manifest: PrivateFile,
) -> HumanOwnedStartupBundle {
    let request = request_for_reopen(bundle.owner.closure(), manifest);
    drop(bundle);
    HumanOwnedStartupBundle::open(request).unwrap()
}

const OLD: &str = "synthetic-owned-rotation-old-bearer-000000000000";
const NEW: &str = "synthetic-owned-rotation-new-bearer-000000000000";

fn source(bundle: &impl Bundle) -> OwnedSourceToken {
    let id = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|s| s.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .source_id;
    bundle.owner().source_token(id).unwrap()
}
fn grant() -> BrokerGrantMutationV1 {
    BrokerGrantMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: Uuid::new_v4().to_string(),
        expected_revision: None,
        credential_id: Uuid::new_v4().to_string(),
        incarnation_id: Uuid::new_v4().to_string(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: "owned-reconciliation-fixture".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    }
}
fn issue(grant: &BrokerGrantMutationV1, revision: u64, token: &str) -> WorkloadBindingIssueV1 {
    WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: grant.credential_id.clone(),
        expected_revision: revision,
        subject: WorkloadSubjectV1::Broker {
            broker_id: grant.broker_id.clone(),
            incarnation_id: grant.incarnation_id.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        expires_at: grant.not_after.clone(),
    }
}
fn enroll(bundle: &impl Bundle, grant: &BrokerGrantMutationV1, token: &str) {
    let src = source(bundle);
    let store = bundle.store(src.source_id()).unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    store.owned_broker_grant(&epoch, &src, grant).unwrap();
    store
        .owned_broker_binding_issue(
            &epoch,
            &src,
            OwnedBrokerBindingStepV1::Issue,
            &issue(grant, 0, token),
        )
        .unwrap();
}
fn rotate(bundle: &impl Bundle, grant: &BrokerGrantMutationV1) {
    let src = source(bundle);
    let store = bundle.store(src.source_id()).unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    store
        .owned_broker_binding_issue(
            &epoch,
            &src,
            OwnedBrokerBindingStepV1::Rotate,
            &issue(grant, 1, NEW),
        )
        .unwrap();
}
fn append_same_owner_incarnations(bundle: &mut HumanOwnedStartupBundle) {
    let src = source(bundle);
    let epoch = bundle.owner.try_write_epoch().unwrap();
    // Use the actual startup producer before retention. Mutating authority
    // snapshots of an already retained Store is deliberately not permitted.
    let mut opening = Store::open_review_startup_for_tenant(
        &src.descriptor().storage_binding.path(),
        "A",
        Redactor::new(vec![]),
        10_000,
        &src,
    )
    .unwrap();
    let capacity = crate::audit::AuditCapacityPolicyV1::resolved(10_000);
    opening
        .configure_owned_read_policies(
            capacity.clone(),
            crate::audit::WorkloadAuditPolicyV1::resolved(10_000, &capacity),
            crate::relay::RelayRetentionPolicyV1::default(),
        )
        .unwrap();
    for _ in 0..2 {
        let revision = opening.current_incarnation_revision(&epoch).unwrap();
        opening
            .append_incarnation(&epoch, &src, Uuid::new_v4(), revision)
            .unwrap();
    }
    let current = opening.retain(&epoch, &src).unwrap();
    drop(epoch);
    let old = bundle.stores.insert(src.source_id(), current).unwrap();
    drop(old);
}

fn online<T>(
    bundle: &impl Bundle,
    token: &str,
    work: impl FnOnce(
        &Store,
        &HumanSourceAccess<'_>,
        &OwnedBrokerWindow<'_>,
        &OwnedRetainedSourceFacts,
        &BrokerIdentityV1,
    ) -> T,
) -> T {
    let src = source(bundle);
    let store = bundle.store(src.source_id()).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &src).unwrap();
    let facts = OwnedRetainedSourceFacts::from_token(&src).unwrap();
    let generation = store.modern_discovery_generation(&access).unwrap();
    let window = OwnedBrokerWindow::new(generation, token);
    let broker = store
        .owned_window_authenticate(&access, &window, &facts)
        .unwrap();
    work(store, &access, &window, &facts, &broker)
}
fn pending(bundle: &impl Bundle) -> OwnedHeartbeatPendingV2 {
    online(bundle, OLD, |store, access, window, facts, broker| {
        let view = store
            .owned_broker_self_view(access, window, facts, broker)
            .unwrap();
        OwnedHeartbeatPendingV2::prepare(&view, Uuid::new_v4().to_string()).unwrap()
    })
}
fn count(bundle: &impl Bundle) -> i64 {
    let src = source(bundle);
    bundle
        .store(src.source_id())
        .unwrap()
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn committed_lost_response_is_acknowledged_under_original_actor_after_real_rotation_without_write()
{
    let fixture = Fixture::new_owned_local();
    let bundle = fixture.migrated();
    let grant = grant();
    enroll(&bundle, &grant, OLD);
    let pending = pending(&bundle);
    let committed = online(&bundle, OLD, |store, access, window, facts, broker| {
        store
            .owned_broker_heartbeat(access, window, facts, broker, &pending.request())
            .unwrap()
    });
    rotate(&bundle, &grant);
    let bundle = restart(bundle);
    let before = count(&bundle);
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending.clone()),
        previous: None,
    };
    let proof = online(&bundle, NEW, |store, access, window, facts, broker| {
        assert!(
            store
                .owned_broker_heartbeat(access, window, facts, broker, &pending.request())
                .is_err()
        );
        store
            .owned_broker_reconcile(access, window, facts, broker, &request)
            .unwrap()
    });
    proof.validate_for(&request).unwrap();
    assert_eq!(proof.observation.subject.binding_revision, 2);
    match proof.state {
        OwnedReconciliationStateV2::CommittedExact {
            historical: OwnedHistoricalResultV2::Heartbeat { result },
        } => {
            assert_eq!(result.identity.subject.binding_revision, 1);
            assert_eq!(result.result.receipt, committed.result.receipt);
        }
        _ => panic!("exact committed heartbeat required"),
    }
    assert_eq!(count(&bundle), before);
}

#[test]
fn noncommitted_requires_rotation_and_owner_barrier_and_never_uses_sequence_alone() {
    let fixture = Fixture::new_owned_local();
    let migrated = fixture.migrated();
    let manifest = migrated.manifest().clone();
    let mut bundle = restart(migrated);
    let grant = grant();
    enroll(&bundle, &grant, OLD);
    let pending = pending(&bundle);
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending),
        previous: None,
    };
    online(&bundle, OLD, |store, access, window, facts, broker| {
        assert!(bundle.owner().try_write_epoch().is_err());
        assert!(
            store
                .owned_broker_reconcile(access, window, facts, broker, &request)
                .is_err()
        );
    });
    rotate(&bundle, &grant);
    append_same_owner_incarnations(&mut bundle);
    let before_restart = online(&bundle, NEW, |store, access, window, facts, broker| {
        store.owned_broker_reconcile(access, window, facts, broker, &request)
    });
    assert_eq!(
        before_restart.unwrap_err(),
        Code::IdentityMismatch,
        "multiple receipts in the same owner incarnation are not an offline fence"
    );
    let bundle = restart_owned(bundle, manifest);
    let before = count(&bundle);
    let proof = online(&bundle, NEW, |store, access, window, facts, broker| {
        store
            .owned_broker_reconcile(access, window, facts, broker, &request)
            .unwrap()
    });
    proof.non_committed_for(&request).unwrap();
    assert_ne!(
        proof.fence.previous_owner_incarnation,
        proof.fence.owner_incarnation
    );
    assert_eq!(count(&bundle), before);
}

#[test]
fn foreign_source_token_and_torn_binding_history_never_become_noncommitted() {
    let fixture = Fixture::new_owned_local();
    let bundle = fixture.migrated();
    let grant = grant();
    enroll(&bundle, &grant, OLD);
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending(&bundle)),
        previous: None,
    };
    rotate(&bundle, &grant);
    let bundle = restart(bundle);
    let other_id = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|s| s.tenant_id.as_deref() == Some("B"))
        .unwrap()
        .source_id;
    let other = bundle.owner.source_token(other_id).unwrap();
    let observed = crate::audit::read_test_observer::observe(bundle.stores.get(&other_id).unwrap());
    let before = count(&bundle);
    online(&bundle, NEW, |store, access, window, facts, broker| {
        store
            .owned_business(
                access,
                &OwnedProof::Windowed(window, broker),
                false,
                |tx, clock| {
                    let current = store.owned_current(tx, clock, facts, broker)?;
                    let own = store.review_authority.as_ref().unwrap().source_token();
                    checked_rotation_fence(tx, own, request.identity(), &current.identity).unwrap();
                    let error =
                        checked_rotation_fence(tx, &other, request.identity(), &current.identity)
                            .unwrap_err();
                    assert_eq!(
                        error.downcast_ref::<crate::workload::WorkloadFailureV1>(),
                        Some(&crate::workload::WorkloadFailureV1::Conflict)
                    );
                    Ok(())
                },
            )
            .unwrap();
        assert!(observed.try_recv().is_err(), "foreign source is not read");
        // Synthetic fault in this test's private SQLite projection. The real
        // checked read must fail; no absence oracle may hide the torn row.
        store.db.lock().unwrap().execute("UPDATE workload_binding_history SET payload_json='{' WHERE credential_id=?1 AND revision=1",[&grant.credential_id]).unwrap();
        assert!(
            store
                .owned_broker_reconcile(access, window, facts, broker, &request)
                .is_err()
        );
    });
    assert_eq!(count(&bundle), before);
}

#[test]
fn stale_generation_and_revoked_bearer_cannot_reconcile_with_an_old_verified_proof() {
    let fixture = Fixture::new_owned_local();
    let bundle = fixture.migrated();
    let grant = grant();
    enroll(&bundle, &grant, OLD);
    let pending = pending(&bundle);
    let old_generation = online(&bundle, OLD, |store, access, _, _, _| {
        store.modern_discovery_generation(access).unwrap()
    });
    rotate(&bundle, &grant);
    let bundle = restart(bundle);
    let cached = online(&bundle, NEW, |_, _, _, _, broker| broker.clone());
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending),
        previous: None,
    };
    let src = source(&bundle);
    let store = bundle.stores.get(&src.source_id()).unwrap();
    let facts = OwnedRetainedSourceFacts::from_token(&src).unwrap();
    {
        let epoch = bundle.owner.try_read_epoch().unwrap();
        let access = HumanSourceAccess::read(&epoch, &src).unwrap();
        let stale = OwnedBrokerWindow::new(old_generation, NEW);
        let before = count(&bundle);
        assert_eq!(
            store
                .owned_broker_reconcile(&access, &stale, &facts, &cached, &request)
                .unwrap_err(),
            Code::SourceRebuildRequired
        );
        assert!(stale.stale());
        assert_eq!(count(&bundle), before);
    }
    {
        let epoch = bundle.owner.try_write_epoch().unwrap();
        store
            .owned_broker_binding_revoke(
                &epoch,
                &src,
                &grant.broker_id,
                &crate::workload::WorkloadBindingRevocationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    credential_id: grant.credential_id.clone(),
                    expected_revision: 2,
                },
            )
            .unwrap();
    }
    let before = count(&bundle);
    let epoch = bundle.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &src).unwrap();
    let current = OwnedBrokerWindow::new(store.modern_discovery_generation(&access).unwrap(), NEW);
    assert_eq!(
        store
            .owned_broker_reconcile(&access, &current, &facts, &cached, &request)
            .unwrap_err(),
        Code::Forbidden
    );
    assert_eq!(count(&bundle), before);
}

#[test]
fn global_operation_collision_with_another_broker_is_not_noncommitted() {
    let fixture = Fixture::new_owned_local();
    let bundle = fixture.migrated();
    let original = grant();
    enroll(&bundle, &original, OLD);
    let mut pending = pending(&bundle);
    let mut other = grant();
    other.authorities[0].authority_id = "owned-reconciliation-other-fixture".into();
    let token = "synthetic-owned-other-broker-bearer-000000000000";
    enroll(&bundle, &other, token);
    let committed_id = online(&bundle, token, |store, access, window, facts, broker| {
        let view = store
            .owned_broker_self_view(access, window, facts, broker)
            .unwrap();
        let operation =
            OwnedHeartbeatPendingV2::prepare(&view, Uuid::new_v4().to_string()).unwrap();
        store
            .owned_broker_heartbeat(access, window, facts, broker, &operation.request())
            .unwrap();
        operation.heartbeat.operation_id
    });
    pending.heartbeat.operation_id = committed_id;
    let commitments = heartbeat_commitments(&pending.identity.subject, &pending.heartbeat).unwrap();
    pending.registry_input_hash = commitments.registry_input_hash;
    pending.operation_commitment = commitments.operation_commitment;
    rotate(&bundle, &original);
    let bundle = restart(bundle);
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending),
        previous: None,
    };
    let before = count(&bundle);
    let error = online(&bundle, NEW, |store, access, window, facts, broker| {
        store
            .owned_broker_reconcile(access, window, facts, broker, &request)
            .unwrap_err()
    });
    assert_eq!(error, Code::OperationConflict);
    assert_eq!(count(&bundle), before);
}

#[test]
fn checkpoint_historical_ack_and_global_different_kind_collision_are_distinct() {
    let fixture = Fixture::new_owned_local();
    let bundle = fixture.migrated();
    let grant = grant();
    enroll(&bundle, &grant, OLD);
    let checkpoint = online(&bundle, OLD, |store, access, window, facts, broker| {
        let identity = store
            .owned_window_identity(access, window, facts, broker)
            .unwrap();
        let pending = OwnedCheckpointPendingV2::prepare(
            &identity,
            0,
            BrokerCheckpointSnapshotV1 {
                version: 1,
                authority: grant.authorities[0].clone(),
                jobs: vec![BrokerJobCheckpointV1 {
                    job_id: Uuid::new_v4().to_string(),
                    revision: 4,
                    checkpoint_hash: "e".repeat(64),
                }],
                has_more: true,
            },
            Uuid::new_v4().to_string(),
        )
        .unwrap();
        store
            .owned_broker_checkpoint(access, window, facts, broker, &pending.request())
            .unwrap();
        pending
    });
    let mut heartbeat = pending(&bundle);
    heartbeat.heartbeat.operation_id = checkpoint.checkpoint.operation_id.clone();
    let hash = heartbeat_commitments(&heartbeat.identity.subject, &heartbeat.heartbeat).unwrap();
    heartbeat.registry_input_hash = hash.registry_input_hash;
    heartbeat.operation_commitment = hash.operation_commitment;
    rotate(&bundle, &grant);
    let bundle = restart(bundle);
    let before = count(&bundle);
    let cp_request = OwnedReconciliationRequestV2::Checkpoint {
        pending: Box::new(checkpoint.clone()),
        previous: None,
    };
    let proof = online(&bundle, NEW, |store, access, window, facts, broker| {
        store
            .owned_broker_reconcile(access, window, facts, broker, &cp_request)
            .unwrap()
    });
    proof.validate_for(&cp_request).unwrap();
    match proof.state {
        OwnedReconciliationStateV2::CommittedExact {
            historical: OwnedHistoricalResultV2::Checkpoint { result },
        } => {
            assert_eq!(result.result.checkpoint, checkpoint.checkpoint);
            assert_eq!(result.identity.subject.binding_revision, 1);
        }
        _ => panic!("exact historical checkpoint required"),
    }
    let hb_request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(heartbeat),
        previous: None,
    };
    let error = online(&bundle, NEW, |store, access, window, facts, broker| {
        store
            .owned_broker_reconcile(access, window, facts, broker, &hb_request)
            .unwrap_err()
    });
    assert_eq!(error, Code::OperationConflict);
    assert_eq!(count(&bundle), before);
}

#[tokio::test]
async fn reserved_reconciliation_authenticates_before_body_and_publishes_real_scoped_absence_without_write()
 {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;
    let fixture = Fixture::new_owned_local();
    let bundle = fixture.migrated();
    let grant = grant();
    enroll(&bundle, &grant, OLD);
    let request = OwnedReconciliationRequestV2::Heartbeat {
        pending: Box::new(pending(&bundle)),
        previous: None,
    };
    rotate(&bundle, &grant);
    let tokens = ["A", "B"]
        .into_iter()
        .map(|tenant| {
            (
                tenant.to_owned(),
                format!("synthetic-{tenant}-runner-credential-long-enough"),
                format!("synthetic-{tenant}-viewer-credential-long-enough"),
            )
        })
        .collect::<Vec<_>>();
    let redactor = tokens.iter().fold(Redactor::new(vec![]), |redactor, (_, runner, viewer)| {
        redactor.union(&Redactor::new(vec![runner.clone(), viewer.clone()]))
    });
    let mut reopening = request_for_reopen(bundle.owner().closure(), bundle.manifest().clone());
    // Configure the exact gateway credentials before retaining the reopened Stores.
    for options in reopening.sources.values_mut() {
        if let HumanOwnedSourceOptions::TenantStore { redactor: value, .. } = options {
            *value = redactor.clone();
        }
    }
    drop(bundle);
    let bundle = HumanOwnedStartupBundle::open(reopening).unwrap();
    let closure = bundle.owner.closure();
    let credentials = closure
        .configuration
        .files
        .iter()
        .find(|file| {
            file.kind == crate::human_authority::HumanConfigurationFileKindV1::TechnicalCredentials
        })
        .unwrap();
    let credentials = PrivateFile::open(&credentials.storage_binding.path(), 1_048_576).unwrap();
    let database = source(&bundle).descriptor().storage_binding.path();
    let events = || {
        rusqlite::Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM events", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    let state = crate::api::GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let router = crate::api::gateway_router(state);
    let before = events();
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&polls);
    let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(
            axum::body::Bytes::from_static(b"{"),
        )))
    }));
    let rejected = router
        .clone()
        .oneshot(
            Request::post("/control/api/v1/owned/brokers/heartbeats/reconcile")
                .header(
                    "authorization",
                    "Bearer synthetic-unbound-broker-token-000000000000",
                )
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let response = router
        .oneshot(
            Request::post("/control/api/v1/owned/brokers/heartbeats/reconcile")
                .header("authorization", format!("Bearer {NEW}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), MAX_OWNED_RECONCILIATION_RESULT_BYTES)
        .await
        .unwrap();
    let proof: OwnedReconciliationResultV2 = serde_json::from_slice(&bytes).unwrap();
    proof.non_committed_for(&request).unwrap();
    assert_eq!(proof.observation.subject.tenant_id, "A");
    assert_eq!(events(), before);
}
