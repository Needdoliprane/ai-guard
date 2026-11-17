//! Physical control-call custody, without start, terminal or retirement rights.
//! Only the actual spawn/collection producers create the opaque settled proofs.
use super::*;
use std::{
    any::Any,
    fmt,
    sync::{Arc, Mutex},
};

pub(crate) struct NativeNoChild {
    _scope: Uuid,
}
pub(crate) struct NativeCollected {
    _scope: Uuid,
}
impl fmt::Debug for NativeNoChild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeNoChild([native proof])")
    }
}
impl fmt::Debug for NativeCollected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeCollected([native proof])")
    }
}

pub(crate) enum NativeRuntimeOutcome<T> {
    NoChild {
        result: Result<T>,
        proof: NativeNoChild,
    },
    Collected {
        result: Result<T>,
        proof: NativeCollected,
    },
    Uncollected(NativeUncollectedCall),
}
pub(super) fn no_child<T>(result: Result<T>) -> NativeRuntimeOutcome<T> {
    // Used only on an actual branch before any control Child can be emitted.
    NativeRuntimeOutcome::NoChild {
        result,
        proof: NativeNoChild {
            _scope: Uuid::new_v4(),
        },
    }
}
pub(crate) enum NativeCollectionOutcome {
    NoChild {
        result: Result<()>,
        proof: NativeNoChild,
    },
    Collected {
        result: Result<()>,
        proof: NativeCollected,
    },
    PendingOrUnknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeIoPhase {
    NoChild,
    Collected,
    Pending,
    Unknown,
}

pub(crate) struct NativeStopResult {
    pub(crate) observation: ContainerObservation,
    // The containing Collected proof covers every CLI used by this result.
    pub(crate) kill_failure: Option<anyhow::Error>,
}
pub(crate) struct NativeStopFailure {
    pub(crate) kill_failure: Option<anyhow::Error>,
    pub(crate) followup_failure: anyhow::Error,
}
impl fmt::Display for NativeStopFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("owned stop failed; original command and observation causes retained")
    }
}
impl fmt::Debug for NativeStopFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeStopFailure")
            .field("kill_failure_retained", &self.kill_failure.is_some())
            .finish_non_exhaustive()
    }
}
impl std::error::Error for NativeStopFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(
            self.kill_failure
                .as_ref()
                .unwrap_or(&self.followup_failure)
                .as_ref(),
        )
    }
}
struct CollectionFailure {
    first: anyhow::Error,
    // Keep the later native failure separately without replacing the primary
    // source. It is retained for the lifetime of this private native cause.
    _collection: anyhow::Error,
}
impl fmt::Display for CollectionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("native control collection failed; original causes retained")
    }
}
impl fmt::Debug for CollectionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CollectionFailure([private native causes])")
    }
}
impl std::error::Error for CollectionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.first.as_ref())
    }
}

