//! Pin the original Legacy listener for one StopsOnly exchange.
use super::{EnforcerError, EnforcerResult};
use crate::{
    broker::{
        discovery::{load_endpoint, recovery_root_binding},
        local_ipc::EndpointRecordV1,
    },
    control_fs::Directory,
};
use tokio::net::UnixStream;

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
