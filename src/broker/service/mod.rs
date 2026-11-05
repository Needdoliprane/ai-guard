//! Trusted service integration. IPC authentication is distinct from Unix UID.
mod client;
mod jobs;
mod profile;
pub mod protocol;
mod recovery_inventory;
mod server;
mod targets;
pub use client::{encode_submit, request};
pub use profile::{RequestedExecutionV1, SubmitV1};
pub use server::{serve, serve_recovery};

use super::{enforcer::CancelGeneration, journal::JournalScopeV1};
use crate::{
    control::SupervisionProtocolV1,
    control_fs::{Directory, absolute_path},
    runner::{AuditClient, ControlIdentityError},
    runtime::Runtime,
    workspace::{AuthorizedExportParent, AuthorizedSource},
};
use anyhow::{Result, anyhow, ensure};
use std::{
    fmt,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

/// Created only after the same presented credential has been confirmed against
/// the current gateway. No serde or public constructor from caller strings.
pub(crate) struct AuthenticatedBatchContext {
    client: AuditClient,
    scope: JournalScopeV1,
    config: Directory,
    runtime: Runtime,
    cancel: CancelGeneration,
    expected_generation: u64,
    source: Option<Arc<AuthorizedSource>>,
    exclusions: Vec<File>,
}
impl fmt::Debug for AuthenticatedBatchContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthenticatedBatchContext(<private>)")
    }
}
impl AuthenticatedBatchContext {
    /// Test fixture for data/RPC contracts, using a real gateway identity check.
    /// This does not qualify operator-registry or filesystem admission.
    #[cfg(test)]
    pub(crate) async fn for_executor_test(
        client: AuditClient,
        scope: JournalScopeV1,
        config: Directory,
        runtime: Runtime,
    ) -> Result<Self> {
        let identity = client.supervision_identity().await?;
        ensure!(
            identity.active
                && [3, 4, 5, 6, 7, 8].contains(&identity.schema_version)
                && identity.protocols == [SupervisionProtocolV1::BrokerSupervisionV1]
                && identity.role == "runner"
                && identity.tenant_id == scope.tenant_id()
                && identity.control_namespace_uuid.as_deref()
                    == Some(scope.control_namespace_uuid().to_string().as_str()),
            "executor fixture requires the matching active gateway runner identity"
        );
        let cancel = CancelGeneration::new();
        let expected_generation = cancel.current();
        Ok(Self {
            client,
            scope,
            config,
            runtime,
            cancel,
            expected_generation,
            source: None,
            exclusions: Vec::new(),
        })
    }

    pub(crate) fn client(&self) -> &AuditClient {
        &self.client
    }
    pub(crate) fn scope(&self) -> &JournalScopeV1 {
        &self.scope
    }
    pub(crate) fn config(&self) -> &Directory {
        &self.config
    }
    pub(crate) fn runtime(&self) -> &Runtime {
        &self.runtime
    }
    pub(crate) fn cancel_generation(&self) -> &CancelGeneration {
        &self.cancel
    }
    pub(crate) fn expected_generation(&self) -> u64 {
        self.expected_generation
    }
    pub(crate) fn source_handle(&self) -> Option<Arc<AuthorizedSource>> {
        self.source.clone()
    }
    pub(crate) fn excluded_roots(&self) -> &[File] {
        &self.exclusions
    }
}

pub(crate) struct ValidatedExportTarget {
    request_id: Uuid,
    destination: PathBuf,
    destination_ref: String,
    destination_intent_hash: String,
    parent: Arc<AuthorizedExportParent>,
}
impl fmt::Debug for ValidatedExportTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ValidatedExportTarget(<private>)")
    }
}
impl ValidatedExportTarget {
    pub(crate) fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub(crate) fn destination(&self) -> &Path {
        &self.destination
    }
    pub(crate) fn destination_ref(&self) -> &str {
        &self.destination_ref
    }
    pub(crate) fn destination_intent_hash(&self) -> &str {
        &self.destination_intent_hash
    }
    pub(crate) fn parent(&self) -> &AuthorizedExportParent {
        &self.parent
    }
}

