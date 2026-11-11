//! L0–L4: when the caller of one engine command is told that the command
//! ended, what is then true of the processes that read its material.
//!
//! Each test carries its complete oracle list. On a consumer that waits for
//! the direct child only, L2, L3 and L4 stop at their `DEFECT:` assertion; L0
//! and L1 are controls and pass.
use crate::{
    adapter::{self, Call, Outcome, RETAINED},
    harness::{Consumer, ENGINE_STDOUT, Liveness, Product, STDERR_CANARY, missed_ready},
};
use std::time::{Duration, Instant, SystemTime};

/// The consumer admits one retained command per process.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Test-side observation windows. They are not product deadlines.
const SLOT_WINDOW: Duration = Duration::from_secs(20);
const READY_WINDOW: Duration = Duration::from_secs(30);
const RESOLVE_WINDOW: Duration = Duration::from_secs(60);
const PROBE_WINDOW: Duration = Duration::from_secs(3);
const END_WINDOW: Duration = Duration::from_secs(10);
const POLL_ONCE: Duration = Duration::from_millis(1);

struct Scenario {
    call: Call,
    consumer: Consumer,
    /// Bound handed to the consumer for this command: a test parameter.
    duration: Duration,
    started: Instant,
    started_at: SystemTime,
}

async fn begin(directives: &[(&str, &str)], duration: Duration) -> Scenario {
    eprintln!("consumer under test: retained owner = {RETAINED}");
    let (consumer, material) = Consumer::prepare(directives);
    let runtime = consumer.runtime();
    if let Err(busy) = adapter::await_free_slot(&runtime, SLOT_WINDOW).await {
        panic!("SETUP: retained slot still busy since the previous test: {busy}");
    }
    let started_at = SystemTime::now();
    let started = Instant::now();
    // The material is handed over whole: the test keeps its path only.
    let call = adapter::start(&runtime, material, duration);
    Scenario {
        call,
        consumer,
        duration,
        started,
        started_at,
    }
}

