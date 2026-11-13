//! Software cancellation of the actual Router future, not a TCP disconnect.
//! Requires the frozen A0 business hook when Root composes this test source.
use super::*;
use crate::api::{auth::owned_control::test_hook, store_io};
use crate::control::ControlProtocolV1;
use std::sync::{atomic::Ordering, mpsc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower::ServiceExt;

const OBSERVATION_BUDGET: Duration = Duration::from_secs(120);

/// Obtain the actual Gate semaphore from an actual admitted permit. This uses
/// Tokio's existing accessor, without exposing or substituting a Gate field.
fn control_semaphore(gate: &Arc<store_io::Gate>) -> Arc<Semaphore> {
    let permit = gate.admit(store_io::Lane::Control, false).unwrap();
    let semaphore = Arc::clone(permit.semaphore());
    drop(permit);
    semaphore
}

/// These permits belong to the test, not to the HTTP job. Leave one actual
/// slot for that job and keep all other slots occupied until its return.
fn leave_one(semaphore: &Arc<Semaphore>) -> (usize, OwnedSemaphorePermit) {
    let capacity = semaphore.available_permits();
    assert!(capacity > 0, "fresh fixture must have an available real slot");
    let remaining = u32::try_from(capacity - 1).unwrap();
    let reserved = Arc::clone(semaphore).try_acquire_many_owned(remaining).unwrap();
    assert_eq!(semaphore.available_permits(), 1);
    (capacity, reserved)
}

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}

struct RouterFutureDropped(Option<oneshot::Sender<()>>);
impl Drop for RouterFutureDropped {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() { let _ = sender.send(()); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborted_router_keeps_owned_usage_and_real_permits_until_postcommit_release_and_drain() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle, credentials, tokens, Policy::default(), None, Redactor::new(vec![]),
    ).unwrap();
    let owned = state.owned_human.as_ref().unwrap();
    let a = state.tenants.iter().position(|tenant| tenant.tenant_id == "A").unwrap();
    let namespace = owned.sources[a].descriptor().durable_namespace_id.to_string();
    let router = gateway_router(state.clone());
    let request = || axum::http::Request::builder().method("GET").uri(RESERVED_PROTOCOL)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {RUNNER}"))
        .body(Body::empty()).unwrap();

    // This genuine200 witness refuses broken setup as a cancellation success.
    // The same migration/startup, retained registry, middleware and Store will
    // handle the request paused below. No response or actor proof is mocked.
    let response = tokio::time::timeout(OBSERVATION_BUDGET, router.clone().oneshot(request()))
        .await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let protocol: ControlProtocolV1 = serde_json::from_slice(&bytes).unwrap();
    protocol.validate().unwrap();
    assert_eq!(protocol.schema_version, 14);
    assert_eq!(protocol.tenant_id, "A");
    assert_eq!(protocol.role, "runner");
    assert_eq!(protocol.control_namespace_uuid.as_deref(), Some(namespace.as_str()));
    assert!(!protocol.active, "diagnostic acceptance must not activate a lease");
    assert_eq!(owned.usages.load(Ordering::Acquire), 0);

    let shared_io = control_semaphore(&state.io);
    let tenant_io = control_semaphore(&state.tenants[a].io);
    let operation = state.tenants[a].control_capacity.runner_semaphore_for_test();
    assert!(!Arc::ptr_eq(&shared_io, &tenant_io));
    let (shared_capacity, shared_reserved) = leave_one(&shared_io);
    let (tenant_capacity, tenant_reserved) = leave_one(&tenant_io);
    let (operation_capacity, operation_reserved) = leave_one(&operation);
    let (reached, release) = test_hook::install(&state);
    let (dropped, drop_observed) = oneshot::channel();
    let actual_request = request();
    let handler = tokio::spawn(async move {
        let _drop = RouterFutureDropped(Some(dropped));
        // This is the actual server-side Router future, including the real
        // reserved transport and bearer middleware. It is not a reqwest task.
        router.oneshot(actual_request).await
    });
    let _abort_on_failure = AbortOnDrop(handler.abort_handle());
    let reached = tokio::time::timeout(OBSERVATION_BUDGET, tokio::task::spawn_blocking(move || {
        reached.recv_timeout(OBSERVATION_BUDGET).unwrap();
        reached
    })).await.unwrap().unwrap();
    // Reached is emitted only after the actual checked Business read committed
    // and released SQLite, while its usage and operation/IO permits are held.
    assert_eq!(owned.usages.load(Ordering::Acquire), 1);
    assert_eq!(shared_io.available_permits(), 0);
    assert_eq!(tenant_io.available_permits(), 0);
    assert_eq!(operation.available_permits(), 0);

    handler.abort();
    assert!(handler.await.unwrap_err().is_cancelled());
    tokio::time::timeout(OBSERVATION_BUDGET, drop_observed).await.unwrap().unwrap();
    assert_eq!(owned.usages.load(Ordering::Acquire), 1,
        "dropping the Router observer must not release accepted running work");

    let shared_return = Arc::clone(&shared_io).acquire_owned();
    let tenant_return = Arc::clone(&tenant_io).acquire_owned();
    let operation_return = Arc::clone(&operation).acquire_owned();
    tokio::pin!(shared_return, tenant_return, operation_return);
    assert!(futures_util::poll!(shared_return.as_mut()).is_pending());
    assert!(futures_util::poll!(tenant_return.as_mut()).is_pending());
    assert!(futures_util::poll!(operation_return.as_mut()).is_pending());

    let retirement = state.human_retirement().unwrap();
    retirement.retire();
    let drained = retirement.drain();
    tokio::pin!(drained);
    assert!(futures_util::poll!(drained.as_mut()).is_pending());
    assert_eq!(owned.usages.load(Ordering::Acquire), super::super::CLOSED | 1);
    assert!(matches!(reached.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "the one-shot postcommit rendez-vous must still be held before release");

    // No sleep and no assumption that the retained2s epoch is still fresh.
    // This observer was already dropped, so no successful response is expected.
    release.release();
    let returned = tokio::time::timeout(OBSERVATION_BUDGET, async {
        let (shared, tenant, operation, drained) = tokio::join!(
            shared_return.as_mut(), tenant_return.as_mut(), operation_return.as_mut(), drained.as_mut(),
        );
        drained.unwrap();
        (shared.unwrap(), tenant.unwrap(), operation.unwrap())
    }).await.expect("actual work, permits and owner must finish within the QA observation budget");
    assert_eq!(owned.usages.load(Ordering::Acquire), super::super::CLOSED);
    assert!(matches!(reached.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
    drop(returned);
    drop((shared_reserved, tenant_reserved, operation_reserved));
    assert_eq!(shared_io.available_permits(), shared_capacity);
    assert_eq!(tenant_io.available_permits(), tenant_capacity);
    assert_eq!(operation.available_permits(), operation_capacity);
}
