//! Nonblocking observations on one exact Store instance, for execution tests.
//!
//! There is no wait, callback or altered Store result in the observed path.
//! An unbounded channel send never waits for the receiving test. Tests attach
//! only after setup and consume the finite observations after each operation.

use super::Store;
use std::{
    sync::{OnceLock, mpsc},
    thread::{self, ThreadId},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Call {
    BeginChecked,
    Integrity,
    Metadata,
}

#[derive(Debug)]
pub(crate) struct Observation {
    pub(crate) call: Call,
    pub(crate) thread: ThreadId,
}

#[derive(Default)]
pub(super) struct ReadTestObserver {
    sender: OnceLock<mpsc::Sender<Observation>>,
}

impl ReadTestObserver {
    pub(super) fn record(&self, call: Call) {
        if let Some(sender) = self.sender.get() {
            let _ = sender.send(Observation {
                call,
                thread: thread::current().id(),
            });
        }
    }
}

pub(crate) fn observe(store: &Store) -> mpsc::Receiver<Observation> {
    let (sender, receiver) = mpsc::channel();
    assert!(
        store.read_test_observer.sender.set(sender).is_ok(),
        "a Store execution observer must be attached only once"
    );
    receiver
}
