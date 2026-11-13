//! Readonly consumption of an explicitly issued workload operation. This does
//! not issue/rotate a proof, authenticate a worker, or grant any runtime effect.
use super::{Result, WorkloadCliError as Error, storage::Operation};
use crate::{
    broker::journal::JournalScopeV1,
    control_fs::PrivateFile,
    relay::{
        RelayEndpointV1,
        client::{RelayClientProfile, WorkerRelayClient},
    },
    workload::{WorkloadBindingIssueV1, WorkloadBindingReceiptV1, WorkloadSubjectV1},
};
use std::path::Path;
#[cfg(test)]
pub(crate) mod fixtures;

/// No deserializer, free DTO constructor or Debug implementation. The retained
/// issued operation and every supporting FD outlive its use by a channel.
pub(crate) struct RetainedWorkerBinding {
    operation: Operation,
    issue: WorkloadBindingIssueV1,
    receipt: WorkloadBindingReceiptV1,
    credential: PrivateFile,
    records: Vec<PrivateFile>,
    endpoint: RelayEndpointV1,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;
    #[test]
    fn readonly_issued_origin_equates_root_slash_without_accepting_another_origin() {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = crate::control_fs::Directory::open_existing(temporary.path()).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        let slash = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9/",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        let endpoint = RelayEndpointV1 {
            worker_id: Uuid::from_u128(3).to_string(),
            attempt_id: Uuid::from_u128(4).to_string(),
            generation: 1,
        };
        let path = fixtures::issued_operation(&root, "issued", &scope, &endpoint);
        let binding = RetainedWorkerBinding::open(&path, &scope).unwrap();
        assert_eq!(binding.endpoint(), &endpoint);
        binding.require_scope(&slash).unwrap();
        let other = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:10",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        assert!(matches!(
            binding.require_scope(&other),
            Err(Error::ScopeMismatch)
        ));
        for invalid in [
            "http://127.0.0.1:9/path",
            "http://127.0.0.1:9/?q=x",
            "http://127.0.0.1:9/#x",
            "http://user@127.0.0.1:9/",
        ] {
            assert!(
                JournalScopeV1::new(
                    Uuid::from_u128(1),
                    "tenant-a",
                    invalid,
                    Uuid::from_u128(2),
                    "fixture"
                )
                .is_err()
            );
        }
    }
}
impl RetainedWorkerBinding {
    pub(crate) fn open(path: &Path, scope: &JournalScopeV1) -> Result<Self> {
        let normalized = crate::control_fs::absolute_path(path).map_err(|_| Error::UnsafeState)?;
        let parent = crate::control_fs::Directory::open_existing(
            normalized.parent().ok_or(Error::UnsafeState)?,
        )
        .map_err(|_| Error::UnsafeState)?;
        let leaf = normalized
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::UnsafeState)?;
        if parent
            .persistent_child(leaf, false)
            .map_err(|_| Error::UnsafeState)?
            .is_none()
        {
            return Err(Error::IncompletePreparation);
        }
        let operation = Operation::open_readonly(path)?;
        let (issue, receipt, credential, records) = operation.retained_binding_material()?;
        let WorkloadSubjectV1::Worker {
            worker_id,
            attempt_id,
            generation,
        } = &issue.subject
        else {
            return Err(Error::ScopeMismatch);
        };
        let endpoint = RelayEndpointV1 {
            worker_id: worker_id.clone(),
            attempt_id: attempt_id.clone(),
            generation: *generation,
        };
        let retained = Self {
            operation,
            issue,
            receipt,
            credential,
            records,
            endpoint,
        };
        retained.require_scope(scope)?;
        retained.revalidate()?;
        Ok(retained)
    }
    fn require_scope(&self, scope: &JournalScopeV1) -> Result<()> {
        let expected = &self.operation.recovery;
        let gateway = super::manifest::origin(scope.gateway_origin())?;
        if expected.gateway != gateway.as_str()
            || expected.expected_tenant_id != scope.tenant_id()
            || expected.control_namespace_uuid != scope.control_namespace_uuid().to_string()
        {
            return Err(Error::ScopeMismatch);
        }
        Ok(())
    }
    pub(crate) fn revalidate(&self) -> Result<()> {
        self.operation.revalidate()?;
        self.credential
            .persistent_revalidate()
            .map_err(|_| Error::UnsafeState)?;
        for record in &self.records {
            record
                .persistent_revalidate()
                .map_err(|_| Error::UnsafeState)?;
            crate::json::decode_unique(record.bytes(), 64).map_err(|_| Error::UnsafeState)?;
        }
        let Some(super::storage::RecordedReceipt::Binding(current)) = self.operation.receipt()?
        else {
            return Err(Error::IncompletePreparation);
        };
        if current != self.receipt {
            return Err(Error::UnsafeState);
        }
        Ok(())
    }
    pub(crate) fn endpoint(&self) -> &RelayEndpointV1 {
        &self.endpoint
    }
    pub(crate) fn credential_id(&self) -> &str {
        &self.issue.credential_id
    }
    pub(crate) fn revision(&self) -> u64 {
        self.receipt.revision
    }
    pub(crate) fn digest(&self) -> &str {
        &self.issue.sha256_digest
    }
    pub(crate) fn expires_at(&self) -> &str {
        &self.issue.expires_at
    }
    pub(crate) fn credential_file(&self) -> PrivateFile {
        self.credential.clone()
    }
    pub(crate) fn client(
        &self,
        scope: &JournalScopeV1,
        gateway_trust: &crate::tls::ClientTrust,
    ) -> Result<WorkerRelayClient> {
        self.require_scope(scope)?;
        self.revalidate()?;
        let proof = std::str::from_utf8(self.credential.bytes())
            .map_err(|_| Error::UnsafeState)?
            .trim_end_matches('\n');
        WorkerRelayClient::with_profile_and_gateway_trust(
            scope.gateway_origin(),
            proof,
            self.endpoint.clone(),
            RelayClientProfile::ControlIngress,
            gateway_trust,
        )
        .map_err(|_| Error::UnsafeState)
    }
}
