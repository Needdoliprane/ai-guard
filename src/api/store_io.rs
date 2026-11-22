//! Bounded blocking work; cancellation never releases a running task's slots.
use super::*;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lane {
    Read,
    Control,
    Discovery,
}

pub(super) struct Gate {
    reads: Arc<Semaphore>,
    control: Arc<Semaphore>,
    discovery: Arc<Semaphore>,
    frequency: Mutex<(Instant, u32)>,
}
impl Gate {
    pub(super) fn new(shared: bool) -> Self {
        Self {
            reads: Arc::new(Semaphore::new(if shared { 32 } else { 4 })),
            // Separate from viewers, including runner polling bursts. These
            // are admission bounds, not a qualification of 100 workers.
            control: Arc::new(Semaphore::new(128)),
            discovery: Arc::new(Semaphore::new(if shared { 16 } else { 4 })),
            frequency: Mutex::new((Instant::now(), 0)),
        }
    }
    pub(super) fn admit(&self, lane: Lane, expensive: bool) -> ApiResult<OwnedSemaphorePermit> {
        self.admit_at(lane, expensive, Instant::now())
    }
    fn admit_at(
        &self,
        lane: Lane,
        expensive: bool,
        now: Instant,
    ) -> ApiResult<OwnedSemaphorePermit> {
        let slots = match lane {
            Lane::Read => &self.reads,
            Lane::Control => &self.control,
            Lane::Discovery => &self.discovery,
        };
        let permit = slots.clone().try_acquire_owned().map_err(|_| capacity())?;
        if lane == Lane::Read && expensive {
            let mut rate = self
                .frequency
                .lock()
                .map_err(|_| unavailable(anyhow::anyhow!("read admission unavailable")))?;
            if now.saturating_duration_since(rate.0) >= Duration::from_secs(1) {
                *rate = (now, 0);
            }
            if rate.1 >= 200 {
                return Err(capacity());
            }
            rate.1 += 1;
        }
        Ok(permit)
    }
}

/// Private evidence for one exact Store, admission gate and work class.
/// A scope lives only in a synchronous blocking segment, never in a returned
/// proof, session or slow HTTP body. Unknown neighboring Stores use temporary
/// leases; known source/context Stores retain and reuse their leases.
struct StoreLease {
    store: TenantStore,
    gate: Arc<Gate>,
    lane: Lane,
    _permit: OwnedSemaphorePermit,
}
#[derive(Default)]
pub(super) struct Scope {
    held: Vec<Arc<StoreLease>>,
    _shared_discovery: Option<(Arc<Gate>, OwnedSemaphorePermit)>,
}
impl Scope {
    #[cfg(feature = "mysql-storage")]
    pub(super) fn admit_peer_discovery(
        &self,
        shared: &Arc<Gate>,
        tenant: &TenantState,
    ) -> ApiResult<(Option<OwnedSemaphorePermit>, OwnedSemaphorePermit)> {
        let shared_permit = match &self._shared_discovery {
            Some((gate, _)) if Arc::ptr_eq(gate, shared) => None,
            _ => Some(shared.admit(Lane::Discovery, false)?),
        };
        let tenant_permit = tenant.io.admit(Lane::Discovery, false)?;
        Ok((shared_permit, tenant_permit))
    }

    pub(super) fn hold(
        &mut self,
        tenant: &TenantState,
        lane: Lane,
        expensive: bool,
    ) -> ApiResult<()> {
        if self.find(tenant, lane).is_none() {
            self.held.push(Self::acquire(tenant, lane, expensive)?);
        }
        Ok(())
    }
    fn find(&self, tenant: &TenantState, lane: Lane) -> Option<Arc<StoreLease>> {
        self.held
            .iter()
            .find(|lease| {
                lease.lane == lane
                    && lease.store.ptr_eq(&tenant.store)
                    && Arc::ptr_eq(&lease.gate, &tenant.io)
            })
            .cloned()
    }
    fn acquire(tenant: &TenantState, lane: Lane, expensive: bool) -> ApiResult<Arc<StoreLease>> {
        Ok(Arc::new(StoreLease {
            store: tenant.store.clone(),
            gate: Arc::clone(&tenant.io),
            lane,
            _permit: tenant.io.admit(lane, expensive)?,
        }))
    }
    pub(super) fn with_store<T>(
        &self,
        tenant: &TenantState,
        lane: Lane,
        expensive: bool,
        operation: impl FnOnce(&Store) -> T,
    ) -> ApiResult<T> {
        let lease = match self.find(tenant, lane) {
            Some(lease) => lease,
            None => Self::acquire(tenant, lane, expensive)?,
        };
        Ok(operation(lease.store.sqlite_api()?))
    }
}
pub(super) fn capacity() -> (StatusCode, Json<Value>) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error":{
            "code":"capacity_exhausted", "message":"Capacité de lecture ou de contrôle atteinte ; réessayer.", "retry_action":"retry"
        }})),
    )
}

