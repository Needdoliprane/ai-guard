//! How the scenarios reach the consumer API. This is the only file of the
//! target that depends on which consumer the product offers.
//!
//! This version drives `Runtime::cli_env_retained`: the command is owned by a
//! dedicated thread of the product, the material is handed over whole as its
//! retained input, and cancelling asks that owner to stop the command.
use crate::harness::Material;
use agentsandbox::runtime::{
    Runtime, RuntimeKind,
    retained_cli::{
        Interruption, InvocationEnd, InvocationFailure, RetainedInput, RuntimeInvocationOwner,
        StopReason,
    },
};
use std::{
    sync::PoisonError,
    time::{Duration, Instant},
};

/// Whether the scenarios run through the retained owner. Printed by every test.
pub const RETAINED: bool = true;

/// What the caller of one engine command learns about its end. The scenarios
/// name every variant; a given consumer produces only some of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The command ended by itself with a success status.
    Completed {
        stdout_len: usize,
    },
    /// The command ended by itself with a failure status.
    Failed {
        code: Option<i32>,
        stderr_len: usize,
        truncated: bool,
        total: u64,
        /// Everything the error formats by itself, plain and debug.
        display: String,
    },
    TimedOut,
    Cancelled,
    /// The end of the command could not be established.
    Unknown,
    Refused(String),
    Other(String),
}

/// The material is the retained input: the owner takes its arguments once,
/// checks its canaries immediately before spawn and drops it after collection.
impl RetainedInput for Material {
    fn arguments(&self) -> Vec<String> {
        Material::arguments(self)
    }
    fn verify_before_spawn(&self) -> anyhow::Result<()> {
        self.verify().map_err(anyhow::Error::msg)
    }
}

/// The consumer admits one retained command per process, and the scenarios of
/// this binary live in two modules: one scenario at a time holds the turn.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// The turn `await_free_slot` obtained for the `start` that follows it.
static GRANTED: std::sync::Mutex<Option<Turn>> = std::sync::Mutex::new(None);

/// One scenario's exclusive use of the consumer. Dropping it ends the turn.
pub struct Turn {
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

/// An input for a command that is never started.
struct SlotProbe;
impl RetainedInput for SlotProbe {
    fn arguments(&self) -> Vec<String> {
        vec!["version".into()]
    }
    fn verify_before_spawn(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Why a command whose interruption is already requested is refused. Nothing
/// is spawned for it, and capacity is decided before the request is looked at:
/// `Capacity` means the admission slot is taken, `Interrupted` that it was free.
pub async fn slot_refusal(runtime: &Runtime) -> Result<InvocationFailure, String> {
    let interruption = Interruption::new();
    interruption.request(libc::SIGTERM);
    let mut probe = runtime.cli_env_retained(
        Vec::new(),
        Box::new(SlotProbe),
        Duration::from_secs(1),
        &[],
        Some(interruption),
    );
    match probe.join().await {
        InvocationEnd::Refused(error) => error.downcast::<InvocationFailure>().map_err(|other| {
            format!("the slot probe was refused without a typed reason: {other:#}")
        }),
        other => Err(format!("the slot probe was not refused: {other:?}")),
    }
}

/// Waits for this binary's turn, then for a free admission slot of the product.
///
/// The turn itself has no bound. Every scenario ahead ends within its own
/// windows and gives the turn back when it is dropped, after a panic as well,
/// so this wait ends when they do and cannot expire before them: with the
/// default parallel test threads a scenario may queue behind one that observes
/// the product's real grace. `window` bounds only what follows, the time the
/// product may still own the previous command.
pub async fn admit(runtime: &Runtime, window: Duration) -> Result<Turn, String> {
    let turn = Turn {
        _turn: TURN.lock().await,
    };
    let started = Instant::now();
    loop {
        match slot_refusal(runtime).await? {
            InvocationFailure::Interrupted { .. } => return Ok(turn),
            InvocationFailure::Capacity => {}
            other => return Err(format!("the slot probe was refused as {other:?}")),
        }
        if started.elapsed() >= window {
            return Err("the product still owns the previous retained command".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until a new command can be admitted. The turn it obtains is handed to
/// the `start` that follows.
pub async fn await_free_slot(runtime: &Runtime, window: Duration) -> Result<(), String> {
    let turn = admit(runtime, window).await?;
    *GRANTED.lock().unwrap_or_else(PoisonError::into_inner) = Some(turn);
    Ok(())
}

pub struct Call {
    // Declared first: the observer is dropped, and the command asked to stop,
    // before the turn passes to the next scenario.
    owner: RuntimeInvocationOwner,
    kind: RuntimeKind,
    outcome: Option<Outcome>,
    _turn: Option<Turn>,
}

/// Starts the command that reads `material`, bounded by `duration`. The
/// material is moved into the owner: the caller keeps nothing of it.
pub fn start(runtime: &Runtime, material: Material, duration: Duration) -> Call {
    let turn = GRANTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    Call {
        owner: runtime.cli_env_retained(Vec::new(), Box::new(material), duration, &[], None),
        kind: runtime.kind,
        outcome: None,
        _turn: turn,
    }
}

impl Call {
    /// Asks the owner to stop the command. Its end is still reported by `outcome`.
    pub fn cancel(&mut self) {
        self.owner.cancel();
    }

    /// The end of the command, or `None` when it is unresolved after `window`.
    pub async fn outcome(&mut self, window: Duration) -> Option<Outcome> {
        if self.outcome.is_none() {
            let end = tokio::time::timeout(window, self.owner.join()).await.ok()?;
            self.outcome = Some(classify(end, self.kind));
        }
        self.outcome.clone()
    }
}

fn classify(end: InvocationEnd, kind: RuntimeKind) -> Outcome {
    let collected = match end {
        InvocationEnd::Collected(collected) => collected,
        InvocationEnd::Unknown(_) => return Outcome::Unknown,
        InvocationEnd::Refused(error) => return Outcome::Refused(format!("{error:#}")),
        aborted @ InvocationEnd::Aborted { .. } => return Outcome::Other(format!("{aborted:?}")),
    };
    if !collected.input_released || collected.input_changed.is_some() {
        return Outcome::Other(format!("{collected:?}"));
    }
    match collected.stopped.map(|stop| stop.reason) {
        Some(StopReason::TimedOut) => Outcome::TimedOut,
        Some(StopReason::Cancelled) => Outcome::Cancelled,
        Some(_) => Outcome::Other(format!("{collected:?}")),
        None if collected.status.success() => Outcome::Completed {
            stdout_len: collected.stdout.bytes.len(),
        },
        None => {
            let code = collected.status.code();
            let (stderr_len, truncated, total) = (
                collected.stderr.bytes.len(),
                collected.stderr.truncated,
                collected.stderr.total_bytes,
            );
            // What a caller that only wants a result is given for this end.
            let display = match InvocationEnd::Collected(collected).into_result(kind) {
                Err(error) => format!("{error:#}\n{error:?}"),
                Ok(_) => return Outcome::Other("a failed command mapped to a result".into()),
            };
            Outcome::Failed {
                code,
                stderr_len,
                truncated,
                total,
                display,
            }
        }
    }
}
