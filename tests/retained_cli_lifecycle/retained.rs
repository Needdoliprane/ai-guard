//! L5–L16 and L1b: what the retained owner itself reports and keeps, read from
//! its own ends: refusal before start, forwarded interruption, escalation and
//! cancellation during the grace, unknown termination and what stays after it,
//! a dropped observer, the hold that keeps a command scope from abandoning a
//! command, a failed check after collection, an owner that fails, a wait
//! status taken by something else, and the bound of the captured output.
//!
//! The oracles are those of L0–L4: a probe answered by the process itself, the
//! liveness lock the kernel releases, and the release witness the material
//! writes when it is dropped. No test holds a handle on the material; it is
//! read back by pathname.
use crate::{
    adapter::{self, RETAINED, Turn},
    harness::{
        Consumer, ENGINE_STDOUT, Liveness, MATERIAL_FILES, Material, Product, ROLES,
        fixture_binary, missed_ready,
    },
};
use agentsandbox::runtime::{
    Runtime,
    retained_cli::{
        INTERRUPT_GRACE, Interruption, InvocationEnd, InvocationFailure, InvocationStopped,
        RetainedInput, RuntimeInvocationOwner, STDOUT_LIMIT, STOP_OBSERVATION, Stop, StopReason,
        UnknownEnd,
    },
};
use std::{
    future::Future,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::PoisonError,
    time::{Duration, Instant, SystemTime},
};

/// At most one scenario of this module waits for the binary's turn.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Test-side observation windows. They are not product deadlines.
const SLOT_WINDOW: Duration = Duration::from_secs(60);
const READY_WINDOW: Duration = Duration::from_secs(30);
const RESOLVE_WINDOW: Duration = Duration::from_secs(30);
const PROBE_WINDOW: Duration = Duration::from_secs(3);
const END_WINDOW: Duration = Duration::from_secs(10);
const POLL_ONCE: Duration = Duration::from_millis(1);
/// Bound handed to a command that the scenario ends in another way.
const LONG: Duration = Duration::from_secs(120);

type Check = Box<dyn Fn() -> anyhow::Result<()> + Send>;
type Record = Box<dyn Fn(&UnknownEnd) -> Option<PathBuf> + Send>;

/// The material with the test's own behaviour at the owner's three call sites.
#[derive(Default)]
struct Hooks {
    before_spawn: Option<Check>,
    after_collection: Option<Check>,
    on_unknown: Option<Record>,
}

struct Hooked {
    material: Material,
    hooks: Hooks,
}

impl RetainedInput for Hooked {
    fn arguments(&self) -> Vec<String> {
        self.material.arguments()
    }
    fn verify_before_spawn(&self) -> anyhow::Result<()> {
        RetainedInput::verify_before_spawn(&self.material)?;
        self.hooks
            .before_spawn
            .as_ref()
            .map_or(Ok(()), |hook| hook())
    }
    fn verify_after_collection(&self) -> anyhow::Result<()> {
        self.hooks
            .after_collection
            .as_ref()
            .map_or(Ok(()), |hook| hook())
    }
    fn termination_unknown(&self, end: &UnknownEnd) -> Option<PathBuf> {
        self.hooks.on_unknown.as_ref().and_then(|hook| hook(end))
    }
}

struct Scene {
    runtime: Runtime,
    started: Instant,
    started_at: SystemTime,
    consumer: Consumer,
    // Declared last: the fixture processes are told to quit before the turn
    // passes to the next scenario.
    _turn: Turn,
}

impl Scene {
    async fn open(directives: &[(&str, &str)]) -> (Self, Material) {
        eprintln!("consumer under test: retained owner = {RETAINED}");
        let (consumer, material) = Consumer::prepare(directives);
        let runtime = consumer.runtime();
        let turn = adapter::admit(&runtime, SLOT_WINDOW)
            .await
            .unwrap_or_else(|busy| {
                panic!("SETUP: retained slot still busy since the previous test: {busy}")
            });
        let scene = Self {
            runtime,
            started: Instant::now(),
            started_at: SystemTime::now(),
            consumer,
            _turn: turn,
        };
        (scene, material)
    }

    /// Hands the input over whole. The test keeps the material's path only.
    fn start(
        &self,
        input: impl RetainedInput,
        duration: Duration,
        interruption: Option<&Interruption>,
    ) -> RuntimeInvocationOwner {
        self.runtime.cli_env_retained(
            Vec::new(),
            Box::new(input),
            duration,
            &[],
            interruption.cloned(),
        )
    }

    /// Requires `ready-<role>` for every role and one answered probe of the last.
    async fn establish(&self, owner: &mut RuntimeInvocationOwner, roles: &[&str]) {
        let mut missing = None;
        for role in roles {
            match self.consumer.state.wait_ready(role, READY_WINDOW) {
                Some(_) => eprintln!(
                    "latency: start to ready-{role} {:?}",
                    self.started.elapsed()
                ),
                None => {
                    missing = Some(*role);
                    break;
                }
            }
        }
        let last = *roles.last().expect("a scenario has a role");
        if missing.is_none() && self.consumer.state.probe(last, PROBE_WINDOW) {
            return;
        }
        let product = match tokio::time::timeout(POLL_ONCE, owner.join()).await {
            Ok(end) => Product::Ended(format!("{end:?}")),
            Err(_) => Product::Pending,
        };
        panic!(
            "{}",
            missed_ready(
                &self.consumer.state,
                missing.unwrap_or(last),
                "build",
                self.started_at,
                self.started.elapsed(),
                &product
            )
        );
    }

