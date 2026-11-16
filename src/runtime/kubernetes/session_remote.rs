//! R2 pre-effect lifecycle through the real broker control HTTP client.
//! Local journal work is admitted before spawn_blocking and owned until its
//! actual completion; a cancelled waiter cannot manufacture a durable ACK.
use super::*;
use crate::broker::held_create_client::sessions::{SessionControlClientV2, SessionControlErrorV2};

#[derive(Debug)]
pub(crate) enum RemoteSessionErrorV2 {
    Control(SessionControlErrorV2),
    Journal(SessionError),
    NoRetainedAbandonment,
    WorkerUnavailable,
}
impl fmt::Display for RemoteSessionErrorV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control(error) => fmt::Display::fmt(error, f),
            Self::Journal(error) => fmt::Display::fmt(error, f),
            Self::NoRetainedAbandonment => f.write_str("no abandonment is retained; use the same-incarnation abandon command or the authorized control recovery procedure"),
            Self::WorkerUnavailable => f.write_str("Kubernetes session journal work is unavailable; preserve the journal and retry the same operation"),
        }
    }
}
impl std::error::Error for RemoteSessionErrorV2 {}
type Result<T> = std::result::Result<T, RemoteSessionErrorV2>;

pub(crate) async fn prepare(client: &SessionControlClientV2, root: Directory, protection: SessionJournalProtection,
    intent: crate::control::kubernetes_session::KubernetesSessionReservationIntentV2) -> Result<SessionJournal> {
    // Reopening an existing reservation is reconciliation, including after a
    // control restart has disabled fresh admission. Only an exact NotFound
    // read permits the one reserve request; errors never mean absence.
    match client.observe(&intent).await {
        Ok(_) => {}
        Err(SessionControlErrorV2::NotFound) => { client.reserve(&intent).await.map_err(RemoteSessionErrorV2::Control)?; }
        Err(error) => return Err(RemoteSessionErrorV2::Control(error)),
    }
    let confirmed = client.confirm(&intent).await.map_err(RemoteSessionErrorV2::Control)?;
    let permit = client.filesystem_permit().map_err(RemoteSessionErrorV2::Control)?;
    tokio::task::spawn_blocking(move || {
        let result = SessionJournal::prepare_or_resume(root, protection, &intent, &confirmed).map_err(RemoteSessionErrorV2::Journal);
        drop(permit);
        result
    }).await.map_err(|_| RemoteSessionErrorV2::WorkerUnavailable)?
}

/// Consume the owned journal, retain the immutable close and drop its lock
/// before any HTTP upload. A failure preserves all existing journal files.
pub(crate) async fn retain_abandonment(client: &SessionControlClientV2, mut journal: SessionJournal) -> Result<crate::control::kubernetes_session::KubernetesSessionReservationCloseV2> {
    let permit = client.filesystem_permit().map_err(RemoteSessionErrorV2::Control)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if let Some(close) = journal.retained_abandonment().map_err(RemoteSessionErrorV2::Journal)? { return Ok(close); }
        let intent = journal.intent().map_err(RemoteSessionErrorV2::Journal)?;
        let binding = journal.bootstrap_binding().map_err(RemoteSessionErrorV2::Journal)?;
        let close = KubernetesSessionReservationCloseV2 { version:2, operation_id:Uuid::new_v4().to_string(), reservation_id:intent.reservation_id().into(),
            reservation_input_hash:binding.reservation_input_hash, expected_revision:1, reason:KubernetesSessionReservationCloseReasonV2::PreparationAbandoned };
        journal.retain_abandonment(&close).map_err(RemoteSessionErrorV2::Journal)?;
        Ok(close)
    }).await.map_err(|_| RemoteSessionErrorV2::WorkerUnavailable)?
}

pub(crate) async fn inspect(client: &SessionControlClientV2, root: Directory, protection: SessionJournalProtection) -> Result<RetainedSessionViewV2> {
    let permit = client.filesystem_permit().map_err(RemoteSessionErrorV2::Control)?;
    tokio::task::spawn_blocking(move || {
        let result = SessionJournal::inspect_retained(root, protection).map_err(RemoteSessionErrorV2::Journal);
        drop(permit);
        result
    }).await.map_err(|_| RemoteSessionErrorV2::WorkerUnavailable)?
}

pub(crate) async fn resume_abandonment(client: &SessionControlClientV2, root: Directory, protection: SessionJournalProtection)
    -> Result<crate::control::kubernetes_session::KubernetesSessionReservationMutationResponseV2> {
    let view = inspect(client, root, protection).await?;
    let close = view.abandonment().ok_or(RemoteSessionErrorV2::NoRetainedAbandonment)?;
    client.abandon(view.intent(), close).await.map_err(RemoteSessionErrorV2::Control)
}