impl Scenario {
    /// Requires `ready-<role>` for every role and one answered probe of the
    /// last one. With `before_deadline`, all of it must happen before the
    /// command's own bound: later, the scenario was not reached.
    async fn establish(&mut self, roles: &[&str], before_deadline: bool) {
        let remaining = |scenario: &Self| {
            if before_deadline {
                scenario.duration.saturating_sub(scenario.started.elapsed())
            } else {
                READY_WINDOW
            }
        };
        let mut missing = None;
        for role in roles {
            match self.consumer.state.wait_ready(role, remaining(self)) {
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
        if missing.is_none() && self.consumer.state.probe(last, remaining(self)) {
            eprintln!(
                "latency: start to first answer of {last} {:?}",
                self.started.elapsed()
            );
            return;
        }
        let waited = self.started.elapsed();
        let ended = self.call.outcome(POLL_ONCE).await;
        if before_deadline && waited >= self.duration {
            panic!(
                "SETUP: the consumers were not established before the command's {:?} bound \
                 (waited {waited:?}); product: {ended:?}; {}",
                self.duration,
                self.consumer.state.describe()
            );
        }
        let product = match ended {
            Some(outcome) => Product::Ended(format!("{outcome:?}")),
            None => Product::Pending,
        };
        panic!(
            "{}",
            missed_ready(
                &self.consumer.state,
                missing.unwrap_or(last),
                "build",
                self.started_at,
                waited,
                &product
            )
        );
    }

    /// The end the caller is told, within a test-side window.
    async fn resolved(&mut self, window: Duration) -> Outcome {
        match self.call.outcome(window).await {
            Some(outcome) => {
                eprintln!(
                    "latency: start to outcome {:?}: {outcome:?}",
                    self.started.elapsed()
                );
                outcome
            }
            None => {
                let alive = self.alive_roles();
                if alive.is_empty() {
                    panic!(
                        "FAIL: every consumer is proven ended but the call is still unresolved \
                         after {window:?}; {}",
                        self.consumer.state.describe()
                    );
                }
                panic!(
                    "UNKNOWN: the call is unresolved after {window:?}; alive: {alive:?}; {}",
                    self.consumer.state.describe()
                );
            }
        }
    }

    fn alive_roles(&self) -> Vec<&'static str> {
        crate::harness::ROLES
            .into_iter()
            .filter(|role| self.consumer.state.liveness(role) == Liveness::Alive)
            .collect()
    }

    /// An `Unknown` in a scenario whose consumers stay in the command's own
    /// process group is not an expected end.
    fn refuse_unknown(&self, outcome: &Outcome) {
        assert!(
            *outcome != Outcome::Unknown,
            "FAIL: unexpected unknown termination {:?} after the start; {}",
            self.started.elapsed(),
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
            "FAIL: the call was reported while {role} is alive"
        );
    }

    fn ended_within(&self, role: &str) {
        assert!(
            self.consumer.state.ended(role, END_WINDOW),
            "FAIL: {role} was not proven ended after the call was reported; {}",
            self.consumer.state.describe()
        );
    }

    /// Facts added to a defect message: how the material was released, and
    /// whether the surviving reader then failed to read it again by pathname.
    fn release_evidence(&self) -> String {
        let unreadable = self
            .consumer
            .state
            .wait_for("material-missing-descendant", PROBE_WINDOW)
            .is_some();
        format!(
            "release witnesses: {:?}; the descendant then failed to re-read the material: \
             {unreadable}",
            self.consumer.witnesses()
        )
    }

    /// The material was released exactly once, with no reader alive, and its
    /// directory is gone when the caller learns the end.
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l0_completed_command_is_reported_after_real_exit_and_output_eof() {
    let _serial = SERIAL.lock().await;
    let mut scenario = begin(&[], Duration::from_secs(30)).await;

    let outcome = scenario.resolved(RESOLVE_WINDOW).await;
    assert_eq!(
        outcome,
        Outcome::Completed {
            stdout_len: ENGINE_STDOUT.len()
        },
        "FAIL: a command that exits 0 was not reported as completed with its whole output"
    );
    scenario.ended_now("leader");
    scenario.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l1_failed_command_keeps_bounded_output_and_is_reported_after_real_exit_and_output_eof() {
    let _serial = SERIAL.lock().await;
    let mut scenario = begin(
        &[("burst", "524288,131072"), ("exit", "7")],
        Duration::from_secs(30),
    )
    .await;

    let outcome = scenario.resolved(RESOLVE_WINDOW).await;
    let Outcome::Failed {
        code,
        stderr_len,
        truncated,
        total,
        display,
    } = outcome
    else {
        panic!("FAIL: a command that exits 7 was reported as {outcome:?}");
    };
    assert_eq!(code, Some(7), "FAIL: the exit status was not reported");
    assert_eq!(
        (stderr_len, truncated),
        (64 * 1024, true),
        "FAIL: standard error is not kept up to its 64 KiB bound"
    );
    assert!(
        total >= 128 * 1024,
        "FAIL: standard error was not read to its end: {total} bytes"
    );
    assert!(
        !display.contains(STDERR_CANARY),
        "FAIL: the failure formats captured output"
    );
    scenario.ended_now("leader");
    scenario.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l2_timed_out_command_is_reported_only_with_its_consumers_ended_or_as_unknown() {
    let _serial = SERIAL.lock().await;
    let duration = Duration::from_secs(5);
    let mut scenario = begin(
        &[
            ("hold", "consume"),
            ("descendant", "same-group"),
            ("deadline-seconds", "60"),
        ],
        duration,
    )
    .await;
    scenario.establish(&["leader", "descendant"], true).await;

    let outcome = scenario.resolved(duration + Duration::from_secs(20)).await;
    scenario.refuse_unknown(&outcome);
    assert_eq!(
        outcome,
        Outcome::TimedOut,
        "FAIL: a command that outlives its bound was not reported as timed out"
    );
    assert!(
        scenario.started.elapsed() >= duration,
        "FAIL: the command was reported as timed out before its bound"
    );
    if scenario.consumer.state.probe("descendant", PROBE_WINDOW) {
        panic!(
            "DEFECT: timed-out call returned a terminal error while a consumer descendant \
             answers a probe; {}",
            scenario.release_evidence()
        );
    }
    scenario.ended_within("leader");
    scenario.ended_within("descendant");
    scenario.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_exited_command_whose_output_is_still_held_is_resolved_at_its_deadline_not_left_pending()
{
    let _serial = SERIAL.lock().await;
    let duration = Duration::from_secs(10);
    let mut scenario = begin(
        &[
            ("descendant", "same-group"),
            ("leader-exit", "0"),
            ("deadline-seconds", "60"),
        ],
        duration,
    )
    .await;
    scenario.establish(&["descendant"], true).await;

    let window = duration.saturating_sub(scenario.started.elapsed()) + Duration::from_secs(8);
    let Some(outcome) = scenario.call.outcome(window).await else {
        let leader = scenario.consumer.state.liveness("leader");
        let descendant = scenario.consumer.state.probe("descendant", PROBE_WINDOW);
        if leader == Liveness::Ended && descendant {
            panic!(
                "DEFECT: call unresolved after its own deadline; leader proven dead, descendant \
                 alive; release witnesses: {:?}",
                scenario.consumer.witnesses()
            );
        }
        panic!(
            "UNKNOWN: the call is unresolved {:?} after its start; leader {leader:?}, descendant \
             answers a probe: {descendant}; {}",
            scenario.started.elapsed(),
            scenario.consumer.state.describe()
        );
    };
    let elapsed = scenario.started.elapsed();
    eprintln!("latency: start to outcome {elapsed:?}: {outcome:?}");
    scenario.refuse_unknown(&outcome);
    assert!(
        elapsed >= duration,
        "FAIL: the call was resolved as {outcome:?} after {elapsed:?}, before its {duration:?} \
         bound, while its output was still held"
    );
    assert_eq!(
        outcome,
        Outcome::TimedOut,
        "FAIL: a command whose output is held past its bound was not reported as timed out"
    );
    scenario.ended_within("leader");
    scenario.ended_within("descendant");
    for marker in ["material-missing-descendant", "material-changed-descendant"] {
        assert!(
            !scenario.consumer.state.exists(marker),
            "FAIL: the descendant recorded {marker}: {:?}",
            scenario.consumer.state.read(marker)
        );
    }
    scenario.released_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l4_cancelled_command_is_reported_only_with_its_consumers_ended_or_as_unknown() {
    let _serial = SERIAL.lock().await;
    let mut scenario = begin(
        &[
            ("hold", "consume"),
            ("descendant", "same-group"),
            ("deadline-seconds", "60"),
        ],
        Duration::from_secs(60),
    )
    .await;
    scenario.establish(&["leader", "descendant"], false).await;

    scenario.call.cancel();
    let outcome = scenario.resolved(Duration::from_secs(20)).await;
    scenario.refuse_unknown(&outcome);
    assert_eq!(
        outcome,
        Outcome::Cancelled,
        "FAIL: a cancelled command was not reported as cancelled"
    );
    if scenario.consumer.state.probe("descendant", PROBE_WINDOW) {
        panic!(
            "DEFECT: cancellation returned while a consumer descendant answers a probe; {}",
            scenario.release_evidence()
        );
    }
    scenario.ended_within("leader");
    scenario.ended_within("descendant");
    scenario.released_clean();
}
