//! Authenticated discovery over real gateway/SQLite and framed Unix IPC.
//! No provider, runtime, restoration or fresh start. The recovery-mode fixture
//! owns an empty native enforcer only to establish the real STOP-only mode.
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn recovery_inventory_cold_profile_requires_explicit_recovery_without_fencing_normal_service()
{
    let gateway = Gateway::inactive(true).await;
    let mut fixture =
        Fixture::with_manual_arm_and_identity(false, Some((&gateway.url, gateway.namespace)));
    let operation = fixture.operation;
    let scope = fixture.writer.scope().clone();
    // The same read may reuse an already-held Full facade, without performing
    // business loading or choosing a new actor.
    let response = fixture
        .service
        .recovery_inventory_for_profile(
            fixture
                .service
                .registry
                .profile(scope.profile_id())
                .unwrap(),
            &scope,
            None,
            20,
        )
        .await
        .unwrap();
    assert!(
        matches!(response.reply, BrokerReplyV1::RecoveryInventory { inventory } if inventory.journal_mode == JournalModeV1::Full)
    );
    fixture.service.profiles =
        BTreeMap::from([("cancel_retry_test".into(), Arc::new(ProfileJobs::new()))]);
    let service = Arc::new(fixture.service);
    drop((fixture.jobs, fixture.writer, fixture.journal));
    assert!(!service.fenced.load(Ordering::SeqCst));
    assert!(matches!(
        dispatch(
            &service,
            &gateway,
            A_RUNNER,
            BrokerCommandV1::RecoveryInventory {
                after: None,
                limit: 20
            }
        )
        .await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    assert!(
        !service.fenced.load(Ordering::SeqCst),
        "a read cannot fence other profiles"
    );
    assert!(
        service
            .profile_jobs(scope.profile_id())
            .unwrap()
            .writer
            .get()
            .is_none()
    );
    let mut service = Arc::try_unwrap(service).unwrap_or_else(|_| panic!("no running request"));
    let (_role_temp, role) = recovery_role_fixture();
    assert_eq!(
        role.path().parent().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    let child = recovery_child(&role, None);
    let client = warmed_recovery_client(&role).await;
    service.enforcer = None;
    service.stop_enforcer = Some(client.clone());
    service.fenced.store(true, Ordering::SeqCst);
    let service = Arc::new(service);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let response = loop {
        let result = dispatch(
            &service,
            &gateway,
            A_RUNNER,
            BrokerCommandV1::RecoveryInventory {
                after: None,
                limit: 20,
            },
        )
        .await;
        match result {
            Ok(response) => break response,
            Err(_) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "old fixture queue must release its lease"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    };
    let BrokerReplyV1::RecoveryInventory { inventory } = response.reply else {
        panic!("inventory");
    };
    assert_eq!(inventory.journal_mode, JournalModeV1::StopOnly);
    assert!(!inventory.incomplete);
    assert!(inventory.reconciliation_required);
    assert_eq!(inventory.entries[0].operation_id, operation);
    let jobs = service.profile_jobs(scope.profile_id()).unwrap();
    let capabilities = jobs.writer.get().unwrap();
    assert!(capabilities.full().is_none());
    let second = dispatch(
        &service,
        &gateway,
        A_RUNNER,
        BrokerCommandV1::RecoveryStatus {
            operation_id: operation,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(second.reply, BrokerReplyV1::RecoveryStatus { status } if status.journal_mode == JournalModeV1::StopOnly)
    );
    assert!(
        capabilities.full().is_none(),
        "later reads cannot promote the retained actor"
    );
    drop((jobs, service, client));
    drop(child);
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_inventory_fresh_identity_guards_precede_journal_initialization() {
    let gateway = Gateway::inactive(true).await;
    for (credential, namespace, wanted) in [
        (A_VIEWER, gateway.namespace, BrokerErrorCodeV1::Forbidden),
        (B_RUNNER, gateway.namespace, BrokerErrorCodeV1::Forbidden),
        (A_RUNNER, Uuid::new_v4(), BrokerErrorCodeV1::Forbidden),
        (
            "unknown-valid-shape-inventory-token",
            gateway.namespace,
            BrokerErrorCodeV1::Unauthorized,
        ),
    ] {
        let fixture = fresh_namespace_fixture(&gateway, namespace);
        let root = fixture.service.root.path().to_owned();
        let service = Arc::new(fixture.service);
        assert!(matches!(dispatch(&service, &gateway, credential,
            BrokerCommandV1::RecoveryInventory { after: None, limit: 20 }).await,
            Err(code) if code == wanted));
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);
        assert!(
            service
                .profile_jobs("cancel_retry_test")
                .unwrap()
                .writer
                .get()
                .is_none()
        );
    }
    let fixture = fresh_namespace_fixture(&gateway, gateway.namespace);
    let root = fixture.service.root.path().to_owned();
    let service = Arc::new(fixture.service);
    for (after, limit) in [(None, 0), (None, 101), (Some(Uuid::nil()), 20)] {
        assert!(matches!(
            dispatch(
                &service,
                &gateway,
                A_RUNNER,
                BrokerCommandV1::RecoveryInventory { after, limit }
            )
            .await,
            Err(BrokerErrorCodeV1::InvalidInput)
        ));
        assert!(
            service
                .profile_jobs("cancel_retry_test")
                .unwrap()
                .writer
                .get()
                .is_none()
        );
    }
    assert!(
        dispatch(
            &service,
            &gateway,
            A_RUNNER,
            BrokerCommandV1::RecoveryInventory {
                after: None,
                limit: 20
            }
        )
        .await
        .is_err(),
        "a missing journal is an error, never a successful empty inventory"
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_inventory_discovers_blobless_stop_only_job_over_framed_ipc() {
    let gateway = Gateway::inactive(true).await;
    // A credential blob does not require a supervised Arm. Avoid combining a
    // fresh local Store's receipts with this external gateway's namespace.
    let mut fixture = Fixture::with_manual_binding_and_identity((&gateway.url, gateway.namespace));
    let operation = fixture.operation;
    let scope = fixture.writer.scope().clone();
    let before = fixture
        .writer
        .load(operation)
        .await
        .unwrap()
        .unwrap()
        .head()
        .clone();
    let blob = fixture.manual_blob.as_ref().unwrap();
    fs::rename(blob, fixture._temp.path().join("preserved-inventory-blob")).unwrap();
    assert!(fixture.writer.load(operation).await.is_err());
    fixture.service.profiles =
        BTreeMap::from([("cancel_retry_test".into(), Arc::new(ProfileJobs::new()))]);
    let service = Arc::new(fixture.service);
    drop((fixture.jobs, fixture.writer, fixture.journal));
    // Wait only for the old fixture queue's graceful lease release. No forced
    // unlock, replacement lease, runtime operation or repeated effect.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let result = dispatch(
            &service,
            &gateway,
            A_RUNNER,
            BrokerCommandV1::RecoveryStatus {
                operation_id: operation,
            },
        )
        .await;
        if result.is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fixture queue must release its lease"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        service
            .profile_jobs(scope.profile_id())
            .unwrap()
            .writer
            .get()
            .unwrap()
            .is_stop_only()
    );
    assert!(
        matches!(
            dispatch(
                &service,
                &gateway,
                A_RUNNER,
                BrokerCommandV1::List {
                    after: None,
                    limit: 20
                }
            )
            .await,
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        ),
        "business List is not a recovery alternative"
    );
    let (mut client, server) = UnixStream::pair().unwrap();
    let task = tokio::spawn(Arc::clone(&service).connection(server));
    write_frame(
        &mut client,
        &EnvelopeV1 {
            version: 1,
            profile_id: scope.profile_id().into(),
            credential: A_RUNNER.into(),
            request: BrokerCommandV1::RecoveryInventory {
                after: None,
                limit: 1,
            },
        },
        FRAME_BYTES,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    let response: ReplyEnvelopeV1 = read_frame(&mut client, FRAME_BYTES, Duration::from_secs(3))
        .await
        .unwrap();
    task.await.unwrap();
    let BrokerReplyV1::RecoveryInventory { inventory } = response.reply else {
        panic!("dedicated inventory reply");
    };
    assert_eq!(inventory.version, 1);
    assert_eq!(inventory.tenant_id, scope.tenant_id());
    assert_eq!(inventory.profile_id, scope.profile_id());
    assert_eq!(
        inventory.control_namespace_uuid,
        scope.control_namespace_uuid()
    );
    assert_eq!(inventory.source, RecoveryDataSourceV1::ConfirmedJournal);
    assert_eq!(
        inventory.runtime_evidence,
        RecoveryRuntimeEvidenceV1::NotQueried
    );
    assert_eq!(
        inventory.coverage,
        RecoveryInventoryCoverageV1::AcceptedStopMetadata
    );
    assert_eq!(inventory.journal_mode, JournalModeV1::StopOnly);
    assert!(inventory.incomplete && inventory.reconciliation_required);
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].operation_id, operation);
    assert_eq!(inventory.entries[0].head_revision, before.revision());
    assert_eq!(inventory.entries[0].head_hash, before.hash());
    assert!(inventory.entries[0].incomplete);
    assert_eq!(inventory.next_after, None);
    let encoded = serde_json::to_string(&inventory).unwrap();
    for forbidden in [
        A_RUNNER,
        "known_arms",
        "prompt",
        "container_id",
        "quota_held",
        "worker_state",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    let response = dispatch(
        &service,
        &gateway,
        A_RUNNER,
        BrokerCommandV1::RecoveryStatus {
            operation_id: inventory.entries[0].operation_id,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(response.reply, BrokerReplyV1::RecoveryStatus { status } if status.operation_id == operation)
    );
}
