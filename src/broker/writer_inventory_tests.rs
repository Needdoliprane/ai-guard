//! The pause is the existing queue harness; journal reads use real files.
use super::tests::{paused_client, private_journal};
use super::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Wake, Waker},
};

struct InventoryReplyWake {
    pending: Arc<AtomicBool>,
    observed: mpsc::SyncSender<JournalResult<()>>,
}

impl Wake for InventoryReplyWake {
    fn wake(self: Arc<Self>) {
        let reacquired = InventoryPermit::acquire(&self.pending).map(drop);
        let _ = self.observed.try_send(reacquired);
    }
}

#[test]
fn completed_inventory_releases_its_permit_before_reply() {
    let (_root, journal) = private_journal();
    let (writer, receiver) = paused_client(&journal, 4, Duration::from_secs(5));
    let pending = Arc::clone(&writer.0.inventory_pending);
    let permit = InventoryPermit::acquire(&pending).unwrap();
    let (reply, mut receive) = oneshot::channel();
    let (observed, observation) = mpsc::sync_channel(1);
    let waker = Waker::from(Arc::new(InventoryReplyWake { pending, observed }));
    assert!(
        Pin::new(&mut receive)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert!(
        writer
            .0
            .sender
            .try_send(Command::RecoveryInventory {
                after: None,
                limit: 20,
                reply,
                permit,
            })
            .is_ok()
    );
    let worker = std::thread::spawn(move || serve(journal, receiver));
    let reacquired = observation.recv_timeout(Duration::from_secs(5)).unwrap();
    let page = receive.try_recv().unwrap().unwrap();
    drop(writer);
    worker.join().unwrap();
    assert!(page.entries().is_empty());
    assert!(
        reacquired.is_ok(),
        "the permit must be available when the reply wakes its caller: {reacquired:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn timed_out_inventory_keeps_one_permit_and_cannot_queue_a_series_before_cancel() {
    let (_root, journal) = private_journal();
    let (writer, receiver) = paused_client(&journal, 4, Duration::from_millis(10));
    let stops = writer.stops();
    assert!(matches!(
        stops.recovery_inventory(None, 20).await,
        Err(JournalError::Io)
    ));
    assert!(
        matches!(
            stops.recovery_inventory(None, 20).await,
            Err(JournalError::Busy)
        ),
        "timeout must not release a scan still owned by the queue"
    );
    let operation = Uuid::new_v4();
    let command = Uuid::new_v4();
    let failure = stops
        .append_cancel(CancelInputV1::new(operation, command).unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(failure, CompletionFailure::Unknown { .. }),
        "Cancel entered the same queue after one inventory"
    );
    let first = receiver.try_recv().unwrap();
    assert!(matches!(&first, Command::RecoveryInventory { .. }));
    let second = receiver.try_recv().unwrap();
    assert!(
        matches!(&second, Command::AppendCancel { input, .. } if input.command_id() == command)
    );
    assert!(
        receiver.try_recv().is_err(),
        "no second inventory can occupy the queue"
    );
    assert!(writer.0.sender.try_send(first).is_ok());
    assert!(writer.0.sender.try_send(second).is_ok());
    let recovered_writer = WriterClient(Arc::new(ClientState {
        sender: writer.0.sender.clone(),
        scope: writer.0.scope.clone(),
        timeout: Duration::from_secs(2),
        inventory_pending: Arc::clone(&writer.0.inventory_pending),
        retirements: Arc::clone(&writer.0.retirements),
        status_cache: Arc::clone(&writer.0.status_cache),
        outstanding: Arc::clone(&writer.0.outstanding),
    }));
    let worker = std::thread::spawn(move || serve(journal, receiver));
    // This FIFO barrier is behind the timed-out inventory and Cancel. It
    // proves real worker processing before checking permit reuse.
    assert!(recovered_writer.status(operation).await.unwrap().is_none());
    assert!(
        recovered_writer
            .stops()
            .recovery_inventory(None, 20)
            .await
            .unwrap()
            .entries()
            .is_empty()
    );
    drop((stops, writer, recovered_writer));
    worker.join().unwrap();
}
