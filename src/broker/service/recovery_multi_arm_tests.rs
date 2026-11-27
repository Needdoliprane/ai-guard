//! Retention of all proven Arms from one real job before a caller can vanish.
//! The native child exercises the real recovery handler; its runtime metadata
//! stays an unavailable fixture and proves no engine stop or worker deadline.
use super::*;
use crate::broker::journal::recovery::{RecoveredStopV1, StopRecoveryReportV1};

const SETUP_OBSERVATION_BUDGET: Duration = Duration::from_secs(60);

/// A failed/panicking/unfinished observer must not delete files still owned by
/// an accepted writer command. Only a validated terminal result releases this
/// guard back to the normal test fixture lifecycle.
struct ObservationFixture(Option<Fixture>);
impl ObservationFixture {
    fn fixture(&self) -> &Fixture {
        self.0.as_ref().expect("owned setup observation fixture")
    }
    fn into_fixture(mut self) -> Fixture {
        self.0.take().expect("validated setup observation fixture")
    }
}
impl Drop for ObservationFixture {
    fn drop(&mut self) {
        if let Some(Fixture {
            _temp,
            service,
            jobs,
            journal,
            writer,
            ..
        }) = self.0.take()
        {
            let _preserved_directory = _temp.keep();
            // Dropping senders closes the queue gracefully. Its own backend
            // keeps the exact Journal/lease until accepted commands drain.
            drop((service, jobs, journal, writer));
        }
    }
}
struct SetupObservation {
    fixture: ObservationFixture,
    result: crate::broker::journal::JournalResult<Option<CancelStopProjectionV1>>,
    elapsed: Duration,
}
// Deliberately retain ownership on a budget failure instead of joining an
// unbounded filesystem operation or dropping its result/fixture on a channel.
// This test-only registry is bounded by the two multi-Arm setup invocations.
static PRESERVED_SETUP_OBSERVERS: std::sync::Mutex<Vec<std::thread::JoinHandle<SetupObservation>>> =
    std::sync::Mutex::new(Vec::new());

struct TargetBinding {
    arm: String,
    registration_hash: String,
}

fn target_bindings(scope: &JournalScopeV1, stops: &[RecoveredStopV1]) -> Vec<TargetBinding> {
    assert_eq!(
        stops.len(),
        2,
        "the same confirmed job must expose two distinct Arms"
    );
    assert_ne!(stops[0].arm().operation_id, stops[1].arm().operation_id);
    assert_ne!(
        stops[0].arm().inspection.container_id,
        stops[1].arm().inspection.container_id
    );
    assert!(matches!(
        &stops[0].arm().subject,
        ArmSubjectV1::Helper {
            purpose: HelperPurposeV1::Import,
            ..
        }
    ));
    assert!(matches!(
        &stops[1].arm().subject,
        ArmSubjectV1::Helper {
            purpose: HelperPurposeV1::ImportVerification,
            ..
        }
    ));
    stops
        .iter()
        .map(|target| {
            target.receipt().validate_for_arm(target.arm()).unwrap();
            let (_, registration_hash) = recovery_registration(scope, target);
            TargetBinding {
                arm: target.arm().operation_id.clone(),
                registration_hash,
            }
        })
        .collect()
}

fn private_barrier() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

struct AbandonmentObservations {
    b_delivery_before_release: bool,
    a_cancel_before_release: Option<Uuid>,
    a_ack_epoch_before_release: Option<Uuid>,
    target_count_before_release: usize,
    a_delivery_after_release: bool,
    same_epoch: bool,
    release_was_absent: bool,
}