    /// The next end the observer is told, within a test-side window.
    async fn end(&self, owner: &mut RuntimeInvocationOwner, window: Duration) -> InvocationEnd {
        match tokio::time::timeout(window, owner.join()).await {
            Ok(end) => {
                eprintln!(
                    "latency: start to end {:?}: {end:?}",
                    self.started.elapsed()
                );
                end
            }
            Err(_) => {
                let alive: Vec<&str> = ROLES
                    .into_iter()
                    .filter(|role| self.consumer.state.liveness(role) == Liveness::Alive)
                    .collect();
                if alive.is_empty() {
                    panic!(
                        "FAIL: every consumer is proven ended but no end was reported after \
                         {window:?}; {}",
                        self.consumer.state.describe()
                    );
                }
                panic!(
                    "UNKNOWN: no end was reported after {window:?}; alive: {alive:?}; {}",
                    self.consumer.state.describe()
                );
            }
        }
    }

    /// No end is reported during `window`.
    async fn pending(&self, owner: &mut RuntimeInvocationOwner, window: Duration) {
        if let Ok(end) = tokio::time::timeout(window, owner.join()).await {
            panic!(
                "FAIL: an end was reported {:?} after the start, while the scenario still runs: \
                 {end:?}",
                self.started.elapsed()
            );
        }
    }

    fn answers(&self, role: &str) {
        assert!(
            self.consumer.state.probe(role, PROBE_WINDOW),
            "FAIL: {role} does not answer a probe; {}",
            self.consumer.state.describe()
        );
    }

    fn ended_within(&self, role: &str) {
        assert!(
            self.consumer.state.ended(role, END_WINDOW),
            "FAIL: {role} was not proven ended; {}",
            self.consumer.state.describe()
        );
    }

    /// The process took its lock, published `ready` and is gone now.
    fn ended_now(&self, role: &str) {
        assert!(
            self.consumer.state.exists(&format!("ready-{role}")),
            "SETUP: {role} never read its input; {}",
            self.consumer.state.describe()
        );
        assert_eq!(
            self.consumer.state.liveness(role),
            Liveness::Ended,
            "FAIL: the end was reported while {role} is alive"
        );
    }

    fn nothing_spawned(&self) {
        assert_eq!(
            self.consumer.state.names("started-"),
            Vec::<String>::new(),
            "FAIL: an engine command was started"
        );
    }

    /// The material is whole when read by pathname, and no consumer recorded
    /// that a re-read failed or differed.
    fn material_intact(&self) {
        if let Err(finding) = self.material_state() {
            panic!("FAIL: {finding}");
        }
    }

    /// What `material_intact` requires, as a finding instead of a panic.
    fn material_state(&self) -> Result<(), String> {
        for (relative, content) in MATERIAL_FILES {
            let read = std::fs::read(self.consumer.material.join(relative));
            if read.as_deref().ok() != Some(content.as_bytes()) {
                return Err(format!(
                    "the material is not intact at {relative}: {read:?}"
                ));
            }
        }
        for kind in ["material-missing-", "material-changed-"] {
            let recorded = self.consumer.state.names(kind);
            if !recorded.is_empty() {
                return Err(format!(
                    "a consumer recorded that its material was lost or altered: {recorded:?}"
                ));
            }
        }
        Ok(())
    }

    /// One sample taken while the product gives a consumer its grace, counted
    /// from `requested`: for `window` no end is reported, then `role` answers
    /// a probe, the material is whole and nothing released it. What a sample
    /// finds is classified by `grace_finding`.
    async fn sample_in_grace(
        &self,
        owner: &mut RuntimeInvocationOwner,
        role: &str,
        window: Duration,
        requested: Instant,
    ) {
        let finding = if let Ok(end) = tokio::time::timeout(window, owner.join()).await {
            Some(format!("an end was reported: {end:?}"))
        } else if !self.consumer.state.probe(role, PROBE_WINDOW) {
            Some(format!(
                "{role} does not answer a probe; {}",
                self.consumer.state.describe()
            ))
        } else if let Err(finding) = self.material_state() {
            Some(finding)
        } else {
            let witnesses = self.consumer.witnesses();
            (!witnesses.is_empty()).then(|| format!("the material was released: {witnesses:?}"))
        };
        if let Some(finding) = finding {
            panic!("{}", grace_finding(requested.elapsed(), &finding));
        }
    }

    /// Kept for good: whole on disk, and its release never ran.
    fn material_kept(&self) {
        self.material_intact();
        assert_eq!(
            self.consumer.witnesses(),
            Vec::<String>::new(),
            "FAIL: material kept after an unknown termination was released"
        );
    }

    /// Released exactly once, with no reader alive, and gone.
    fn released_clean(&self) {
        assert_eq!(
            self.consumer.witnesses(),
            ["released-clean"],
            "FAIL: the material was not released with every consumer ended"
        );
        assert!(
            !self.consumer.material.exists(),
            "FAIL: the material directory still exists after its release"
        );
    }

