use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Copy)]
enum MarkerFault {
    Error,
    NoAdvance,
}

struct FixtureSpool {
    event: Option<SpooledEvent>,
    delivered: AtomicU64,
    marker_fault: Mutex<Option<MarkerFault>>,
    cursor_unavailable: AtomicBool,
}

impl FixtureSpool {
    fn with_event() -> Self {
        Self {
            event: Some(SpooledEvent {
                seq: 1,
                bytes: 128,
                event: Event::new("activity.channel"),
            }),
            delivered: AtomicU64::new(0),
            marker_fault: Mutex::new(None),
            cursor_unavailable: AtomicBool::new(false),
        }
    }
}

impl ForwardSpool for FixtureSpool {
    fn batch(&self) -> Vec<SpooledEvent> {
        self.event
            .iter()
            .filter(|event| event.seq > self.delivered.load(Ordering::SeqCst))
            .cloned()
            .collect()
    }

    fn persist_delivery(&self, through: u64) -> anyhow::Result<()> {
        match self.marker_fault.lock().unwrap().take() {
            Some(MarkerFault::Error) => anyhow::bail!("synthetic delivery marker write failure"),
            Some(MarkerFault::NoAdvance) => Ok(()),
            None => {
                self.delivered.store(through, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    fn delivery_cursor(&self) -> anyhow::Result<DeliveryCursor> {
        anyhow::ensure!(
            !self.cursor_unavailable.load(Ordering::SeqCst),
            "synthetic spool cursor unavailable"
        );
        Ok(DeliveryCursor {
            spooled: u64::from(self.event.is_some()),
            delivered: self.delivered.load(Ordering::SeqCst),
        })
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<String>>);

impl ActivitySink for RecordingSink {
    async fn deliver(&self, events: &[Event]) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .extend(events.iter().map(|event| event.id.clone()));
        Ok(())
    }
}

#[tokio::test]
async fn marker_fault_does_not_repost_an_acknowledged_batch() {
    for fault in [MarkerFault::Error, MarkerFault::NoAdvance] {
        let spool = FixtureSpool::with_event();
        *spool.marker_fault.lock().unwrap() = Some(fault);
        let sink = RecordingSink::default();
        let (_stop, stopped) = watch::channel(true);

        let result = forward_spool(&spool, &sink, stopped, Duration::from_secs(1)).await;

        assert_eq!(
            sink.0.lock().unwrap().len(),
            1,
            "a missing marker requires recovery, not an immediate repost"
        );
        assert_eq!(result.delivered_through, 0);
        assert_eq!(result.undelivered, None);
        assert_eq!(result.failed_attempts, 1);
        assert!(result.last_error.is_some());
        assert_eq!(spool.delivery_cursor().unwrap().delivered, 0);

        let (_stop, stopped) = watch::channel(true);
        let replayed = forward_spool(&spool, &sink, stopped, Duration::from_secs(1)).await;
        assert_eq!(replayed.delivered_through, 1);
        assert_eq!(replayed.undelivered, Some(0));
        let id = &spool.event.as_ref().unwrap().event.id;
        assert_eq!(sink.0.lock().unwrap().as_slice(), [id.clone(), id.clone()]);
    }
}

#[tokio::test]
async fn an_unavailable_cursor_does_not_report_zero_or_advance_the_confirmed_marker() {
    for pending in [false, true] {
        let mut spool = FixtureSpool::with_event();
        if !pending {
            spool.event = None;
        }
        spool.cursor_unavailable.store(true, Ordering::SeqCst);
        let sink = RecordingSink::default();
        let (_stop, stopped) = watch::channel(true);

        let result = forward_spool(&spool, &sink, stopped, Duration::from_secs(1)).await;

        assert_eq!(sink.0.lock().unwrap().len(), usize::from(pending));
        assert_eq!(result.undelivered, None);
        assert_eq!(result.delivered_through, 0);
        assert!(result.last_error.is_some());
    }
}

#[derive(Default)]
struct HeldSink {
    started: tokio::sync::Notify,
    attempts: AtomicU64,
}

impl ActivitySink for HeldSink {
    async fn deliver(&self, _events: &[Event]) -> anyhow::Result<()> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn stop_bounds_an_in_flight_gateway_await_without_reposting() {
    for drop_owner in [false, true] {
        let spool = FixtureSpool::with_event();
        let sink = HeldSink::default();
        let (stop, stopped) = watch::channel(false);
        let mut forward = std::pin::pin!(forward_spool(
            &spool,
            &sink,
            stopped,
            Duration::from_millis(30),
        ));
        tokio::select! {
            _ = sink.started.notified() => {}
            _ = &mut forward => panic!("the gateway await must still be in flight"),
        };
        if drop_owner {
            drop(stop);
        } else {
            stop.send(true).unwrap();
        }

        let result = tokio::time::timeout(Duration::from_secs(1), forward)
            .await
            .unwrap();

        assert_eq!(sink.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(result.undelivered, Some(1));
        assert_eq!(result.delivered_through, 0);
        assert_eq!(spool.batch().len(), 1);
    }
}
