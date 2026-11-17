//! SOURCE ONLY / NOT_RUN. Current real journal + real native recovery transport.
//! The unavailable runtime fixture proves no container stop or deadline run-on.
//! These are inverted regressions, not the historical print-only audit probes.
use super::*;
use crate::broker::enforcer::EnforcerError;

const ROOT17: &str = "AI_GUARD_TEST_D7_RECOVERY_ROOT";
const SLOTS17: &str = "AI_GUARD_TEST_D7_RECOVERY_SLOTS";
const OWNERS17: &str = "AI_GUARD_TEST_D7_RECOVERY_OWNERS";

#[test]
#[ignore = "owned native recovery fixture child; only its parent may invoke it"]
fn d7_native_recovery_capacity_child() {
    let path = std::env::var_os(ROOT17).expect("private recovery root");
    let slots = match std::env::var(SLOTS17).as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => panic!("capacity witness must use exactly one or two slots"),
    };
    let owners: u32 = std::env::var(OWNERS17).unwrap().parse().unwrap();
    assert!(owners <= 2);
    let root = Directory::open_existing(Path::new(&path)).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(
            crate::broker::enforcer::run_internal_recovery_enforcer_with_budget(
                &root, slots, owners,
            ),
        )
        .unwrap();
}

// Four named parents can each retain at most one owned child and one setup
// observer. A retained slot cannot be entered again and create new resources.
#[derive(Default)]
struct NativeWork17 {
    child: Option<std::process::Child>,
    setup_observer: Option<std::thread::JoinHandle<SetupObservation>>,
    retained_report: Option<Arc<StopRecoveryReportV1>>,
    child_pid: Option<u32>,
    observed_exit: Option<std::process::ExitStatus>,
    kill_error: Option<std::io::Error>,
    wait_error: Option<std::io::Error>,
    stage: &'static str,
    first_cause: Option<String>,
    last_cause: Option<String>,
    role_path: Option<PathBuf>,
    journal_path: Option<PathBuf>,
}
enum NativeCase17 {
    Vacant,
    Running,
    Retained(Box<NativeWork17>),
}
static NATIVE_CASES17: std::sync::Mutex<[NativeCase17; 4]> = std::sync::Mutex::new([
    NativeCase17::Vacant,
    NativeCase17::Vacant,
    NativeCase17::Vacant,
    NativeCase17::Vacant,
]);
struct NativeGuard17 {
    index: usize,
    work: Option<Box<NativeWork17>>,
}
impl NativeGuard17 {
    fn new(index: usize) -> Self {
        let mut slots = NATIVE_CASES17.lock().unwrap_or_else(|p| p.into_inner());
        match &slots[index] {
            NativeCase17::Vacant => {}
            NativeCase17::Running => panic!("D7 case already has a running owner"),
            NativeCase17::Retained(work) => panic!(
                "D7 case retains native ownership: pid={:?} stage={} first={:?} last={:?}; do not repeat",
                work.child_pid, work.stage, work.first_cause, work.last_cause
            ),
        }
        slots[index] = NativeCase17::Running;
        Self {
            index,
            work: Some(Box::new(NativeWork17 {
                stage: "before fixture",
                ..Default::default()
            })),
        }
    }
    fn work(&mut self) -> &mut NativeWork17 {
        self.work.as_mut().expect("owned D7 case slot")
    }
    fn cause(&mut self, cause: String) {
        let work = self.work();
        if work.first_cause.is_none() {
            work.first_cause = Some(cause.clone());
        }
        work.last_cause = Some(cause);
    }
    fn spawn_child(&mut self, root: &Directory, slots: u32, owners: u32) {
        assert!(slots == 1 || slots == 2);
        self.work().role_path = Some(root.path().into());
        self.work().stage = "spawn attributed native child";
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "broker::service::server::cancel_retry_tests::recovery_multi_arm_tests::d7_recovery_capacity_tests::d7_native_recovery_capacity_child",
            "--ignored",
        ]).env_clear().env(ROOT17, root.path()).env(SLOTS17, slots.to_string()).env(OWNERS17, owners.to_string())
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match command.spawn() {
            Ok(child) => {
                self.work().child_pid = Some(child.id());
                // Install the actual handle before any wait or endpoint read.
                self.work().child = Some(child);
            }
            Err(error) => {
                self.cause(format!("spawn: {error}"));
                panic!("D7 child spawn failed");
            }
        }
    }
    async fn observe_endpoint(&mut self, root: &Directory) {
        self.work().stage = "observe owned child endpoint";
        let expires = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match self.work().child.as_mut().unwrap().try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    self.work().observed_exit = Some(status);
                    self.cause(format!("owned child exited before endpoint: {status}"));
                    panic!("D7 endpoint prerequisite failed");
                }
                Err(error) => {
                    self.cause(format!("endpoint try_wait: {error}"));
                    self.work().wait_error = Some(error);
                    panic!("D7 child status is unknown");
                }
            }
            if crate::broker::discovery::load_endpoint(root)
                .unwrap()
                .is_some()
            {
                return;
            }
            assert!(
                std::time::Instant::now() < expires,
                "owned D7 endpoint prerequisite failed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn close_child_observed(&mut self) {
        self.work().stage = "observe attributed child termination";
        let first = self.work().child.as_mut().unwrap().try_wait();
        let already_exited = match first {
            Ok(Some(status)) => Some(status),
            Ok(None) => None,
            Err(error) => {
                self.cause(format!("termination try_wait: {error}"));
                self.work().wait_error = Some(error);
                panic!("D7 termination status unknown; child/root retained");
            }
        };
        if let Some(status) = already_exited {
            self.work().observed_exit = Some(status);
        } else {
            // Only the still-owned Child handle from spawn is addressed. No
            // process search, reused PID input or unrelated signal is accepted.
            if let Err(error) = self.work().child.as_mut().unwrap().kill() {
                self.cause(format!("attributed child kill: {error}"));
                self.work().kill_error = Some(error);
            }
            let expires = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match self.work().child.as_mut().unwrap().try_wait() {
                    Ok(Some(status)) => {
                        self.work().observed_exit = Some(status);
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.cause(format!("termination wait: {error}"));
                        self.work().wait_error = Some(error);
                        panic!("D7 child wait failed; handle/root retained");
                    }
                }
                if std::time::Instant::now() >= expires {
                    self.cause(
                        "owned child still has no ExitStatus after termination observation budget"
                            .into(),
                    );
                    panic!("D7 child termination unproved; handle/root retained");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        // A failed kill can race an already-exited child. A real ExitStatus,
        // obtained by try_wait on THIS handle, is the required positive fact.
        assert!(self.work().observed_exit.is_some());
        let work = self.work();
        eprintln!(
            "D7 owned child termination observed: pid={:?} status={:?} kill_error={:?}",
            work.child_pid, work.observed_exit, work.kill_error
        );
        drop(self.work().child.take()); // Already reaped, not Drop-as-join.
    }
    fn complete(mut self) {
        let work = self.work.as_ref().unwrap();
        assert!(
            work.child.is_none()
                && work.setup_observer.is_none()
                && work.retained_report.is_none()
                && work.observed_exit.is_some()
        );
        drop(self.work.take());
        let mut slots = NATIVE_CASES17.lock().unwrap_or_else(|p| p.into_inner());
        slots[self.index] = NativeCase17::Vacant;
    }
}
impl Drop for NativeGuard17 {
    fn drop(&mut self) {
        if let Some(work) = self.work.take() {
            // No kill/wait or file release in Drop. Preserve native handles,
            // original cause and fixture paths even on cancellation/unwind.
            let diagnostic = format!(
                "D7 harness resources retained: case={} pid={:?} stage={} role={:?} journal={:?} first={:?} last={:?}\n",
                self.index,
                work.child_pid,
                work.stage,
                work.role_path,
                work.journal_path,
                work.first_cause,
                work.last_cause
            );
            let mut slots = NATIVE_CASES17.lock().unwrap_or_else(|p| p.into_inner());
            slots[self.index] = NativeCase17::Retained(work);
            drop(slots);
            // A failed diagnostic write cannot drop the retained owners.
            let _ = std::io::Write::write_all(&mut std::io::stderr().lock(), diagnostic.as_bytes());
        }
    }
}

async fn setup_guarded17(
    owned: ObservationFixture,
    command: Uuid,
    native: &mut NativeGuard17,
) -> (ObservationFixture, CancelStopProjectionV1) {
    // The future owns the preserving guard even if it is dropped before its
    // first poll. No plain Fixture is passed across a fallible await.
    native.work().stage = "append original Cancel once with guarded fixture";
    let started = std::time::Instant::now();
    let result = owned
        .fixture()
        .writer
        .stops()
        .append_cancel(CancelInputV1::new(owned.fixture().operation, command).unwrap())
        .await;
    match result {
        Ok(projection) => {
            validate_confirmed_setup(owned.fixture(), command, &projection).unwrap();
            (owned, projection)
        }
        Err(CompletionFailure::Unknown {
            error: crate::broker::journal::JournalError::Io,
        }) => {
            native.cause(format!(
                "append_cancel original Unknown(Io); elapsed_ms={}",
                started.elapsed().as_millis()
            ));
            native.work().stage = "one confirming setup read, no mutation retry";
            let observed_started = std::time::Instant::now();
            let worker = std::thread::Builder::new()
                .name("d7-setup-observer".into())
                .spawn(move || {
                    let operation = owned.fixture().operation;
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
                        elapsed: observed_started.elapsed(),
                    }
                });
            let worker = match worker {
                Ok(worker) => worker,
                Err(error) => {
                    native.cause(format!("setup observer spawn: {error}"));
                    panic!("D7 confirming read not started");
                }
            };
            native.work().setup_observer = Some(worker);
            loop {
                if native.work().setup_observer.as_ref().unwrap().is_finished() {
                    break;
                }
                if observed_started.elapsed() >= SETUP_OBSERVATION_BUDGET {
                    native.cause(
                        "setup observer not terminal at unchanged observation budget".into(),
                    );
                    panic!("D7 setup remains unknown; same observer and fixture retained");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let observed = match native.work().setup_observer.take().unwrap().join() {
                Ok(observed) => observed,
                Err(_) => {
                    native.cause("setup observer joined with panic; original fixture guard preserved its path".into());
                    panic!("D7 confirming read panicked");
                }
            };
            // The returned preserving guard is installed before elapsed/result
            // checks that may refuse. It owns the SAME original fixture.
            let SetupObservation {
                fixture: owned,
                result,
                elapsed,
            } = observed;
            assert!(
                elapsed <= SETUP_OBSERVATION_BUDGET,
                "D7 setup completed after its budget"
            );
            let projection = match result {
                Ok(Some(projection)) => projection,
                Ok(None) => {
                    native.cause("same journal confirmed no Cancel projection".into());
                    panic!("D7 setup projection absent");
                }
                Err(error) => {
                    native.cause(format!("same journal confirming read: {error:?}"));
                    panic!("D7 confirming read failed");
                }
            };
            validate_confirmed_setup(owned.fixture(), command, &projection).unwrap();
            (owned, projection)
        }
        Err(failure) => {
            native.cause(format!(
                "original append_cancel failure: {:?}",
                failure.error()
            ));
            panic!("D7 setup append failed; original fixture retained");
        }
    }
}

async fn close_guarded_report17(
    owned: ObservationFixture,
    native: &mut NativeGuard17,
) -> (RetainedTemp17, Arc<StopRecoveryReportV1>, Uuid) {
    // owned already preserves on cancellation before first poll. Between this
    // destructure and the guard below there is no fallible call or await.
    let Fixture {
        _temp,
        service,
        jobs,
        journal,
        writer,
        operation,
        ..
    } = owned.into_fixture();
    let temporary = RetainedTemp17(Some(_temp));
    native.work().stage = "close original senders, acquire same existing report lease";
    let scope = writer.scope().clone();
    let root = service.root.clone();
    let material = Arc::clone(&service.material);
    drop((service, jobs, journal, writer));
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match super::super::super::recover_existing_profile(&root, scope.clone(), &material) {
            Ok((Some(report), false)) => return (temporary, report, operation),
            Ok((Some(report), true)) => {
                native.work().retained_report = Some(report);
                native.cause(
                    "same existing report is incomplete; its acquired owner retained".into(),
                );
                panic!("D7 report prerequisite incomplete");
            }
            Ok((None, partial)) => {
                native.cause(format!(
                    "same journal returned no report; partial={partial}"
                ));
                panic!("D7 report prerequisite absent");
            }
            Err(error) => {
                native.cause(format!("same existing report lease/read: {error:?}"));
                if std::time::Instant::now() >= expires {
                    panic!(
                        "D7 existing lease not reacquired in unchanged3s budget; files/backend retained"
                    );
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn client17(root: &Directory, slots: u32, owners: u32) -> EnforcerStopClient {
    let client = EnforcerStopClient::connect_or_spawn_for_recovery_with_budget(
        root,
        &std::env::current_exe().unwrap(),
        slots,
        owners,
    )
    .await
    .unwrap();
    let epoch = crate::broker::discovery::load_endpoint(root)
        .unwrap()
        .unwrap()
        .epoch();
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while client.observed_watch_epoch() != Some(epoch) {
        assert!(
            std::time::Instant::now() < expires,
            "current Watch epoch not observed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client
}

// A failed observer preserves its fixture files while any retained proof/client
// can still own their native anchors. Weak extinction is not a guardian join.
struct RetainedTemp17(Option<tempfile::TempDir>);
impl Drop for RetainedTemp17 {
    fn drop(&mut self) {
        if let Some(temporary) = self.0.take() {
            let _preserved = temporary.keep();
        }
    }
}
impl RetainedTemp17 {
    fn observed(mut self) {
        drop(self.0.take());
    }
}

#[derive(Debug)]
struct ArmObservation17 {
    cancel_observed: bool,
    query_conflict: bool,
}
#[derive(Debug)]
struct Observation17 {
    transport_call_ok: bool,
    reported_confirmations: Option<usize>,
    partial: Option<bool>,
    capacity_error: bool,
    native_slot_count: usize,
    arms: Vec<ArmObservation17>,
    owned_active: usize,
    recovery_incomplete: bool,
    recovery_attempts: u64,
    recovery_attempt_failures: u64,
}

async fn observe17(slots: u32, owners: u32, cancel_projection: bool) -> Observation17 {
    assert!(
        EnforcerStopClient::take_created_clients_for_test().is_extinct(),
        "D7 test thread already has an unrelated live STOP client"
    );
    let index = match (slots, owners, cancel_projection) {
        (1, 2, false) => 0,
        (2, 0, false) => 1,
        (1, 2, true) => 2,
        (1, 1, false) => 3,
        _ => panic!("unknown D7 parent case"),
    };
    let mut native = NativeGuard17::new(index);
    let fixture = ObservationFixture(Some(Fixture::with_manual_arm_count(2)));
    native.work().journal_path = Some(fixture.fixture()._temp.path().into());
    let command = Uuid::new_v4();
    let (fixture, projection) = setup_guarded17(fixture, command, &mut native).await;
    let (fixture_owner, report_owner, projection_owner, targets, job) = if cancel_projection {
        let targets = target_bindings(projection.scope(), projection.stops());
        assert!(
            projection
                .stops()
                .iter()
                .all(|target| target.retirement().is_none())
        );
        (
            Some(fixture),
            None,
            Some(Arc::new(projection)),
            targets,
            None,
        )
    } else {
        drop(projection);
        let (temporary, report, operation) = close_guarded_report17(fixture, &mut native).await;
        assert!(!report.incomplete());
        assert_eq!(report.jobs().len(), 1);
        let recovered = &report.jobs()[0];
        assert_eq!(recovered.operation_id(), operation);
        assert!(recovered.retirements().is_empty());
        assert!(
            recovered
                .stops()
                .iter()
                .all(|target| target.retirement().is_none())
        );
        let targets = target_bindings(report.scope(), recovered.stops());
        let jobs = ProfileJobs::new();
        let job = jobs.job(operation).unwrap();
        job.remember_cancel_ids(recovered.cancel_command_ids())
            .unwrap();
        (None, Some((temporary, report)), None, targets, Some(job))
    };
    let (temporary, root) = recovery_role_fixture();
    let role_temp = RetainedTemp17(Some(temporary));
    // Uses only the sandbox's existing TMPDIR; a long UDS path is an explicit
    // prerequisite refusal, never a re-root into another writable namespace.
    native.spawn_child(&root, slots, owners);
    native.observe_endpoint(&root).await;
    let client = client17(&root, slots, owners).await;
    let client_owners = EnforcerStopClient::take_created_clients_for_test();
    let (transport_call_ok, reported_confirmations, partial, capacity_error) =
        if let Some(projection) = &projection_owner {
            let result = tokio::time::timeout(
                Duration::from_secs(20),
                client.deliver_projection(Arc::clone(projection), 1),
            )
            .await
            .expect("D7 projection observation did not finish within budget");
            (
                result.is_ok(),
                None,
                None,
                result == Err(EnforcerError::Capacity),
            )
        } else {
            let report = &report_owner.as_ref().unwrap().1;
            let (count, partial) = tokio::time::timeout(
                Duration::from_secs(20),
                super::super::super::restore_recovered_job(
                    None,
                    Some(client.clone()),
                    Arc::clone(report),
                    0,
                    job.unwrap(),
                ),
            )
            .await
            .expect("D7 report observation did not finish within budget");
            (!partial, Some(count), Some(partial), false)
        };
    let mut arms = Vec::new();
    for target in &targets {
        let delivered = wait_for_cancel_without_business_retry(
            &client,
            &target.arm,
            &target.registration_hash,
            command,
        )
        .await;
        let query = client
            .query_stop(&target.arm, &target.registration_hash)
            .await;
        let query_conflict = matches!(&query, Err(EnforcerError::Conflict));
        assert!(
            query.is_ok() || query_conflict,
            "D7 query failed before a meaningful capacity observation"
        );
        let cancel_observed = delivered
            && query.as_ref().is_ok_and(|slot| {
                slot.arm_id() == target.arm
                    && slot.registration_hash() == target.registration_hash
                    && slot.cancel_command_id() == Some(command)
            });
        arms.push(ArmObservation17 {
            cancel_observed,
            query_conflict,
        });
    }
    let status = client.status().await.unwrap();
    assert!(status.transport_healthy());
    assert_eq!(
        serde_json::to_value(&status).unwrap()["max_slots"],
        json!(slots),
        "the fixture or product changed the requested native capacity"
    );
    assert_eq!(status.max_recovery_stop_owners(), owners);
    assert!(status.recovery_fenced());
    let native_slot_count = status.slot_count();
    assert!(
        native_slot_count <= (slots + owners.saturating_sub(1)) as usize,
        "D7 cannot be green by silently increasing S or W"
    );
    drop(client);
    let expires = std::time::Instant::now() + Duration::from_secs(5);
    while !client_owners.is_extinct() && std::time::Instant::now() < expires {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        client_owners.is_extinct(),
        "D7 observer still owns a client/proof; native fixture files are preserved"
    );
    native.close_child_observed().await;
    drop(projection_owner);
    // Projection setup still has its original writer. Reacquire its SAME lease
    // under a preserving guard before deleting that fixture, too.
    let report_owner = if let Some(fixture) = fixture_owner {
        let (temporary, report, _) = close_guarded_report17(fixture, &mut native).await;
        Some((temporary, report))
    } else {
        report_owner
    };
    if let Some((temporary, report)) = report_owner {
        drop(report);
        temporary.observed();
    }
    assert!(native.work().observed_exit.is_some() && native.work().child.is_none());
    drop(root);
    role_temp.observed();
    native.complete();
    Observation17 {
        transport_call_ok,
        reported_confirmations,
        partial,
        capacity_error,
        native_slot_count,
        arms,
        owned_active: status.owned_active(),
        recovery_incomplete: status.recovery_incomplete(),
        recovery_attempts: status.recovery_attempts(),
        recovery_attempt_failures: status.recovery_attempt_failures(),
    }
}

fn require_coverage17(observed: Observation17) {
    // A Conflict and missing Cancel are separately observable facts on the
    // CURRENT baseline. They are not an agreed future overflow architecture.
    // This unavailable-engine fixture cannot produce a genuine stop/retirement
    // proof. If a later design adds such a proof, extend the oracle using that
    // real authority rather than demanding two simultaneous table entries.
    assert_eq!(observed.arms.len(), 2);
    assert_eq!(observed.owned_active, 2);
    assert!(!observed.recovery_incomplete);
    assert!(
        observed.arms.iter().all(|arm| arm.cancel_observed),
        "an ACK remains without confirmed STOP/Cancel coverage: {observed:?}"
    );
    assert!(
        observed.transport_call_ok,
        "recovery still reports lost coverage: {observed:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn d7_one_slot_report_leaves_no_ack_without_stop_cancel_coverage() {
    require_coverage17(observe17(1, 2, false).await);
}
#[tokio::test(flavor = "current_thread")]
async fn d7_two_slot_report_is_a_separate_capacity_control() {
    let observed = observe17(2, 0, false).await;
    assert_eq!(observed.reported_confirmations, Some(2));
    assert_eq!(observed.partial, Some(false));
    assert_eq!(observed.native_slot_count, 2);
    assert!(!observed.capacity_error && observed.arms.iter().all(|arm| !arm.query_conflict));
    require_coverage17(observed);
}
#[tokio::test(flavor = "current_thread")]
async fn d7_one_slot_cancel_projection_leaves_no_ack_without_coverage() {
    require_coverage17(observe17(1, 2, true).await);
}

#[tokio::test(flavor = "current_thread")]
async fn d7_reserved_catchup_attempts_surplus_without_active_coverage_ack() {
    let observed = observe17(1, 1, false).await;
    assert_eq!(observed.native_slot_count, 1);
    assert_eq!(observed.owned_active, 1);
    assert_eq!(observed.reported_confirmations, Some(1));
    assert_eq!(observed.partial, Some(true));
    assert!(observed.recovery_incomplete);
    assert_eq!(
        observed
            .arms
            .iter()
            .filter(|arm| arm.cancel_observed)
            .count(),
        1
    );
    assert_eq!(
        observed
            .arms
            .iter()
            .filter(|arm| arm.query_conflict)
            .count(),
        1
    );
    // This fixture's runtime is deliberately unavailable: reopening is a real
    // attempted call and unknown/error, never an observed stopped container.
    assert!(
        observed.recovery_attempts > 0,
        "surplus was only queued: {observed:?}"
    );
    assert!(
        observed.recovery_attempt_failures > 0,
        "unavailable runtime must remain unknown: {observed:?}"
    );
}