type Io = Arc<tokio::sync::Mutex<batch_io::OwnedBatchIo>>;
type Spawn = Arc<tokio::sync::Mutex<SpawnTask>>;
struct SpawnTask {
    handle: tokio::task::JoinHandle<Result<Instant>>,
    failed: bool,
    returned: bool,
}
struct Ledger {
    // Explicit producer transitions, never absence of an error downcast.
    settled: bool,
    emitted: bool,
    unknown: bool,
    io: Option<Io>,
    spawn: Option<Spawn>,
    // Handoff protection from the actual spawn until IO ownership is installed.
    child: Option<tokio::process::Child>,
}
struct ScopeInner {
    id: Uuid,
    runtime: Runtime,
    ledger: Mutex<Ledger>,
}
impl Drop for ScopeInner {
    fn drop(&mut self) {
        let state = match self.ledger.get_mut() {
            Ok(state) => state,
            Err(error) => {
                let state = error.into_inner();
                state.unknown = true;
                state
            }
        };
        if !state.settled || state.unknown {
            // A dropped observer is not collection. The caller's S/W cell is
            // still charged; this pins only that already allocated operation.
            let retained = (
                state.io.take(),
                state.spawn.take(),
                state.child.take(),
                self.runtime.clone(),
            );
            std::mem::forget(retained);
        }
    }
}
#[derive(Clone)]
pub(super) struct NativeControlScope(Arc<ScopeInner>);
impl fmt::Debug for NativeControlScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NativeControlScope")
            .field(&self.phase())
            .finish()
    }
}
impl NativeControlScope {
    pub(super) fn new(mut runtime: Runtime) -> Self {
        runtime.native_control = None;
        Self(Arc::new(ScopeInner {
            id: Uuid::new_v4(),
            runtime,
            ledger: Mutex::new(Ledger {
                settled: true,
                emitted: false,
                unknown: false,
                io: None,
                spawn: None,
                child: None,
            }),
        }))
    }
    fn state(&self) -> std::sync::MutexGuard<'_, Ledger> {
        match self.0.ledger.lock() {
            Ok(state) => state,
            Err(error) => {
                let mut state = error.into_inner();
                state.unknown = true;
                state
            }
        }
    }
    pub(super) fn phase(&self) -> NativeIoPhase {
        let state = self.state();
        if state.unknown {
            NativeIoPhase::Unknown
        } else if !state.settled {
            NativeIoPhase::Pending
        } else if state.emitted {
            NativeIoPhase::Collected
        } else {
            NativeIoPhase::NoChild
        }
    }
    pub(super) fn runtime(&self) -> Runtime {
        let mut runtime = self.0.runtime.clone();
        runtime.native_control = Some(self.clone());
        runtime
    }
    pub(super) fn finish<T: Send + Sync + 'static>(
        &self,
        result: Result<T>,
    ) -> NativeRuntimeOutcome<T> {
        match self.phase() {
            NativeIoPhase::NoChild => NativeRuntimeOutcome::NoChild {
                result,
                proof: NativeNoChild { _scope: self.0.id },
            },
            NativeIoPhase::Collected => NativeRuntimeOutcome::Collected {
                result,
                proof: NativeCollected { _scope: self.0.id },
            },
            NativeIoPhase::Pending | NativeIoPhase::Unknown => {
                let (cause, retained_value) = match result {
                    Err(error) => (error, None),
                    Ok(value) => (
                        anyhow::anyhow!(
                            "native control result lacks collection; original value retained"
                        ),
                        Some(Box::new(value) as Box<dyn Any + Send + Sync>),
                    ),
                };
                NativeRuntimeOutcome::Uncollected(NativeUncollectedCall {
                    scope: self.clone(),
                    cause: Some(cause),
                    _retained_value: retained_value,
                })
            }
        }
    }
    pub(super) async fn spawn(
        &self,
        mut command: Command,
        args: &[String],
        environment: &[(&str, &str)],
    ) -> Result<(Io, Instant)> {
        {
            let mut state = self.state();
            anyhow::ensure!(
                state.settled && !state.unknown,
                "previous native control IO remains owned and uncollected"
            );
            state.settled = false;
        }
        let scope = self.clone();
        let args = args.to_vec();
        let environment: Vec<_> = environment
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let runtime = self.0.runtime.clone();
        let current = match tokio::runtime::Handle::try_current() {
            Ok(current) => current,
            Err(error) => {
                self.state().settled = true;
                return Err(anyhow::Error::new(error)
                    .context("native control spawn requires its owning runtime"));
            }
        };
        let entered = current.clone();
        let handle = current.spawn_blocking(move || {
            let _entered = entered.enter();
            if let Err(error) = runtime.anchor.verify(&runtime.invocation, runtime.kind) {
                scope.state().settled = true;
                return Err(error);
            }
            let trace = if args.first().map(String::as_str) == Some("kill") && args.len() == 2 { runtime.trace_target(&args[1]) } else { None };
            observation::record(&trace, observation::Phase::StopSpawnRequested);
            let child = match command.args(args).envs(environment).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
                Ok(child) => child,
                Err(error) => {
                    scope.state().settled = true;
                    return Err(anyhow::Error::new(error).context(format!("{} is unavailable; install and start a container runtime. No local fallback exists", runtime.kind)));
                }
            };
            let armed = Instant::now();
            { let mut state = scope.state(); state.child = Some(child); state.emitted = true; }
            observation::record(&trace, observation::Phase::StopSpawnReturned);
            let child = scope.state().child.take().ok_or_else(|| anyhow::anyhow!("native spawned child custody is unavailable"))?;
            let io = Arc::new(tokio::sync::Mutex::new(batch_io::OwnedBatchIo::own_cli_retained_observed(child, 256 * 1024, 64 * 1024, trace)));
            scope.state().io = Some(io);
            Ok(armed)
        });
        let spawn = Arc::new(tokio::sync::Mutex::new(SpawnTask {
            handle,
            failed: false,
            returned: false,
        }));
        self.state().spawn = Some(Arc::clone(&spawn));
        let armed = self.wait_spawn(&spawn).await?;
        let io = self
            .state()
            .io
            .clone()
            .ok_or_else(|| anyhow::anyhow!("native control IO was not installed"))?;
        Ok((io, armed))
    }
    async fn wait_spawn(&self, spawn: &Spawn) -> Result<Instant> {
        let mut task = spawn.lock().await;
        anyhow::ensure!(
            !task.failed && !task.returned,
            "native spawn join remains unknown or was already consumed"
        );
        match (&mut task.handle).await {
            Ok(result) => {
                task.returned = true;
                self.state().spawn = None;
                result
            }
            Err(error) => {
                task.failed = true;
                self.state().unknown = true;
                Err(anyhow::Error::new(error)
                    .context("native control spawn join failed; original custody retained"))
            }
        }
    }
    pub(super) fn collected(&self) {
        let mut state = self.state();
        state.io = None;
        state.settled = true;
    }
    pub(super) fn unknown(&self) {
        self.state().unknown = true;
    }
}