    /// Takes the wait status of the exited leader from outside the owner, as
    /// any other reaper inside the control process would: a library that waits
    /// for every child, for example. The leader's output is held by its
    /// descendant, so the owner has left it unreaped. Returns its PID.
    fn take_leader_status_elsewhere(&self) -> u32 {
        self.ended_within("leader");
        let leader: u32 = self
            .consumer
            .state
            .read("ready-leader")
            .and_then(|pid| pid.trim().parse().ok())
            .expect("SETUP: the leader published its PID");
        let mut status = 0;
        // SAFETY: the leader is a child of this process and `status` is a
        // live, exclusive out-pointer for this call.
        let reaped = unsafe { libc::waitpid(leader as libc::pid_t, &mut status, 0) };
        assert_eq!(
            reaped,
            leader as libc::pid_t,
            "SETUP: the exited leader could not be reaped from outside the owner: {}",
            std::io::Error::last_os_error()
        );
        leader
    }

    async fn slot(&self) -> InvocationFailure {
        adapter::slot_refusal(&self.runtime)
            .await
            .unwrap_or_else(|other| panic!("FAIL: {other}"))
    }

    /// After the last end was taken the slot is free at once: the owner frees
    /// it before it sends that end.
    async fn slot_is_free(&self) {
        assert!(
            matches!(self.slot().await, InvocationFailure::Interrupted { .. }),
            "FAIL: the admission slot is not free after the last end"
        );
    }

    /// Without a taken end, the slot follows the release of the material.
    async fn slot_freed_within(&self, window: Duration) {
        let started = Instant::now();
        while !matches!(self.slot().await, InvocationFailure::Interrupted { .. }) {
            assert!(
                started.elapsed() < window,
                "FAIL: the admission slot is still taken {window:?} after the material was \
                 released"
            );
            tokio::time::sleep(POLL_ONCE).await;
        }
    }
}

fn refused_as(end: InvocationEnd) -> (InvocationFailure, String) {
    let InvocationEnd::Refused(error) = end else {
        panic!("FAIL: the command was not refused: {end:?}");
    };
    let text = format!("{error:#}");
    let reason = error
        .downcast::<InvocationFailure>()
        .unwrap_or_else(|other| panic!("FAIL: the refusal has no typed reason: {other:#}"));
    (reason, text)
}

fn interrupted(signal: i32, kill_sent: bool) -> Option<Stop> {
    Some(Stop {
        reason: StopReason::Interrupted { signal },
        forwarded: Some(signal),
        kill_sent,
    })
}

/// The class of what a sample found, `after` the request, while the product
/// gave a consumer its grace. The product stops that consumer
/// `INTERRUPT_GRACE` after the request at the earliest, so a sample that
/// overran that time (a stalled host) saw the product do what it must: it says
/// nothing about the grace and is inconclusive, not a product failure. This is
/// the rule of the operator target (`regressions::observed`).
fn grace_finding(after: Duration, finding: &str) -> String {
    if after < INTERRUPT_GRACE {
        format!("FAIL: {after:?} after the request, {finding}")
    } else {
        format!(
            "SETUP: a sample overran the grace of the product ({after:?} after the request): \
             {finding}"
        )
    }
}

