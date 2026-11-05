use super::*;
use std::sync::mpsc::TrySendError;

#[derive(Debug)]
struct Item(&'static str);

#[test]
fn completion_reservation_uses_one_slot_after_a_full_fifo_without_expanding_ordinary_capacity() {
    let (sender, receiver) = bounded::<Item>(2).expect("capacity two is valid");
    sender.try_send(Item("first")).expect("first ordinary slot");
    sender
        .try_send(Item("second"))
        .expect("second ordinary slot");

    let permit = sender
        .try_reserve_completion()
        .expect("completion slot remains available when ordinary slots are full");
    assert!(matches!(
        sender.try_send(Item("ordinary-overflow")),
        Err(TrySendError::Full(Item("ordinary-overflow")))
    ));
    permit
        .try_send(Item("completion"))
        .expect("reserved completion is enqueued without reacquiring capacity");

    assert!(matches!(receiver.recv(), Ok(Item("first"))));
    assert!(matches!(receiver.recv(), Ok(Item("second"))));
    assert!(matches!(receiver.recv(), Ok(Item("completion"))));
}

#[test]
fn receiving_releases_an_ordinary_slot_before_returning_the_retained_value() {
    let (sender, receiver) = bounded::<Item>(1).expect("capacity one is valid");
    sender.try_send(Item("retained-by-caller")).unwrap();

    let retained = receiver.recv().unwrap();
    sender
        .try_send(Item("next"))
        .expect("dequeue releases queue capacity even while the caller retains its value");

    assert!(matches!(retained, Item("retained-by-caller")));
    assert!(matches!(receiver.recv(), Ok(Item("next"))));
}

#[test]
fn completion_permit_is_single_use_and_drop_or_disconnect_preserves_values() {
    let (sender, receiver) = bounded::<Item>(1).expect("capacity one is valid");
    let clone = sender.clone();

    let permit = sender.try_reserve_completion().expect("one reservation");
    assert!(matches!(
        clone.try_reserve_completion(),
        Err(ReserveError::Full)
    ));
    drop(permit);
    let permit = clone
        .try_reserve_completion()
        .expect("dropping a permit releases its reserved slot");
    drop(receiver);

    assert!(matches!(
        permit.try_send(Item("completion-after-close")),
        Err(TrySendError::Disconnected(Item("completion-after-close")))
    ));
    assert!(matches!(
        sender.try_send(Item("ordinary-after-close")),
        Err(TrySendError::Disconnected(Item("ordinary-after-close")))
    ));
    assert!(matches!(
        clone.try_reserve_completion(),
        Err(ReserveError::Disconnected)
    ));
}

#[test]
fn ordinary_capacity_rejects_zero_and_values_above_the_bound() {
    assert!(matches!(
        bounded::<Item>(0),
        Err(QueueConfigError::InvalidCapacity)
    ));
    assert!(matches!(
        bounded::<Item>(257),
        Err(QueueConfigError::InvalidCapacity)
    ));
    assert!(bounded::<Item>(1).is_ok());
    assert!(bounded::<Item>(256).is_ok());
}
