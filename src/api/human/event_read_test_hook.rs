//! One-shot post-commit event-read barrier. This module is entirely cfg(test).
use super::*;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

pub(super) struct Pause {
    reached: SyncSender<()>,
    resume: Receiver<()>,
}

/// Unwinding the test always releases its accepted blocking read. If the
/// receiver already timed out, this nonblocking send cannot mask the failure.
pub(crate) struct Release {
    resume: Option<SyncSender<()>>,
}
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(resume) = self.resume.take() {
            let _ = resume.try_send(());
        }
    }
}

pub(crate) fn install(state: &GatewayState) -> (Receiver<()>, Release) {
    let (reached, observed) = sync_channel(1);
    let (resume, resumed) = sync_channel(1);
    let previous = state
        .human
        .event_publication_pause
        .lock()
        .unwrap()
        .replace(Pause {
            reached,
            resume: resumed,
        });
    assert!(
        previous.is_none(),
        "one event publication pause per fixture"
    );
    (
        observed,
        Release {
            resume: Some(resume),
        },
    )
}

pub(super) fn pause(human: &HumanAccessState) -> ApiResult<()> {
    let pause = human
        .event_publication_pause
        .lock()
        .map_err(|_| cursor_unavailable())?
        .take();
    if let Some(pause) = pause {
        pause
            .reached
            .try_send(())
            .map_err(|_| cursor_unavailable())?;
        // This bound exists only in a test build. Production deadlines and
        // behavior have no test barrier or extra wait.
        pause
            .resume
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| cursor_unavailable())?;
    }
    Ok(())
}