#[test]
fn l10_sample_inside_the_grace_is_a_failure_and_one_that_overran_it_is_inconclusive() {
    let inside = grace_finding(
        INTERRUPT_GRACE - Duration::from_millis(1),
        "an end was reported",
    );
    assert!(inside.starts_with("FAIL: "), "{inside}");
    for after in [INTERRUPT_GRACE, INTERRUPT_GRACE + Duration::from_secs(3)] {
        let overran = grace_finding(after, "an end was reported");
        assert!(
            overran.starts_with("SETUP: a sample overran the grace of the product"),
            "{overran}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l5_command_interrupted_before_its_admission_is_refused_and_never_started() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let interruption = Interruption::new();
    interruption.request(libc::SIGINT);

    let mut owner = scene.start(material, LONG, Some(&interruption));
    let (reason, _) = refused_as(scene.end(&mut owner, RESOLVE_WINDOW).await);
    assert_eq!(
        reason,
        InvocationFailure::Interrupted {
            signal: libc::SIGINT
        },
        "FAIL: the refusal does not name the requested signal"
    );
    scene.nothing_spawned();
    scene.released_clean();
    assert_eq!(
        interruption.active(),
        0,
        "FAIL: a refused command keeps a hold"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l5b_command_interrupted_during_its_last_check_is_refused_and_never_started() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let interruption = Interruption::new();
    let request = interruption.clone();
    let input = Hooked {
        material,
        hooks: Hooks {
            // The request arrives after the entry check of the owner thread.
            before_spawn: Some(Box::new(move || {
                request.request(libc::SIGHUP);
                Ok(())
            })),
            ..Hooks::default()
        },
    };

    let mut owner = scene.start(input, LONG, Some(&interruption));
    let (reason, _) = refused_as(scene.end(&mut owner, RESOLVE_WINDOW).await);
    assert_eq!(
        reason,
        InvocationFailure::Interrupted {
            signal: libc::SIGHUP
        },
        "FAIL: a request seen right before spawn did not refuse the command"
    );
    scene.nothing_spawned();
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l5c_command_cancelled_during_its_last_check_is_refused_and_never_started() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    // The input's check tells the test that it runs, then waits for it: the
    // cancellation lands after the entry check of the owner thread.
    let (entered, checking) = std::sync::mpsc::channel::<()>();
    let (proceed, cancelled) = std::sync::mpsc::channel::<()>();
    let input = Hooked {
        material,
        hooks: Hooks {
            before_spawn: Some(Box::new(move || {
                entered.send(())?;
                cancelled.recv_timeout(RESOLVE_WINDOW)?;
                Ok(())
            })),
            ..Hooks::default()
        },
    };

    let mut owner = scene.start(input, LONG, None);
    checking
        .recv_timeout(RESOLVE_WINDOW)
        .expect("SETUP: the owner never reached the input's check");
    owner.cancel();
    proceed
        .send(())
        .expect("SETUP: the input's check is no longer waiting");
    let (reason, _) = refused_as(scene.end(&mut owner, RESOLVE_WINDOW).await);
    assert_eq!(
        reason,
        InvocationFailure::Cancelled,
        "FAIL: a cancellation seen right before spawn did not refuse the command"
    );
    scene.nothing_spawned();
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l6_cancelled_command_whose_output_is_held_outside_its_group_is_unknown_and_keeps_its_material()
 {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("descendant", "own-group"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let mut owner = scene.start(material, LONG, None);
    scene.establish(&mut owner, &["leader", "descendant"]).await;

    let cancelled = Instant::now();
    owner.cancel();
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Unknown(unknown) = end else {
        panic!("FAIL: a command whose pipes are held outside its group was reported as {end:?}");
    };
    assert!(
        cancelled.elapsed() >= STOP_OBSERVATION,
        "FAIL: unknown termination was announced {:?} after the stop, before the observation \
         bound",
        cancelled.elapsed()
    );
    assert_eq!(
        unknown.stop,
        Stop {
            reason: StopReason::Cancelled,
            forwarded: None,
            kill_sent: true
        }
    );
    assert!(
        unknown.leader_reaped && !unknown.wait_uncertain,
        "FAIL: the killed leader was not reaped: {unknown:?}"
    );
    assert!(
        unknown.stdout_open && unknown.stderr_open,
        "FAIL: the held pipes are not reported open: {unknown:?}"
    );
    assert_eq!(unknown.process_group, unknown.leader_pid);
    scene.ended_within("leader");
    scene.answers("descendant");
    scene.material_kept();
    assert_eq!(
        scene.slot().await,
        InvocationFailure::Capacity,
        "FAIL: a second command was admitted while the first one is uncollected"
    );

    scene.consumer.state.release();
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: the late end of an unknown termination was reported as {end:?}");
    };
    assert!(
        !collected.input_released && collected.input_changed.is_none(),
        "FAIL: material was released after an announced unknown termination: {collected:?}"
    );
    assert_eq!(collected.status.signal(), Some(libc::SIGKILL));
    scene.ended_now("descendant");
    scene.material_kept();
    scene.slot_is_free().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l7_interruption_is_forwarded_to_the_command_and_collected_without_a_kill() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[("hold", "consume"), ("deadline-seconds", "60")]).await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, LONG, Some(&interruption));
    scene.establish(&mut owner, &["leader"]).await;

    let requested = Instant::now();
    interruption.request(libc::SIGINT);
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: an interrupted command was reported as {end:?}");
    };
    assert!(
        requested.elapsed() < INTERRUPT_GRACE / 2,
        "FAIL: a command that dies on the forwarded signal was collected only after {:?}",
        requested.elapsed()
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGINT),
        "FAIL: the command did not end by the forwarded signal"
    );
    assert_eq!(collected.stopped, interrupted(libc::SIGINT, false));
    assert!(collected.input_released && collected.input_changed.is_none());
    scene.ended_now("leader");
    scene.released_clean();
    assert_eq!(interruption.active(), 0);

    // A caller that only wants a result is told that the owner stopped it.
    let error = InvocationEnd::Collected(collected)
        .into_result(scene.runtime.kind)
        .expect_err("a stopped command has no result");
    let stopped = error
        .downcast_ref::<InvocationStopped>()
        .unwrap_or_else(|| panic!("FAIL: a stopped command mapped to {error:#}"));
    assert_eq!(Some(stopped.stop), interrupted(libc::SIGINT, false));
    assert_eq!(stopped.status.signal(), Some(libc::SIGINT));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l7b_paired_requests_are_one_interruption_and_a_later_one_escalates_to_a_kill() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("ignore-signals", "leader"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, LONG, Some(&interruption));
    scene.establish(&mut owner, &["leader"]).await;

    let requested = Instant::now();
    interruption.request(libc::SIGINT);
    interruption.request(libc::SIGTERM);
    scene.pending(&mut owner, Duration::from_secs(2)).await;
    scene.answers("leader");
    assert!(
        !interruption.escalated(),
        "FAIL: two requests delivered together escalated"
    );
    scene.material_intact();

    interruption.request(libc::SIGINT);
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: an escalated interruption was reported as {end:?}");
    };
    assert!(
        requested.elapsed() < INTERRUPT_GRACE - Duration::from_secs(2),
        "FAIL: the escalation was collected only after {:?}, not before the grace",
        requested.elapsed()
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGKILL),
        "FAIL: a command that ignores the forwarded signal did not end by SIGKILL"
    );
    assert_eq!(collected.stopped, interrupted(libc::SIGINT, true));
    scene.ended_now("leader");
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l7c_cancellation_during_the_grace_kills_the_command_at_once_and_stays_an_interruption() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("ignore-signals", "leader"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, LONG, Some(&interruption));
    scene.establish(&mut owner, &["leader"]).await;

    let requested = Instant::now();
    interruption.request(libc::SIGINT);
    // The consumer ignores the forwarded signal: its grace is running.
    scene
        .sample_in_grace(&mut owner, "leader", Duration::from_secs(1), requested)
        .await;

    owner.cancel();
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command cancelled during its grace was reported as {end:?}");
    };
    assert!(
        requested.elapsed() < INTERRUPT_GRACE - Duration::from_secs(2),
        "FAIL: a command cancelled during its grace was collected only {:?} after the request, \
         not before the end of the grace",
        requested.elapsed()
    );
    assert!(
        !interruption.escalated(),
        "SETUP: the interruption escalated: the kill is not the one of the cancellation"
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGKILL),
        "FAIL: a command that ignores the forwarded signal did not end by SIGKILL"
    );
    // The first reason stays: the stop does not record that the cancellation,
    // not the end of the grace, brought the SIGKILL.
    assert_eq!(collected.stopped, interrupted(libc::SIGINT, true));
    assert!(collected.input_released && collected.input_changed.is_none());
    scene.ended_now("leader");
    scene.released_clean();
    assert_eq!(interruption.active(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l8_material_of_a_dropped_observer_is_released_only_after_its_consumers_ended() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("descendant", "same-group"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, LONG, Some(&interruption));
    scene.establish(&mut owner, &["leader", "descendant"]).await;

    drop(owner);
    let dropped = Instant::now();
    while scene.consumer.material.exists() {
        assert!(
            dropped.elapsed() < END_WINDOW,
            "FAIL: the material of a dropped observer is still there after {END_WINDOW:?}; {}",
            scene.consumer.state.describe()
        );
        std::thread::sleep(POLL_ONCE);
    }
    // Seen missing: both consumers must already be gone.
    scene.ended_now("leader");
    scene.ended_now("descendant");
    scene.released_clean();
    assert_eq!(
        interruption.active(),
        1,
        "FAIL: the hold of a command whose end nobody took was released"
    );
    scene.slot_freed_within(END_WINDOW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l9_material_altered_after_its_hand_over_refuses_the_command_before_spawn() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let input: Box<dyn RetainedInput> = Box::new(material);
    // The test has no handle left: the material is altered by pathname.
    let (relative, _) = MATERIAL_FILES[1];
    std::fs::write(scene.consumer.material.join(relative), "altered\n")
        .expect("SETUP: material canary");

    let mut owner = scene
        .runtime
        .cli_env_retained(Vec::new(), input, LONG, &[], None);
    let (reason, text) = refused_as(scene.end(&mut owner, RESOLVE_WINDOW).await);
    assert_eq!(reason, InvocationFailure::InputChanged);
    assert!(
        text.contains(&format!("material changed: {relative}")),
        "FAIL: the refusal does not carry the input's own finding: {text}"
    );
    scene.nothing_spawned();
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l10_material_stays_while_a_consumer_ignores_the_forwarded_signal_for_the_whole_grace() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("descendant", "same-group"),
        ("ignore-signals", "descendant"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, LONG, Some(&interruption));
    scene.establish(&mut owner, &["leader", "descendant"]).await;

    let requested = Instant::now();
    interruption.request(libc::SIGINT);
    // The leader dies on the forwarded signal; the descendant lives on with
    // both pipes. Nothing may be reported or released during the grace.
    scene.ended_within("leader");
    for _ in 0..3 {
        scene
            .sample_in_grace(
                &mut owner,
                "descendant",
                Duration::from_millis(2500),
                requested,
            )
            .await;
    }
    assert!(
        requested.elapsed() < INTERRUPT_GRACE,
        "SETUP: the samples took {:?}, longer than the grace they observe",
        requested.elapsed()
    );

    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a consumer killed at the end of the grace was reported as {end:?}");
    };
    assert!(
        requested.elapsed() >= INTERRUPT_GRACE,
        "FAIL: the command was collected {:?} after the request, before the grace elapsed",
        requested.elapsed()
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGINT),
        "FAIL: the leader's status is not the one it really ended with"
    );
    assert_eq!(collected.stopped, interrupted(libc::SIGINT, true));
    assert!(collected.input_released);
    scene.ended_now("descendant");
    for marker in ["material-missing-descendant", "material-changed-descendant"] {
        assert!(
            !scene.consumer.state.exists(marker),
            "FAIL: the descendant recorded {marker}"
        );
    }
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l10b_bound_that_elapses_during_the_grace_ends_it_and_the_stop_stays_an_interruption() {
    let _serial = SERIAL.lock().await;
    let duration = Duration::from_secs(4);
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("ignore-signals", "leader"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, duration, Some(&interruption));
    scene.establish(&mut owner, &["leader"]).await;

    let requested = Instant::now();
    interruption.request(libc::SIGINT);
    assert!(
        scene.started.elapsed() < duration,
        "SETUP: the interruption was not requested before the command's {duration:?} bound"
    );
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command killed at its bound during the grace was reported as {end:?}");
    };
    // The grace gives time inside the bound, never beyond it.
    assert!(
        scene.started.elapsed() >= duration,
        "FAIL: a command that ignores the forwarded signal was collected before its bound"
    );
    assert!(
        requested.elapsed() < INTERRUPT_GRACE - Duration::from_secs(2),
        "FAIL: the command outlived its {duration:?} bound for the grace: collected {:?} after \
         the request",
        requested.elapsed()
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGKILL),
        "FAIL: a command that ignores the forwarded signal did not end by SIGKILL"
    );
    // The first reason stays: the stop does not record that the bound, not the
    // end of the grace, brought the SIGKILL.
    assert_eq!(collected.stopped, interrupted(libc::SIGINT, true));
    assert!(collected.input_released && collected.input_changed.is_none());
    scene.ended_now("leader");
    scene.released_clean();
    assert_eq!(interruption.active(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l11_timed_out_command_whose_output_is_held_outside_its_group_is_unknown_and_keeps_its_material()
 {
    let _serial = SERIAL.lock().await;
    let duration = Duration::from_secs(5);
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("descendant", "own-group"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let mut owner = scene.start(material, duration, None);
    scene.establish(&mut owner, &["leader", "descendant"]).await;
    assert!(
        scene.started.elapsed() < duration,
        "SETUP: the consumers were not established before the command's {duration:?} bound"
    );

    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Unknown(unknown) = end else {
        panic!("FAIL: a timed-out command whose pipes are held was reported as {end:?}");
    };
    assert!(
        scene.started.elapsed() >= duration + STOP_OBSERVATION,
        "FAIL: unknown termination was announced before the bound and its observation elapsed"
    );
    assert_eq!(
        unknown.stop,
        Stop {
            reason: StopReason::TimedOut,
            forwarded: None,
            kill_sent: true
        }
    );
    assert!(unknown.stdout_open, "FAIL: {unknown:?}");
    scene.answers("descendant");
    scene.material_kept();

    scene.consumer.state.release();
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: the late end of an unknown termination was reported as {end:?}");
    };
    assert!(
        !collected.input_released,
        "FAIL: material was released after an announced unknown termination"
    );
    assert_eq!(
        collected.stopped.map(|stop| stop.reason),
        Some(StopReason::TimedOut)
    );
    scene.ended_now("descendant");
    scene.material_kept();
    scene.slot_is_free().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l12_unknown_termination_is_still_announced_when_the_input_fails_to_record_it() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[
        ("hold", "consume"),
        ("descendant", "own-group"),
        ("deadline-seconds", "60"),
    ])
    .await;
    let input = Hooked {
        material,
        hooks: Hooks {
            on_unknown: Some(Box::new(|_| {
                panic!("fixture: the input fails while it records an unknown termination")
            })),
            ..Hooks::default()
        },
    };
    let mut owner = scene.start(input, LONG, None);
    scene.establish(&mut owner, &["leader", "descendant"]).await;

    owner.cancel();
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Unknown(unknown) = end else {
        panic!("FAIL: the observer of a failing input was told {end:?}");
    };
    assert!(unknown.stdout_open && unknown.retained_at.is_none());
    scene.answers("descendant");
    scene.material_kept();

    scene.consumer.state.release();
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    assert!(
        matches!(
            end,
            InvocationEnd::Aborted {
                input_released: false
            }
        ),
        "FAIL: the owner of a failing input ended as {end:?}"
    );
    scene.ended_now("descendant");
    scene.material_kept();
    scene.slot_is_free().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l12b_owner_that_fails_before_spawn_ends_as_aborted_with_its_input_released() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let interruption = Interruption::new();
    let input = Hooked {
        material,
        hooks: Hooks {
            before_spawn: Some(Box::new(|| {
                panic!("fixture: the input fails while it is checked before spawn")
            })),
            ..Hooks::default()
        },
    };

    let mut owner = scene.start(input, LONG, Some(&interruption));
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    assert!(
        matches!(
            end,
            InvocationEnd::Aborted {
                input_released: true
            }
        ),
        "FAIL: an owner that failed before spawn ended as {end:?}"
    );
    scene.nothing_spawned();
    // Released by the unwinding owner, with no consumer ever started.
    scene.released_clean();
    assert_eq!(
        interruption.active(),
        0,
        "FAIL: an owner that released its input keeps a hold"
    );
    scene.slot_is_free().await;
    assert_eq!(
        end.into_result(scene.runtime.kind)
            .expect_err("an aborted owner has no result")
            .downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Unavailable)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l13_hold_of_a_command_ends_when_its_end_is_taken_not_when_it_is_sent() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let interruption = Interruption::new();
    let mut owner = scene.start(material, LONG, Some(&interruption));

    // The owner releases the material right before it sends the end.
    let started = Instant::now();
    while scene.consumer.material.exists() {
        assert!(
            started.elapsed() < RESOLVE_WINDOW,
            "FAIL: a command that exits 0 still has its material after {RESOLVE_WINDOW:?}; {}",
            scene.consumer.state.describe()
        );
        std::thread::sleep(POLL_ONCE);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        interruption.active(),
        1,
        "FAIL: the hold ended before the caller took the end"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), interruption.idle())
            .await
            .is_err(),
        "FAIL: the interruption is idle while an end is still to be taken"
    );

    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command that exits 0 was reported as {end:?}");
    };
    assert!(collected.status.success() && collected.stopped.is_none());
    assert!(collected.input_released);
    assert_eq!(
        interruption.active(),
        0,
        "FAIL: the taken end kept its hold"
    );
    tokio::time::timeout(Duration::from_secs(1), interruption.idle())
        .await
        .expect("FAIL: the interruption is not idle after the end was taken");
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l13b_observer_dropped_with_its_end_already_sent_releases_its_hold() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let interruption = Interruption::new();
    let owner = scene.start(material, LONG, Some(&interruption));

    // The owner releases the material, frees the slot and sends the end, in
    // that order and without anything in between.
    let started = Instant::now();
    while scene.consumer.material.exists() {
        assert!(
            started.elapsed() < RESOLVE_WINDOW,
            "FAIL: a command that exits 0 still has its material after {RESOLVE_WINDOW:?}; {}",
            scene.consumer.state.describe()
        );
        std::thread::sleep(POLL_ONCE);
    }
    scene.slot_freed_within(END_WINDOW).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        interruption.active(),
        1,
        "FAIL: the hold ended before the observer took or dropped the end"
    );

    drop(owner);
    assert_eq!(
        interruption.active(),
        0,
        "FAIL: an observer dropped with a releasing end already sent kept its hold for good"
    );
    tokio::time::timeout(Duration::from_secs(1), interruption.idle())
        .await
        .expect("FAIL: the interruption is not idle after the observer settled its end");
    scene.ended_now("leader");
    scene.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l14_failed_check_after_collection_is_reported_with_the_collected_end() {
    let _serial = SERIAL.lock().await;
    let (scene, material) = Scene::open(&[]).await;
    let input = Hooked {
        material,
        hooks: Hooks {
            after_collection: Some(Box::new(|| {
                anyhow::bail!("fixture: the material differs after the command")
            })),
            ..Hooks::default()
        },
    };

    let mut owner = scene.start(input, LONG, None);
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = &end else {
        panic!("FAIL: a command that exits 0 was reported as {end:?}");
    };
    assert!(collected.status.success() && collected.stopped.is_none());
    assert!(
        collected.input_changed.is_some() && collected.input_released,
        "FAIL: the failed check is not in the collected end: {collected:?}"
    );
    scene.ended_now("leader");
    scene.released_clean();

    let error = end
        .into_result(scene.runtime.kind)
        .expect_err("a result is not bound to material that failed its check");
    assert_eq!(
        error.downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::InputChanged)
    );
    assert!(
        format!("{error:#}").contains("the material differs after the command"),
        "FAIL: the input's own finding is missing: {error:#}"
    );
}

