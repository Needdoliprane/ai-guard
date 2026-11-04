//! Explicitly scoped test measurements. Inclusive spans can overlap; only the
//! SyncAll spans surround the exact std File::sync_all calls. No paths or file
//! identities leave the active collector, and ordinary tests never activate it.

use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Read,
    ReadToEnd,
    Revalidate,
    ReadAt,
    DirectoryRevalidate,
    DirectorySync,
    DirectorySyncAll,
    FileSync,
    FileSyncAll,
    Create,
    Publish,
}

impl Operation {
    const ALL: [Self; 11] = [
        Self::Read,
        Self::ReadToEnd,
        Self::Revalidate,
        Self::ReadAt,
        Self::DirectoryRevalidate,
        Self::DirectorySync,
        Self::DirectorySyncAll,
        Self::FileSync,
        Self::FileSyncAll,
        Self::Create,
        Self::Publish,
    ];
}

#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Metric {
    pub(crate) calls: u64,
    pub(crate) succeeded: u64,
    pub(crate) failed: u64,
    pub(crate) abandoned: u64,
    pub(crate) bytes: u64,
    pub(crate) elapsed_ns: u64,
}

#[derive(Serialize)]
struct Sample {
    operation: Operation,
    #[serde(flatten)]
    metric: Metric,
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    pub(crate) outstanding: u64,
    samples: Vec<Sample>,
}

impl Snapshot {
    pub(crate) fn metric(&self, operation: Operation) -> Metric {
        self.samples[operation as usize].metric
    }
}

struct Active {
    token: u64,
    root: PathBuf,
    metrics: [Metric; Operation::ALL.len()],
    outstanding: u64,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Test harness ownership around a complete scoped measurement. Async callers
/// await this gate; synchronous #[test] callers may use blocking_lock. Keep the
/// guard until after Measurement::finish or Drop has disarmed the collector.
/// This does not change start's explicit AlreadyActive error or create another
/// collector, and never exists in non-test builds.
pub(crate) static EXCLUSIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug)]
pub(crate) enum ProbeError {
    AlreadyActive,
    InvalidRoot,
    Poisoned,
    ScopeLost,
    TokenExhausted,
}

/// One private fixture and one awaited operation at a time. Drop also disarms
/// the collector after an assertion failure or cancellation of the caller.
pub(crate) struct Measurement {
    token: u64,
    finished: bool,
}

impl Measurement {
    pub(crate) fn start(root: &Path) -> Result<Self, ProbeError> {
        if !root.is_absolute() {
            return Err(ProbeError::InvalidRoot);
        }
        let mut active = ACTIVE.lock().map_err(|_| ProbeError::Poisoned)?;
        if active.is_some() {
            return Err(ProbeError::AlreadyActive);
        }
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        if token == u64::MAX || token == 0 {
            return Err(ProbeError::TokenExhausted);
        }
        *active = Some(Active {
            token,
            root: root.to_owned(),
            metrics: [Metric::default(); Operation::ALL.len()],
            outstanding: 0,
        });
        Ok(Self {
            token,
            finished: false,
        })
    }

    pub(crate) fn finish(mut self) -> Result<Snapshot, ProbeError> {
        let mut active = ACTIVE.lock().map_err(|_| ProbeError::Poisoned)?;
        if active.as_ref().map(|value| value.token) != Some(self.token) {
            return Err(ProbeError::ScopeLost);
        }
        let collected = active.take().ok_or(ProbeError::ScopeLost)?;
        self.finished = true;
        Ok(Snapshot {
            outstanding: collected.outstanding,
            samples: Operation::ALL
                .into_iter()
                .map(|operation| Sample {
                    operation,
                    metric: collected.metrics[operation as usize],
                })
                .collect(),
        })
    }
}

impl Drop for Measurement {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut active) = ACTIVE.lock()
            && active.as_ref().map(|value| value.token) == Some(self.token)
        {
            active.take();
        }
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Succeeded,
    Failed,
    Abandoned,
}

/// Created only after a matching active scope was found. An unfinished span is
/// recorded as abandoned, never as a successful operation.
pub(crate) struct Span {
    token: u64,
    operation: Operation,
    started: Instant,
    outcome: Outcome,
    bytes: u64,
    completed_elapsed_ns: Option<u64>,
}

impl Span {
    pub(crate) fn start(operation: Operation, path: &Path) -> Option<Self> {
        let mut active = ACTIVE.lock().ok()?;
        let selected = active.as_mut()?;
        if !path.starts_with(&selected.root) {
            return None;
        }
        selected.metrics[operation as usize].calls += 1;
        selected.outstanding += 1;
        Some(Self {
            token: selected.token,
            operation,
            started: Instant::now(),
            outcome: Outcome::Abandoned,
            bytes: 0,
            completed_elapsed_ns: None,
        })
    }

    fn finish(mut self, succeeded: bool, bytes: u64, elapsed_ns: u64) {
        self.outcome = if succeeded {
            Outcome::Succeeded
        } else {
            Outcome::Failed
        };
        self.bytes = bytes;
        self.completed_elapsed_ns = Some(elapsed_ns);
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let elapsed_ns = self.completed_elapsed_ns.unwrap_or_else(|| {
            u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX)
        });
        let Ok(mut active) = ACTIVE.lock() else {
            return;
        };
        let Some(selected) = active.as_mut().filter(|value| value.token == self.token) else {
            return;
        };
        selected.outstanding = selected.outstanding.saturating_sub(1);
        let metric = &mut selected.metrics[self.operation as usize];
        metric.elapsed_ns = metric.elapsed_ns.saturating_add(elapsed_ns);
        metric.bytes = metric.bytes.saturating_add(self.bytes);
        match self.outcome {
            Outcome::Succeeded => metric.succeeded += 1,
            Outcome::Failed => metric.failed += 1,
            Outcome::Abandoned => metric.abandoned += 1,
        }
    }
}

/// Executes the original body once. The completed duration stops immediately
/// after its return, before result classification, byte accounting or locking
/// the collector. A panic drops the span as abandoned instead of inventing Err.
pub(crate) fn measure<T, E>(
    operation: Operation,
    path: &Path,
    body: impl FnOnce() -> Result<T, E>,
    returned_bytes: impl FnOnce(&Result<T, E>) -> u64,
) -> Result<T, E> {
    let Some(span) = Span::start(operation, path) else {
        return body();
    };
    let started = Instant::now();
    let result = body();
    let elapsed_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    span.finish(result.is_ok(), returned_bytes(&result), elapsed_ns);
    result
}
