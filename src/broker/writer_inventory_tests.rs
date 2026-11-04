//! The pause is the existing queue harness; journal reads use real files.
use super::tests::{paused_client, private_journal};
use super::*;

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