struct VerifiedProfile {
    client: AuditClient,
    scope: JournalScopeV1,
}
impl VerifiedProfile {
    async fn authenticate(
        registry: &profile::Registry,
        profile_id: &str,
        credential: &str,
        require_active: bool,
    ) -> std::result::Result<Self, protocol::BrokerErrorCodeV1> {
        use protocol::BrokerErrorCodeV1 as Code;
        if credential.is_empty()
            || credential.len() > 4096
            || !credential.is_ascii()
            || credential.chars().any(char::is_whitespace)
            || credential.chars().any(char::is_control)
        {
            return Err(Code::Unauthorized);
        }
        // Profile lookup reads already validated operator memory only. No job,
        // project, provider or configuration directory is opened for this caller.
        let profile = registry
            .profile(profile_id)
            .map_err(|_| Code::Unauthorized)?;
        let client =
            AuditClient::new(&profile.gateway, credential).map_err(|_| Code::Unauthorized)?;
        let protocol = client
            .supervision_identity()
            .await
            .map_err(|error| match error {
                ControlIdentityError::Unauthorized => Code::Unauthorized,
                ControlIdentityError::Forbidden => Code::Forbidden,
                ControlIdentityError::Unavailable => Code::Unavailable,
                ControlIdentityError::InvalidResponse => Code::IncompatibleControl,
            })?;
        if protocol.role != "runner"
            || protocol.tenant_id != profile.tenant_id
            || protocol.control_namespace_uuid.as_deref()
                != Some(profile.control_namespace_uuid.to_string().as_str())
        {
            return Err(Code::Forbidden);
        }
        if ![3, 4, 5, 6, 7, 8].contains(&protocol.schema_version)
            || !protocol
                .protocols
                .contains(&SupervisionProtocolV1::BrokerSupervisionV1)
            || (require_active && !protocol.active)
        {
            return Err(Code::IncompatibleControl);
        }
        let scope = JournalScopeV1::new(
            registry.data.installation_id,
            &protocol.tenant_id,
            &profile.gateway,
            profile.control_namespace_uuid,
            &profile.id,
        )
        .map_err(|_| Code::IncompatibleControl)?;
        Ok(Self { client, scope })
    }

    fn open_context(
        self,
        registry: &profile::Registry,
        profile: &profile::ControlProfileV1,
        root: &Directory,
        cancel: CancelGeneration,
        source: Option<Arc<AuthorizedSource>>,
    ) -> Result<AuthenticatedBatchContext> {
        // Capture once, before any filesystem operation or deferred task. A
        // context created before cancellation must never adopt its new epoch.
        let expected_generation = cancel.current();
        ensure!(
            self.scope.profile_id() == profile.id && self.scope.tenant_id() == profile.tenant_id,
            "broker profile identity changed"
        );
        let config_path = absolute_path(&profile.config_dir)
            .map_err(|_| anyhow!("invalid private broker configuration root"))?;
        let config = Directory::open_or_create(&config_path)
            .map_err(|_| anyhow!("private broker configuration root is unavailable"))?;
        let runtime = Runtime::with_program(profile.runtime, &profile.runtime_program)
            .map_err(|_| anyhow!("approved broker runtime is unavailable"))?;
        let exclusions = vec![
            root.try_clone_file()
                .map_err(|_| anyhow!("broker control root changed"))?,
            config
                .try_clone_file()
                .map_err(|_| anyhow!("broker configuration root changed"))?,
            registry
                .source_directory
                .try_clone_file()
                .map_err(|_| anyhow!("broker registry root changed"))?,
        ];
        Ok(AuthenticatedBatchContext {
            client: self.client,
            scope: self.scope,
            config,
            runtime,
            cancel,
            expected_generation,
            source,
            exclusions,
        })
    }
}
