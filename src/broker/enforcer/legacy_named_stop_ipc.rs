//! Pin the original Legacy listener for one StopsOnly exchange.
use super::{
    Challenge, EnforcerError, EnforcerResult, IO_TIMEOUT, capability,
    legacy_named_stop_auth::{
        HANDSHAKE_FRAME, StopsOnlyAckV1, StopsOnlyAuthenticationV1, StopsOnlyGrantV1,
        StopsOnlyResultV1, request_hash,
    },
};
use crate::{
    broker::{
        discovery::{load_endpoint, recovery_root_binding},
        journal::CancelStopProjectionV1,
        local_ipc::{EndpointRecordV1, read_frame, write_frame},
    },
    control_fs::Directory,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::net::UnixStream;

pub(super) const STOP_COMMAND_FRAME: u32 = 8 * 1024;
pub(super) const STOP_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum NamedImportStopCommand<T> {
    DeliverNamedImportStop { facts: T },
}

pub(in crate::broker) async fn deliver_named_import_stop_from_legacy_root(
    root: &Directory,
    projection: &CancelStopProjectionV1,
) -> EnforcerResult<()> {
    let endpoint = NamedImportStopEndpoint::pin(root, None)?;
    let key = capability(root, false)?;
    endpoint.deliver(projection, &key).await
}

pub(super) struct NamedImportStopEndpoint {
    root: Directory,
    endpoint: EndpointRecordV1,
    binding: String,
}

impl NamedImportStopEndpoint {
    /// An established broker supplies its guarded endpoint record. Cold
    /// recovery may pin the current verified announcement once. This path
    /// neither acquires the guardian lease nor starts another guardian.
    pub(super) fn pin(
        root: &Directory,
        established: Option<&EndpointRecordV1>,
    ) -> EnforcerResult<Self> {
        let announced = load_endpoint(root)?.ok_or(EnforcerError::Unavailable)?;
        let endpoint = established.unwrap_or(&announced);
        let binding = recovery_root_binding(root, endpoint)?;
        check_announcement(root, &announced, endpoint, &binding)?;
        Ok(Self {
            root: root.clone(),
            endpoint: endpoint.clone(),
            binding,
        })
    }

    pub(super) fn endpoint(&self) -> &EndpointRecordV1 {
        &self.endpoint
    }

    pub(super) fn binding(&self) -> &str {
        &self.binding
    }

    pub(super) fn revalidate(&self) -> EnforcerResult<()> {
        let announced = load_endpoint(&self.root)?.ok_or(EnforcerError::Unavailable)?;
        check_announcement(&self.root, &announced, &self.endpoint, &self.binding)
    }

    pub(super) async fn connect(&self) -> EnforcerResult<UnixStream> {
        self.revalidate()?;
        let endpoints = self
            .root
            .persistent_child("endpoints", false)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?
            .ok_or(EnforcerError::Unavailable)?;
        let stream = self.endpoint.connect_checked(&endpoints).await?;
        self.revalidate()?;
        Ok(stream)
    }

    pub(super) async fn deliver(
        &self,
        projection: &CancelStopProjectionV1,
        key: &[u8; 32],
    ) -> EnforcerResult<()> {
        let facts = projection
            .named_import_stop_facts()
            .map_err(|_| EnforcerError::Conflict)?
            .ok_or(EnforcerError::NotReady)?;
        self.exchange(&facts, key, || {
            projection.revalidate().map_err(|_| EnforcerError::Conflict)
        })
        .await
    }

    pub(super) async fn exchange(
        &self,
        facts: &impl Serialize,
        key: &[u8; 32],
        before_send: impl FnOnce() -> EnforcerResult<()>,
    ) -> EnforcerResult<()> {
        tokio::time::timeout(STOP_EXCHANGE_TIMEOUT, async {
            let mut stream = self.connect().await?;
            let challenge: Challenge = read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            if challenge.epoch != self.endpoint().epoch() {
                return Err(EnforcerError::Unauthorized);
            }
            let command = NamedImportStopCommand::DeliverNamedImportStop { facts };
            let (authentication, session) = StopsOnlyAuthenticationV1::new(
                key,
                &challenge,
                self.binding(),
                &request_hash(&command)?,
            )?;
            write_frame(&mut stream, &authentication, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            let grant: StopsOnlyGrantV1 =
                read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            session.verify_grant(key, &grant)?;
            self.revalidate()?;
            before_send()?;
            write_frame(&mut stream, &command, STOP_COMMAND_FRAME, IO_TIMEOUT).await?;
            let ack: StopsOnlyAckV1 = read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            match session.verify_acknowledgement(key, &ack)? {
                StopsOnlyResultV1::IntentLatched {} => Ok(()),
                StopsOnlyResultV1::Refused { code } => Err(code),
            }
        })
        .await
        .map_err(|_| EnforcerError::Unavailable)?
    }
}

fn check_announcement(
    root: &Directory,
    announced: &EndpointRecordV1,
    expected: &EndpointRecordV1,
    binding: &str,
) -> EnforcerResult<()> {
    if announced.epoch() != expected.epoch() || recovery_root_binding(root, announced)? != binding {
        return Err(EnforcerError::Unauthorized);
    }
    Ok(())
}
