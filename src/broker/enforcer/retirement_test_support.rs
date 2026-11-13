//! Shared private fixture for writer/guardian composition tests only.
//!
//! The caller supplies its actual test Journal/control Arm and receipt in the
//! registration. The sole runtime substitution is the explicitly SYNTHETIC
//! Ready hook already authorized for the native actor IPC regression tests.
//! Registration auth, original capability, revoke, writer/audit authorization,
//! actual native join and verified Q all use their real producers.

use super::{super::client::EnforcerFullClientV3, tests::OriginalIpcFixture};
use crate::broker::enforcer::{
    CancelGeneration, EnforcedSlot, EnforcerError, EnforcerResult, RegistrationV1,
};
use uuid::Uuid;

/// Retains the real private role, listener, native handles and cleanup owner.
/// No live witness or authorization can be constructed from fixture data.
pub(crate) struct OriginalUnusedFixture {
    fixture: Option<OriginalIpcFixture>,
}

impl OriginalUnusedFixture {
    pub(crate) async fn register(
        installation_id: Uuid,
        registration: RegistrationV1,
    ) -> EnforcerResult<(Self, EnforcerFullClientV3, EnforcedSlot)> {
        Self::register_with_generation(installation_id, registration, CancelGeneration::new()).await
    }

    /// Share the caller's actual cancellation owner with the registered slot.
    pub(crate) async fn register_with_generation(
        installation_id: Uuid,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<(Self, EnforcerFullClientV3, EnforcedSlot)> {
        if installation_id.is_nil() {
            return Err(EnforcerError::InvalidInput);
        }
        let mut fixture = OriginalIpcFixture::start_for_installation(installation_id).await;
        // The hook runs only after NativeOwners owns the actual registered
        // thread. It may return normally only after that actor's real T/exit.
        fixture.install(&registration, true);
        let client = fixture.client().await;
        match client.register(registration, generation).await {
            Ok(slot) => Ok((
                Self {
                    fixture: Some(fixture),
                },
                client,
                slot,
            )),
            Err(error) => {
                fixture.finish().await;
                Err(error)
            }
        }
    }

    /// Connect the real restricted client to this fixture's existing role.
    pub(crate) async fn stop_client(
        &self,
    ) -> EnforcerResult<crate::broker::enforcer::EnforcerStopClient> {
        self.fixture
            .as_ref()
            .ok_or(EnforcerError::Unavailable)?
            .stop_client()
            .await
    }

    /// Close the normal owned listener after the caller's real revoke, without
    /// changing or restoring any namespace or granting new authority.
    pub(crate) async fn stop_listener_preserving_endpoint(&mut self) -> EnforcerResult<()> {
        self.fixture
            .as_mut()
            .ok_or(EnforcerError::Unavailable)?
            .stop_listener_preserving_endpoint()
            .await
    }

    /// Join all owned native/support work after collecting the test result.
    /// An unhealthy anchor is preserved instead of unlinked from a live lease.
    pub(crate) async fn finish(mut self) {
        if let Some(fixture) = self.fixture.take() {
            fixture.finish().await;
        }
    }
}