/// A cleanup obligation, reserved before creating authority that needs fencing.
/// Its slot includes queued blocking work. A retained handle cannot promise a
/// terminal write during runtime shutdown: rejected jobs have no synchronous
/// I/O fallback, and their pending/rotating authority must remain unusable.
pub(super) struct CleanupReservation {
    runtime: tokio::runtime::Handle,
    permit: OwnedSemaphorePermit,
}
impl CleanupReservation {
    pub(super) fn reserve(slots: &Arc<Semaphore>) -> ApiResult<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| unavailable(anyhow::anyhow!("cleanup runtime unavailable")))?;
        let permit = slots.clone().try_acquire_owned().map_err(|_| capacity())?;
        Ok(Self { runtime, permit })
    }
    pub(super) fn dispatch<T: Send + 'static>(
        self,
        cleanup: impl FnOnce() -> T + Send + 'static,
    ) -> tokio::task::JoinHandle<T> {
        let Self { runtime, permit } = self;
        runtime.spawn_blocking(move || {
            let _permit = permit;
            cleanup()
        })
    }
}

/// Drop the operation, then the I/O permits, then the retained guard. This
/// order also applies if Tokio drops the work before running it.
struct BlockingStoreWork<F, P, R> {
    operation: F,
    _permits: P,
    _retained: R,
}

impl<F, P, R> BlockingStoreWork<F, P, R> {
    fn run<T>(self) -> ApiResult<T>
    where
        F: FnOnce() -> ApiResult<T>,
    {
        (self.operation)()
    }
}

pub(super) async fn run<T, F>(
    shared: Arc<Gate>,
    tenant: Option<Arc<Gate>>,
    lane: Lane,
    expensive: bool,
    operation: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    run_retaining(shared, tenant, lane, expensive, (), operation).await
}