/// Set, to the name of one test, in the process that runs that test alone.
const ALONE: &str = "AI_GUARD_RETAINED_CLI_SCENARIO_ALONE";
/// One such process at a time: each compiles the fixture engine in the
/// scratch directory this binary shares.
static ONE_ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `scenario`, the body of the test `name` of this module, in a process
/// of its own.
///
/// After an uncertain wait the owner never collects its command and keeps the
/// one admission slot of its process for good: in the test binary no later
/// scenario would be admitted. The test therefore starts this binary again for
/// itself alone, and passes only if that process ran exactly this test and it
/// passed there.
fn alone<S>(name: &str, scenario: impl FnOnce() -> S)
where
    S: Future<Output = ()>,
{
    if std::env::var_os(ALONE).is_some_and(|test| test == name) {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("SETUP: the scenario's runtime")
            .block_on(scenario());
        return;
    }
    // Compiled here first: no fixture build of this process can then overlap
    // the one the other process runs on the same scratch files.
    fixture_binary();
    let module = module_path!();
    let test = format!(
        "{}::{name}",
        module.split_once("::").map_or(module, |(_, path)| path)
    );
    let turn = ONE_ALONE.lock().unwrap_or_else(PoisonError::into_inner);
    let output = Command::new(std::env::current_exe().expect("SETUP: the test binary's own path"))
        .args([test.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(ALONE, name)
        .stdin(Stdio::null())
        .output()
        .expect("SETUP: the test binary could not be started again");
    drop(turn);
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    eprintln!("{test} in its own process:\n{stderr}");
    if output.status.success() && stdout.contains("test result: ok. 1 passed;") {
        return;
    }
    // The class the scenario gave its own failure is kept.
    let class = ["SETUP:", "UNKNOWN:"]
        .into_iter()
        .find(|class| stderr.contains(class))
        .unwrap_or("FAIL:");
    panic!(
        "{class} {test} did not pass in its own process ({}); its output:\n{stdout}",
        output.status
    );
}

#[test]
fn l15_stop_of_a_command_whose_leader_was_reaped_elsewhere_signals_nothing_and_is_unknown() {
    alone(
        "l15_stop_of_a_command_whose_leader_was_reaped_elsewhere_signals_nothing_and_is_unknown",
        || async {
            let (scene, material) = Scene::open(&[
                ("descendant", "same-group"),
                ("leader-exit", "0"),
                ("deadline-seconds", "60"),
            ])
            .await;
            let mut owner = scene.start(material, LONG, None);
            scene.establish(&mut owner, &["descendant"]).await;
            let leader = scene.take_leader_status_elsewhere();

            let cancelled = Instant::now();
            owner.cancel();
            let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
            let InvocationEnd::Unknown(unknown) = end else {
                panic!("FAIL: a stop whose process group cannot be named was reported as {end:?}");
            };
            // Announced at once: nothing was sent, so there is nothing to observe.
            assert!(
                cancelled.elapsed() < STOP_OBSERVATION,
                "FAIL: an uncertain wait was announced only {:?} after the stop",
                cancelled.elapsed()
            );
            assert_eq!(
                unknown.stop,
                Stop {
                    reason: StopReason::Cancelled,
                    forwarded: None,
                    kill_sent: false
                },
                "FAIL: a signal was reported for a process group that could not be named"
            );
            assert!(
                unknown.wait_uncertain && !unknown.leader_reaped,
                "FAIL: the lost wait status is not reported: {unknown:?}"
            );
            assert!(unknown.stdout_open && unknown.stderr_open, "{unknown:?}");
            assert_eq!(
                (unknown.leader_pid, unknown.process_group),
                (leader, leader)
            );
            // The descendant is in the group the leader's PID named. A SIGKILL
            // sent to that group would have ended it.
            std::thread::sleep(Duration::from_millis(500));
            scene.answers("descendant");
            scene.material_kept();
            assert_eq!(
                scene.slot().await,
                InvocationFailure::Capacity,
                "FAIL: a second command was admitted while the first one is uncollected"
            );

            // Its output closes, and still it is never collected: its status
            // is gone. The material and the slot stay.
            scene.consumer.state.release();
            scene.ended_within("descendant");
            scene.pending(&mut owner, Duration::from_secs(1)).await;
            scene.material_kept();
            assert_eq!(scene.slot().await, InvocationFailure::Capacity);
        },
    );
}

#[test]
fn l16_command_whose_wait_status_was_taken_elsewhere_is_unknown_without_any_signal() {
    alone(
        "l16_command_whose_wait_status_was_taken_elsewhere_is_unknown_without_any_signal",
        || async {
            let (scene, material) = Scene::open(&[
                ("descendant", "same-group"),
                ("leader-exit", "0"),
                ("deadline-seconds", "60"),
            ])
            .await;
            let interruption = Interruption::new();
            let mut owner = scene.start(material, LONG, Some(&interruption));
            scene.establish(&mut owner, &["descendant"]).await;
            let leader = scene.take_leader_status_elsewhere();
            // Nothing is reported while the output is held: the owner has not
            // asked for the status yet.
            scene.pending(&mut owner, Duration::from_millis(500)).await;

            // The output closes by itself; the owner now asks for a status
            // that no longer exists.
            scene.consumer.state.release();
            let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
            let InvocationEnd::Unknown(unknown) = end else {
                panic!("FAIL: a command whose wait status is gone was reported as {end:?}");
            };
            assert_eq!(
                unknown.stop,
                Stop {
                    reason: StopReason::WaitUnavailable,
                    forwarded: None,
                    kill_sent: false
                },
                "FAIL: a stop or a signal was reported for a command nobody stopped"
            );
            assert!(
                unknown.wait_uncertain && !unknown.leader_reaped,
                "FAIL: the lost wait status is not reported: {unknown:?}"
            );
            assert!(
                !unknown.stdout_open && !unknown.stderr_open,
                "FAIL: closed pipes are reported open: {unknown:?}"
            );
            assert_eq!(
                (unknown.leader_pid, unknown.process_group),
                (leader, leader)
            );
            scene.ended_within("descendant");
            scene.material_kept();
            assert_eq!(
                interruption.active(),
                1,
                "FAIL: the hold of an unknown termination was released"
            );
            assert_eq!(scene.slot().await, InvocationFailure::Capacity);
            assert_eq!(
                InvocationEnd::Unknown(unknown)
                    .into_result(scene.runtime.kind)
                    .expect_err("an unknown termination has no result")
                    .downcast_ref::<InvocationFailure>(),
                Some(&InvocationFailure::Unknown)
            );
        },
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l1b_standard_output_beyond_its_bound_is_cut_at_256_kib_and_counted_whole() {
    /// Twice the documented bound of the captured standard output.
    const BURST: usize = 2 * 256 * 1024;
    /// The line the fixture engine repeats to fill a burst of standard output.
    const BURST_LINE: &[u8] = b"fixture-stdout-line\n";
    let _serial = SERIAL.lock().await;
    let burst = format!("{BURST},0");
    let (scene, material) = Scene::open(&[("burst", burst.as_str())]).await;

    let mut owner = scene.start(material, LONG, None);
    let end = scene.end(&mut owner, RESOLVE_WINDOW).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command that writes its output and exits 0 was reported as {end:?}");
    };
    assert!(
        collected.status.success() && collected.stopped.is_none(),
        "FAIL: {collected:?}"
    );
    assert_eq!(
        collected.stdout.bytes.len(),
        256 * 1024,
        "FAIL: the captured standard output is not cut at 256 KiB"
    );
    assert!(
        collected.stdout.truncated,
        "FAIL: a cut capture is not marked as truncated"
    );
    // Everything the command wrote is counted: its burst, then the line a
    // fixture command ends with.
    assert_eq!(
        collected.stdout.total_bytes,
        (BURST + ENGINE_STDOUT.len()) as u64,
        "FAIL: the bytes beyond the bound were not counted"
    );
    let first: Vec<u8> = BURST_LINE
        .iter()
        .copied()
        .cycle()
        .take(256 * 1024)
        .collect();
    assert!(
        collected.stdout.bytes == first,
        "FAIL: what is kept is not the first 256 KiB the command wrote"
    );
    assert_eq!(
        STDOUT_LIMIT,
        256 * 1024,
        "FAIL: the bound of the captured standard output is not the documented one"
    );
    assert_eq!(
        (collected.stderr.total_bytes, collected.stderr.truncated),
        (0, false),
        "FAIL: {collected:?}"
    );
    assert!(collected.input_released && collected.input_changed.is_none());
    scene.ended_now("leader");
    scene.released_clean();
}