async fn wait_for_delivery_without_business_retry(
    client: &EnforcerStopClient,
    target: &TargetBinding,
    command: Uuid,
    epoch: Uuid,
) -> bool {
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < expires {
        if client.acknowledged_restore_epoch(&target.arm) == Some(epoch)
            && client
                .query_stop(&target.arm, &target.registration_hash)
                .await
                .is_ok_and(|slot| slot.cancel_command_id() == Some(command))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn abandon_waiter_while_a_is_blocked<T>(
    client: &EnforcerStopClient,
    root: &Directory,
    barrier: &Path,
    waiter: tokio::task::JoinHandle<T>,
    targets: &[TargetBinding],
    command: Uuid,
    epoch: Uuid,
) -> AbandonmentObservations {
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    let expected_marker = b"fixture-reserved\n";
    loop {
        assert!(
            !waiter.is_finished(),
            "the caller must reach the real post-Reserve barrier before failing"
        );
        assert!(
            std::time::Instant::now() < expires,
            "Arm A did not reach its authenticated post-Reserve barrier"
        );
        match fs::read(barrier.join("reserved")) {
            Ok(bytes) if bytes == expected_marker => break,
            Ok(bytes) if expected_marker.starts_with(&bytes) => {}
            Ok(_) => panic!("post-Reserve fixture marker contains divergent bytes"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => panic!("post-Reserve fixture marker cannot be read"),
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !waiter.is_finished(),
        "the abandoned waiter must still be waiting on Arm A"
    );
    waiter.abort();
    match waiter.await {
        Err(error) => assert!(
            error.is_cancelled(),
            "owned waiter panicked before explicit abort"
        ),
        Ok(_) => panic!("owned waiter completed before the abandonment barrier"),
    }
    // From this point onward only read-only observations are submitted. There
    // is no second deliver_projection/restore call to introduce Arm B.
    let b_delivery_before_release =
        wait_for_delivery_without_business_retry(client, &targets[1], command, epoch).await;
    // Restore installs the verified Cancel before replying. The client cache
    // records delivery only after validating the separate StopAccepted ACK.
    let a_cancel_before_release = client
        .query_stop(&targets[0].arm, &targets[0].registration_hash)
        .await
        .unwrap()
        .cancel_command_id();
    let target_count_before_release = client.status().await.unwrap().slot_count();
    let same_epoch_before_release = crate::broker::discovery::load_endpoint(root)
        .unwrap()
        .unwrap()
        .epoch()
        == epoch;
    let a_ack_epoch_before_release = client.acknowledged_restore_epoch(&targets[0].arm);
    let release_was_absent = !barrier.join("release").exists();
    // repeat_barrier prevents a timed-out A connection from permitting a later
    // A retry to bypass this gate while B's independent progress is observed.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(barrier.join("release"))
        .and_then(|mut file| std::io::Write::write_all(&mut file, b"fixture-release\n"))
        .unwrap();
    let a_delivery_after_release =
        wait_for_delivery_without_business_retry(client, &targets[0], command, epoch).await;
    AbandonmentObservations {
        b_delivery_before_release,
        a_cancel_before_release,
        a_ack_epoch_before_release,
        target_count_before_release,
        a_delivery_after_release,
        same_epoch: same_epoch_before_release && client.connection_epoch() == Some(epoch),
        release_was_absent,
    }
}

fn assert_independent_multi_arm_recovery(observed: AbandonmentObservations, command: Uuid) {
    assert!(
        observed.same_epoch,
        "the original authenticated guardian must remain alive"
    );
    assert!(
        observed.release_was_absent,
        "Arm A's gate must stay closed while observing B"
    );
    assert_eq!(
        observed.a_cancel_before_release,
        Some(command),
        "A must already retain the exact Cancel while its Restore reply is held"
    );
    assert_eq!(
        observed.a_ack_epoch_before_release, None,
        "A must still lack its Deliver ACK behind the barrier"
    );
    assert!(
        observed.b_delivery_before_release,
        "Arm B must have its exact Deliver ACK when observed after caller abort while A is still blocked"
    );
    assert_eq!(
        observed.target_count_before_release, 2,
        "one actor per original Arm must be reserved"
    );
    assert!(
        observed.a_delivery_after_release,
        "the already owned A operation must receive its exact Deliver ACK after its gate is released"
    );
}

async fn append_multi_arm_setup_once(
    fixture: Fixture,
    command: Uuid,
) -> (Fixture, CancelStopProjectionV1) {
    let started = std::time::Instant::now();
    let result = fixture
        .writer
        .stops()
        .append_cancel(CancelInputV1::new(fixture.operation, command).unwrap())
        .await;
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(projection) => (fixture, projection),
        Err(CompletionFailure::Unknown {
            error: crate::broker::journal::JournalError::Io,
        }) => observe_unknown_setup_once(fixture, command, elapsed_ms).await,
        Err(failure) => {
            let error = failure.error();
            let _preserve = ObservationFixture(Some(fixture));
            panic!(
                "multi-Arm setup append_cancel failed: error={error:?}; append_elapsed_ms={elapsed_ms}"
            );
        }
    }
}

fn validate_confirmed_setup(
    fixture: &Fixture,
    command: Uuid,
    projection: &CancelStopProjectionV1,
) -> Result<(), &'static str> {
    let expected_scope =
        canonical(fixture.journal.scope()).map_err(|_| "expected_scope_invalid")?;
    let observed_scope = canonical(projection.scope()).map_err(|_| "observed_scope_invalid")?;
    if expected_scope != observed_scope {
        return Err("scope_mismatch");
    }
    if projection.operation_id() != fixture.operation
        || projection.command_id() != command
        || projection.receipt().command_id() != command
    {
        return Err("command_identity_mismatch");
    }
    if projection.incomplete() || projection.stops().len() != 2 {
        return Err("incomplete_multi_arm_projection");
    }
    let intent = &projection.stops()[0].arm().intent_hash;
    let expected = crate::broker::lifecycle::LocalCommandV1::new(
        fixture.operation,
        intent,
        command,
        crate::broker::lifecycle::LocalCommandRequestV1::CancelRequested,
    )
    .map_err(|_| "expected_cancel_identity_invalid")?;
    if projection.receipt().input_hash() != expected.input_hash() {
        return Err("cancel_input_hash_mismatch");
    }
    for (index, stop) in projection.stops().iter().enumerate() {
        if &stop.arm().intent_hash != intent
            || stop.cancel_after_arm() != Some(command)
            || stop.receipt().validate_for_arm(stop.arm()).is_err()
        {
            return Err("arm_binding_mismatch");
        }
        let prefix = projection
            .cancel_prefix_for_stop(index)
            .ok_or("verified_prefix_missing")?;
        if !prefix.receipts().iter().any(|receipt| {
            receipt.command_id() == command
                && receipt.input_hash() == projection.receipt().input_hash()
                && receipt.head() == projection.receipt().head()
        }) {
            return Err("receipt_not_in_verified_prefix");
        }
    }
    Ok(())
}

async fn observe_unknown_setup_once(
    fixture: Fixture,
    command: Uuid,
    append_elapsed_ms: u128,
) -> (Fixture, CancelStopProjectionV1) {
    let owned = ObservationFixture(Some(fixture));
    let started = std::time::Instant::now();
    let worker = std::thread::Builder::new()
        .name("multi-arm-setup-observer".into())
        .spawn(move || {
            let operation = owned.fixture().operation;
            // One direct confirmed read, after the same writer mutex becomes
            // available. Never enqueue/replay the mutating Cancel command.
            let result = owned
                .fixture()
                .journal
                .cancel_stop_projection(operation, command)
                .and_then(|projection| {
                    if let Some(projection) = &projection {
                        projection.revalidate()?;
                    }
                    Ok(projection)
                });
            SetupObservation {
                fixture: owned,
                result,
                elapsed: started.elapsed(),
            }
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(_) => panic!(
            "multi-Arm setup observation failed: observation_spawn_failed; append_error=Io; append_elapsed_ms={append_elapsed_ms}"
        ),
    };
    loop {
        if worker.is_finished() {
            break;
        }
        if started.elapsed() >= SETUP_OBSERVATION_BUDGET {
            // is_finished is sampled explicitly before conservative failure.
            // A later completion does not turn this failed setup into PASS.
            let worker_nonterminal = !worker.is_finished();
            if !worker_nonterminal {
                break;
            }
            PRESERVED_SETUP_OBSERVERS
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(worker);
            panic!(
                "multi-Arm setup observation failed: worker_nonterminal=true; append_error=Io; append_elapsed_ms={append_elapsed_ms}; observation_budget_ms={}",
                SETUP_OBSERVATION_BUDGET.as_millis()
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let observed = match worker.join() {
        Ok(observed) => observed,
        Err(_) => panic!(
            "multi-Arm setup observation failed: worker_panicked; append_error=Io; append_elapsed_ms={append_elapsed_ms}"
        ),
    };
    if observed.elapsed > SETUP_OBSERVATION_BUDGET {
        panic!(
            "multi-Arm setup observation failed: terminal_after_budget; append_error=Io; append_elapsed_ms={append_elapsed_ms}; observation_elapsed_ms={}; observation_budget_ms={}",
            observed.elapsed.as_millis(),
            SETUP_OBSERVATION_BUDGET.as_millis()
        );
    }
    let projection = match observed.result {
        Ok(Some(projection)) => projection,
        Ok(None) => panic!(
            "multi-Arm setup observation failed: confirmed_cancel_absent; append_error=Io; append_elapsed_ms={append_elapsed_ms}; observation_elapsed_ms={}",
            observed.elapsed.as_millis()
        ),
        Err(error) => panic!(
            "multi-Arm setup observation failed: confirming_read_error={error:?}; append_error=Io; append_elapsed_ms={append_elapsed_ms}; observation_elapsed_ms={}",
            observed.elapsed.as_millis()
        ),
    };
    if let Err(reason) = validate_confirmed_setup(observed.fixture.fixture(), command, &projection)
    {
        panic!(
            "multi-Arm setup observation failed: {reason}; append_error=Io; append_elapsed_ms={append_elapsed_ms}; observation_elapsed_ms={}",
            observed.elapsed.as_millis()
        );
    }
    eprintln!(
        "multi-Arm setup confirmed by one exact read: append_error=Io; append_elapsed_ms={append_elapsed_ms}; observation_elapsed_ms={}; observation_budget_ms={}",
        observed.elapsed.as_millis(),
        SETUP_OBSERVATION_BUDGET.as_millis()
    );
    (observed.fixture.into_fixture(), projection)
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_projection_retains_all_job_arms_before_an_abandoned_waiter() {
    let fixture = Fixture::with_manual_arm_count(2);
    let command = Uuid::new_v4();
    let (fixture, projection) = append_multi_arm_setup_once(fixture, command).await;
    let projection = Arc::new(projection);
    assert_eq!(projection.operation_id(), fixture.operation);
    let targets = target_bindings(projection.scope(), projection.stops());
    assert!(
        projection
            .stops()
            .iter()
            .all(|target| target.cancel_after_arm() == Some(command))
    );
    let (_role_temp, root) = recovery_role_fixture();
    let barrier = private_barrier();
    let barrier_path = barrier.path().canonicalize().unwrap();
    let child = recovery_child_with_faults(
        &root,
        None,
        &RecoveryFaults {
            barrier: Some((&targets[0].arm, &barrier_path)),
            repeat_barrier: true,
            ..RecoveryFaults::default()
        },
    );
    let client = warmed_recovery_client(&root).await;
    let epoch = client.observed_watch_epoch().unwrap();
    let caller = client.clone();
    let waiter = tokio::spawn(async move { caller.deliver_projection(projection, 1).await });
    let observed = abandon_waiter_while_a_is_blocked(
        &client,
        &root,
        &barrier_path,
        waiter,
        &targets,
        command,
        epoch,
    )
    .await;
    // Queries above recouple the Arm ID to the exact registration digest; that
    // digest includes the original CID, authority, receipt and Arm deadline.
    drop(client);
    drop(child);
    assert_independent_multi_arm_recovery(observed, command);
}

async fn close_fixture_and_open_report(
    fixture: Fixture,
) -> (tempfile::TempDir, Arc<StopRecoveryReportV1>, Uuid) {
    let Fixture {
        _temp,
        service,
        jobs,
        journal,
        writer,
        operation,
        ..
    } = fixture;
    let scope = writer.scope().clone();
    let root = service.root.clone();
    let material = Arc::clone(&service.material);
    // Close only the fixture's old Full owners. The report must acquire its
    // actual existing lease after that writer drains, without unlock/replace.
    drop((service, jobs, journal, writer));
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match super::super::recover_existing_profile(&root, scope.clone(), &material) {
            Ok((Some(report), false)) => return (_temp, report, operation),
            result if std::time::Instant::now() < expires => {
                drop(result);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => panic!("fixture's confirmed report could not reopen under the existing lease"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_report_job_retains_all_arms_before_an_abandoned_bootstrap_waiter() {
    let fixture = Fixture::with_manual_arm_count(2);
    let command = Uuid::new_v4();
    let (fixture, projection) = append_multi_arm_setup_once(fixture, command).await;
    assert_eq!(projection.stops().len(), 2);
    drop(projection);
    let (_journal_temp, report, operation) = close_fixture_and_open_report(fixture).await;
    assert!(
        !report.incomplete(),
        "the test needs a complete verified journal report"
    );
    assert_eq!(report.jobs().len(), 1);
    let recovered = &report.jobs()[0];
    assert_eq!(recovered.operation_id(), operation);
    let targets = target_bindings(report.scope(), recovered.stops());
    assert!(
        recovered
            .stops()
            .iter()
            .all(|target| target.cancel_after_arm() == Some(command))
    );
    let jobs = ProfileJobs::new();
    let job = jobs.job(operation).unwrap();
    job.remember_cancel_ids(recovered.cancel_command_ids())
        .unwrap();
    let (_role_temp, root) = recovery_role_fixture();
    let barrier = private_barrier();
    let barrier_path = barrier.path().canonicalize().unwrap();
    let child = recovery_child_with_faults(
        &root,
        None,
        &RecoveryFaults {
            barrier: Some((&targets[0].arm, &barrier_path)),
            repeat_barrier: true,
            ..RecoveryFaults::default()
        },
    );
    let client = warmed_recovery_client(&root).await;
    let epoch = client.observed_watch_epoch().unwrap();
    let caller = client.clone();
    let proof = Arc::clone(&report);
    let waiter = tokio::spawn(async move {
        super::super::restore_recovered_job(None, Some(caller), proof, 0, job).await
    });
    let observed = abandon_waiter_while_a_is_blocked(
        &client,
        &root,
        &barrier_path,
        waiter,
        &targets,
        command,
        epoch,
    )
    .await;
    drop(client);
    drop(child);
    assert_independent_multi_arm_recovery(observed, command);
}

#[path = "d5_ledger_tests.rs"]
mod d5_ledger_tests;

#[path = "d7_recovery_capacity_tests.rs"]
mod d7_recovery_capacity_tests;