/// Retain a guard until both the operation and its I/O permits are released.
pub(super) async fn run_retaining<T, F, R>(
    shared: Arc<Gate>,
    tenant: Option<Arc<Gate>>,
    lane: Lane,
    expensive: bool,
    retained: R,
    operation: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
    R: Send + 'static,
{
    // Never enqueue a waiter for admission. Only admitted tasks reach Tokio's
    // blocking pool, and both bounds survive a dropped HTTP request future.
    let shared_permit = shared.admit(lane, false)?;
    let tenant_permit = tenant.map(|gate| gate.admit(lane, expensive)).transpose()?;
    let work = BlockingStoreWork {
        operation,
        _permits: (shared_permit, tenant_permit),
        _retained: retained,
    };
    run_blocking(move || work.run()).await
}

pub(super) async fn run_discovery<T, F, R>(
    shared: Arc<Gate>,
    retained: R,
    operation: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<Scope>) -> ApiResult<T> + Send + 'static,
    R: Send + 'static,
{
    let permit = shared.admit(Lane::Discovery, false)?;
    // The blocking segment can reuse this shared slot for MySQL peer work.
    // Keep the scope through operation drop and release it before owned usage.
    let scope = Arc::new(Scope {
        _shared_discovery: Some((shared, permit)),
        ..Scope::default()
    });
    let operation_scope = Arc::clone(&scope);
    let work = BlockingStoreWork {
        operation: move || operation(operation_scope),
        _permits: scope,
        _retained: retained,
    };
    run_blocking(move || work.run()).await
}

async fn run_blocking<T, F>(operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| unavailable(anyhow::anyhow!("blocking Store task failed")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_slot_covers_the_queue_and_outlives_a_cancelled_waiter() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let slots = Arc::new(Semaphore::new(1));
            let (occupied, occupied_ready) = tokio::sync::oneshot::channel();
            let (release_blocker, blocker) = std::sync::mpsc::channel();
            let blocking_worker = tokio::task::spawn_blocking(move || {
                occupied.send(()).unwrap();
                blocker.recv().unwrap();
            });
            occupied_ready.await.unwrap();
            let reservation = CleanupReservation::reserve(&slots).unwrap();
            let (started, started_ready) = tokio::sync::oneshot::channel();
            let (release_cleanup, cleanup) = std::sync::mpsc::channel();
            let job = reservation.dispatch(move || {
                started.send(()).unwrap();
                cleanup.recv().unwrap();
            });
            // The sole blocking worker is occupied, so the cleanup is queued.
            assert!(matches!(
                CleanupReservation::reserve(&slots),
                Err((StatusCode::TOO_MANY_REQUESTS, _))
            ));
            let (waiting, waiting_ready) = tokio::sync::oneshot::channel();
            let waiter = tokio::spawn(async move {
                waiting.send(()).unwrap();
                job.await.unwrap();
            });
            waiting_ready.await.unwrap();
            waiter.abort();
            assert!(waiter.await.unwrap_err().is_cancelled());
            assert!(matches!(
                CleanupReservation::reserve(&slots),
                Err((StatusCode::TOO_MANY_REQUESTS, _))
            ));
            release_blocker.send(()).unwrap();
            blocking_worker.await.unwrap();
            started_ready.await.unwrap();
            assert!(matches!(
                CleanupReservation::reserve(&slots),
                Err((StatusCode::TOO_MANY_REQUESTS, _))
            ));
            release_cleanup.send(()).unwrap();
            drop(slots.clone().acquire_owned().await.unwrap());
            drop(CleanupReservation::reserve(&slots).unwrap());
        });
    }

    #[tokio::test]
    async fn cancelled_read_keeps_its_slot_and_control_remains_admissible() {
        let shared = Arc::new(Gate::new(true));
        let tenant = Arc::new(Gate::new(false));
        let _other_readers: Vec<_> = (0..3)
            .map(|_| tenant.admit(Lane::Read, false).unwrap())
            .collect();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, finish) = std::sync::mpsc::channel();
        let task_shared = Arc::clone(&shared);
        let task_tenant = Arc::clone(&tenant);
        let request = tokio::spawn(run(
            task_shared,
            Some(task_tenant),
            Lane::Read,
            true,
            move || {
                started.send(()).unwrap();
                finish.recv().unwrap();
                Ok(())
            },
        ));
        ready.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let rejected = run(
            Arc::clone(&shared),
            Some(Arc::clone(&tenant)),
            Lane::Read,
            true,
            || Ok(()),
        )
        .await;
        assert_eq!(rejected.unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        run(
            Arc::clone(&shared),
            Some(Arc::clone(&tenant)),
            Lane::Control,
            true,
            || Ok(()),
        )
        .await
        .unwrap();
        release.send(()).unwrap();
        // Synchronize on actual permit release, with no latency threshold.
        drop(tenant.reads.clone().acquire_owned().await.unwrap());
        run(shared, Some(tenant), Lane::Read, true, || Ok(()))
            .await
            .unwrap();
    }

    #[test]
    fn read_frequency_budget_does_not_consume_the_control_reserve() {
        let gate = Gate::new(false);
        let instant = Instant::now();
        for _ in 0..200 {
            drop(gate.admit_at(Lane::Read, true, instant).unwrap());
        }
        assert_eq!(
            gate.admit_at(Lane::Read, true, instant).unwrap_err().0,
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(gate.admit_at(Lane::Control, true, instant).unwrap());
        let other_tenant = Gate::new(false);
        drop(other_tenant.admit_at(Lane::Read, true, instant).unwrap());
        drop(
            gate.admit_at(Lane::Read, true, instant + Duration::from_secs(1))
                .unwrap(),
        );
    }
}