pub(crate) struct NativeUncollectedCall {
    scope: NativeControlScope,
    cause: Option<anyhow::Error>,
    _retained_value: Option<Box<dyn Any + Send + Sync>>,
}
impl fmt::Debug for NativeUncollectedCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeUncollectedCall")
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}
impl NativeUncollectedCall {
    /// Private inspection only; never format captured data into status/audit.
    /// The borrow cannot move or release the actual native IO owner.
    pub(crate) fn cause(&self) -> Option<&anyhow::Error> {
        self.cause.as_ref()
    }
    pub(crate) fn phase(&self) -> NativeIoPhase {
        self.scope.phase()
    }
    pub(crate) async fn collect(&mut self) -> NativeCollectionOutcome {
        if self.cause.is_none() || self.scope.phase() == NativeIoPhase::Unknown {
            return NativeCollectionOutcome::PendingOrUnknown;
        }
        let spawn = self.scope.state().spawn.clone();
        if let Some(spawn) = spawn {
            if let Err(collection) = self.scope.wait_spawn(&spawn).await {
                if let Some(first) = self.cause.take() {
                    self.cause = Some(anyhow::Error::new(CollectionFailure {
                        first,
                        _collection: collection,
                    }));
                }
                if self.scope.phase() == NativeIoPhase::Unknown {
                    return NativeCollectionOutcome::PendingOrUnknown;
                }
            }
        }
        let io = self.scope.state().io.clone();
        if let Some(io) = io {
            let mut owned = io.lock().await;
            match owned.collect().await {
                Ok(collected) => {
                    let Some(cause) = self.cause.take() else {
                        return NativeCollectionOutcome::PendingOrUnknown;
                    };
                    self.scope.collected();
                    return NativeCollectionOutcome::Collected {
                        result: Err(cause.context(CliEvidence(collected))),
                        proof: NativeCollected {
                            _scope: self.scope.0.id,
                        },
                    };
                }
                Err(batch_io::BatchIoCollectError::NoRuntime) => {
                    return NativeCollectionOutcome::PendingOrUnknown;
                }
                Err(_) => {
                    self.scope.unknown();
                    return NativeCollectionOutcome::PendingOrUnknown;
                }
            }
        }
        let Some(cause) = self.cause.take() else {
            return NativeCollectionOutcome::PendingOrUnknown;
        };
        match self.scope.phase() {
            NativeIoPhase::NoChild => NativeCollectionOutcome::NoChild {
                result: Err(cause),
                proof: NativeNoChild {
                    _scope: self.scope.0.id,
                },
            },
            NativeIoPhase::Collected => NativeCollectionOutcome::Collected {
                result: Err(cause),
                proof: NativeCollected {
                    _scope: self.scope.0.id,
                },
            },
            _ => {
                self.cause = Some(cause);
                NativeCollectionOutcome::PendingOrUnknown
            }
        }
    }
}

#[cfg(test)]
#[path = "control_outcome_tests.rs"]
mod tests;
