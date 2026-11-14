use agentsandbox::{
    control::egress_policy::{
        EgressPolicyExpectedScopeV1, EgressPolicyReceiptV1, EgressReplacePolicyRequestV1,
        EgressRevokePolicyRequestV1,
    },
    control_fs::{
        Directory, PrivateFile, absolute_path,
        operation::{DurableOperationDirectory, OperationRecord},
    },
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Replace,
    Revoke,
}

pub(super) enum Request {
    Replace(EgressReplacePolicyRequestV1),
    Revoke(EgressRevokePolicyRequestV1),
}
impl Request {
    fn parse(action: Action, bytes: &[u8]) -> Result<Self> {
        match action {
            Action::Replace => EgressReplacePolicyRequestV1::from_json(bytes).map(Self::Replace),
            Action::Revoke => EgressRevokePolicyRequestV1::from_json(bytes).map(Self::Revoke),
        }
        .map_err(|_| anyhow!("egress_policy_invalid_preparation"))
    }
    fn canonical(&self) -> Result<Vec<u8>> {
        let result = match self {
            Self::Replace(request) => {
                request
                    .validate()
                    .map_err(|_| anyhow!("egress_policy_invalid_preparation"))?;
                request.canonical_bytes()
            }
            Self::Revoke(request) => {
                request
                    .validate()
                    .map_err(|_| anyhow!("egress_policy_invalid_preparation"))?;
                request.canonical_bytes()
            }
        };
        result.map_err(|_| anyhow!("egress_policy_invalid_preparation"))
    }
    pub(super) fn operation_id(&self) -> uuid::Uuid {
        match self {
            Self::Replace(r) => r.operation_id(),
            Self::Revoke(r) => r.operation_id(),
        }
    }
    pub(super) fn scope(&self) -> &EgressPolicyExpectedScopeV1 {
        match self {
            Self::Replace(r) => r.expected_scope(),
            Self::Revoke(r) => r.expected_scope(),
        }
    }
    fn revision(&self) -> u64 {
        match self {
            Self::Replace(r) => r.expected_revision(),
            Self::Revoke(r) => r.expected_revision(),
        }
    }
    fn epoch(&self) -> u64 {
        match self {
            Self::Replace(r) => r.expected_epoch(),
            Self::Revoke(r) => r.expected_epoch(),
        }
    }
    fn validate_receipt(&self, receipt: &EgressPolicyReceiptV1) -> Result<()> {
        match self {
            Self::Replace(r) => receipt.validate_for_replace(r),
            Self::Revoke(r) => receipt.validate_for_revoke(r),
        }
        .map_err(|_| anyhow!("egress_policy_invalid_receipt"))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input<'a> {
    version: u32,
    action: Action,
    #[serde(borrow)]
    request: &'a RawValue,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Metadata {
    version: u32,
    pub(super) action: Action,
    pub(super) gateway: String,
    operation_id: String,
    expected_scope: EgressPolicyExpectedScopeV1,
    expected_revision: u64,
    expected_epoch: u64,
    request_sha256: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Prepared,
    RemoteOutcomeUnresolved,
    ReceiptRecorded,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    version: u32,
    operation_id: String,
    request_sha256: String,
    action: Action,
    pub(super) phase: Phase,
    pub(super) attempts: u64,
    pub(super) last_error: Option<String>,
}

pub(super) fn fs_error<T>(
    result: std::result::Result<T, agentsandbox::control_fs::operation::OperationFsError>,
) -> Result<T> {
    result.map_err(|error| match error {
        agentsandbox::control_fs::operation::OperationFsError::Busy => {
            anyhow!("egress_policy_local_busy")
        }
        agentsandbox::control_fs::operation::OperationFsError::DurabilityUnknown => {
            anyhow!("egress_policy_local_publication_unresolved")
        }
        _ => anyhow!("egress_policy_unsafe_local_operation"),
    })
}
fn bytes(directory: &DurableOperationDirectory, record: OperationRecord) -> Result<&[u8]> {
    directory
        .record(record)
        .map(PrivateFile::bytes)
        .ok_or_else(|| anyhow!("egress_policy_incomplete_local_operation"))
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| anyhow!("egress_policy_invalid_local_record"))
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| anyhow!("egress_policy_invalid_local_record"))?;
    ensure!(bytes.len() <= 4096, "egress_policy_local_record_too_large");
    Ok(bytes)
}

pub(super) fn origin(value: &str) -> Result<String> {
    ensure!(
        value.trim() == value && !value.chars().any(char::is_control),
        "egress_policy_invalid_origin"
    );
    let url = reqwest::Url::parse(value).map_err(|_| anyhow!("egress_policy_invalid_origin"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        (url.scheme() == "https" || url.scheme() == "http" && loopback)
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "egress_policy_invalid_origin"
    );
    Ok(url.to_string())
}

fn safe_code(code: &str) -> bool {
    matches!(
        code,
        "egress_policy_invalid_request"
            | "egress_policy_invalid_response"
            | "egress_policy_unauthenticated"
            | "egress_policy_forbidden"
            | "egress_policy_scope_conflict"
            | "egress_policy_conflict"
            | "egress_policy_migration_required"
            | "egress_policy_capacity_exhausted"
            | "egress_policy_busy"
            | "egress_policy_clock_unavailable"
            | "egress_policy_unavailable"
            | "egress_policy_transport_unresolved"
    )
}
fn validate_state(state: &State, metadata: &Metadata) -> Result<()> {
    ensure!(
        state.version == 1
            && state.operation_id == metadata.operation_id
            && state.request_sha256 == metadata.request_sha256
            && state.action == metadata.action,
        "egress_policy_local_state_mismatch"
    );
    ensure!(
        state.last_error.as_deref().is_none_or(safe_code),
        "egress_policy_invalid_local_error"
    );
    ensure!(
        match state.phase {
            Phase::Prepared => state.attempts == 0 && state.last_error.is_none(),
            Phase::RemoteOutcomeUnresolved => state.attempts > 0,
            Phase::ReceiptRecorded => state.attempts > 0 && state.last_error.is_none(),
        },
        "egress_policy_invalid_local_state"
    );
    Ok(())
}
fn transition(before: &State, after: &State) -> Result<()> {
    let allowed = match (before.phase, after.phase) {
        (Phase::Prepared, Phase::RemoteOutcomeUnresolved) => {
            before.attempts.checked_add(1) == Some(after.attempts)
        }
        (Phase::RemoteOutcomeUnresolved, Phase::RemoteOutcomeUnresolved) => {
            after.attempts == before.attempts
                || before.attempts.checked_add(1) == Some(after.attempts)
        }
        (Phase::RemoteOutcomeUnresolved | Phase::ReceiptRecorded, Phase::ReceiptRecorded) => {
            after.attempts == before.attempts
        }
        _ => false,
    };
    ensure!(allowed, "egress_policy_invalid_local_transition");
    Ok(())
}

pub(super) fn prepare(gateway: &str, input: &Path, output: &Path) -> Result<Loaded> {
    let input = absolute_path(input).map_err(|_| anyhow!("egress_policy_unsafe_input"))?;
    let file =
        PrivateFile::open(&input, 66_560).map_err(|_| anyhow!("egress_policy_unsafe_input"))?;
    let envelope: Input<'_> =
        serde_json::from_slice(file.bytes()).map_err(|_| anyhow!("egress_policy_invalid_input"))?;
    ensure!(envelope.version == 1, "egress_policy_invalid_input");
    let request = Request::parse(envelope.action, envelope.request.get().as_bytes())?;
    prepare_request(
        gateway,
        envelope.action,
        request,
        output,
        |_| {
            file.revalidate()
                .map_err(|_| anyhow!("egress_policy_input_changed"))
        },
        |_| {
            file.revalidate()
                .map_err(|_| anyhow!("egress_policy_input_changed_after_preparation"))
        },
    )
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Draft {
    Replace {
        version: u32,
        policy: agentsandbox::control::egress_policy::EgressPolicyInputV1,
    },
    Revoke {
        version: u32,
    },
}

/// Derive a new local request from an explicitly retained historical report.
/// Only the later server mutation is authoritative for scope and CAS.
pub(super) fn prepare_from_inspection(
    inspection_dir: &Path,
    input: &Path,
    output: &Path,
) -> Result<Loaded> {
    use agentsandbox::control::egress_policy::{
        EgressPolicyObservedCeilingV1, EgressPolicyObservedHeadV1,
        EgressPolicyObservedSessionResourceV1, EgressPolicySelectorV1, EgressPolicyStateV1,
        EgressPolicyTenantCeilingV1,
    };
    let inspection_dir =
        absolute_path(inspection_dir).map_err(|_| anyhow!("egress_policy_unsafe_inspection"))?;
    let output = absolute_path(output).map_err(|_| anyhow!("egress_policy_unsafe_output"))?;
    ensure!(
        !output.starts_with(&inspection_dir),
        "egress_policy_output_overlaps_inspection"
    );
    // Retain the real report flock through all derivation and publication.
    let report = super::inspection::storage::Report::open(&inspection_dir)?;
    let input = absolute_path(input).map_err(|_| anyhow!("egress_policy_unsafe_input"))?;
    let file =
        PrivateFile::open(&input, 66_560).map_err(|_| anyhow!("egress_policy_unsafe_input"))?;
    let draft: Draft =
        serde_json::from_slice(file.bytes()).map_err(|_| anyhow!("egress_policy_invalid_draft"))?;
    let observation = &report.observation;
    let (revision, epoch) = match observation.head() {
        EgressPolicyObservedHeadV1::Absent => (0, 0),
        EgressPolicyObservedHeadV1::Present {
            revision, epoch, ..
        } => (*revision, *epoch),
    };
    let (action, request) = match draft {
        Draft::Replace { version, policy } => {
            ensure!(version == 1, "egress_policy_invalid_draft");
            let tenant_ceiling = match observation.selector() {
                EgressPolicySelectorV1::Tenant => None,
                EgressPolicySelectorV1::Session(_) => {
                    ensure!(
                        observation.session_resource()
                            == EgressPolicyObservedSessionResourceV1::Active,
                        "egress_policy_session_resource_unavailable"
                    );
                    let EgressPolicyObservedCeilingV1::Active {
                        revision,
                        epoch,
                        digest,
                    } = observation.tenant_ceiling()
                    else {
                        return Err(anyhow!("egress_policy_tenant_ceiling_unavailable"));
                    };
                    let digest: [u8; 32] = hex::decode(digest)
                        .map_err(|_| anyhow!("egress_policy_invalid_inspection_ceiling"))?
                        .try_into()
                        .map_err(|_| anyhow!("egress_policy_invalid_inspection_ceiling"))?;
                    Some(
                        EgressPolicyTenantCeilingV1::new(*revision, *epoch, digest)
                            .map_err(|_| anyhow!("egress_policy_invalid_inspection_ceiling"))?,
                    )
                }
            };
            let request = EgressReplacePolicyRequestV1::new(
                uuid::Uuid::new_v4(),
                observation.scope().clone(),
                observation.selector().clone(),
                revision,
                epoch,
                policy,
                tenant_ceiling,
            )
            .map_err(|_| anyhow!("egress_policy_invalid_preparation"))?;
            (Action::Replace, Request::Replace(request))
        }
        Draft::Revoke { version } => {
            ensure!(version == 1, "egress_policy_invalid_draft");
            ensure!(
                matches!(
                    observation.head(),
                    EgressPolicyObservedHeadV1::Present {
                        state: EgressPolicyStateV1::Active,
                        ..
                    }
                ),
                "egress_policy_revoke_requires_active_head"
            );
            // Cleanup needs the actual target CAS, independently of resource
            // activity, ceiling activity or any former policy time window.
            let request = EgressRevokePolicyRequestV1::new(
                uuid::Uuid::new_v4(),
                observation.scope().clone(),
                observation.selector().clone(),
                revision,
                epoch,
            )
            .map_err(|_| anyhow!("egress_policy_invalid_preparation"))?;
            (Action::Revoke, Request::Revoke(request))
        }
    };
    prepare_request(
        report.gateway(),
        action,
        request,
        &output,
        |parent| {
            report.revalidate()?;
            report.require_output_parent_outside(parent)?;
            file.revalidate()
                .map_err(|_| anyhow!("egress_policy_input_changed"))
        },
        |parent| {
            report.revalidate()?;
            report.require_output_parent_outside(parent)?;
            file.revalidate()
                .map_err(|_| anyhow!("egress_policy_input_changed_after_preparation"))
        },
    )
}

fn prepare_request(
    gateway: &str,
    action: Action,
    request: Request,
    output: &Path,
    revalidate_before: impl Fn(&Directory) -> Result<()>,
    revalidate_after: impl Fn(&Directory) -> Result<()>,
) -> Result<Loaded> {
    let canonical = request.canonical()?;
    ensure!(canonical.len() <= 65_536, "egress_policy_invalid_input");
    use sha2::{Digest, Sha256};
    let metadata = Metadata {
        version: 1,
        action,
        gateway: origin(gateway)?,
        operation_id: request.operation_id().to_string(),
        expected_scope: request.scope().clone(),
        expected_revision: request.revision(),
        expected_epoch: request.epoch(),
        request_sha256: hex::encode(Sha256::digest(&canonical)),
    };
    let state = State {
        version: 1,
        operation_id: metadata.operation_id.clone(),
        request_sha256: metadata.request_sha256.clone(),
        action: metadata.action,
        phase: Phase::Prepared,
        attempts: 0,
        last_error: None,
    };
    let output = absolute_path(output).map_err(|_| anyhow!("egress_policy_unsafe_output"))?;
    let containing = output
        .parent()
        .ok_or_else(|| anyhow!("egress_policy_unsafe_output"))?;
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("egress_policy_unsafe_output"))?;
    let parent = Directory::open_existing(containing)
        .map_err(|_| anyhow!("egress_policy_unsafe_output_parent"))?;
    revalidate_before(&parent)?;
    fs_error(DurableOperationDirectory::prepare_new(
        &parent,
        name,
        &canonical,
        &encode(&metadata)?,
        &encode(&state)?,
    ))?;
    revalidate_after(&parent)?;
    let loaded = Loaded::open(&output)?;
    revalidate_after(&parent)?;
    loaded.revalidate()?;
    Ok(loaded)
}

pub(super) struct Loaded {
    directory: DurableOperationDirectory,
    pub(super) request: Request,
    pub(super) metadata: Metadata,
    pub(super) state: State,
    pending: Option<State>,
}
impl Loaded {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let directory = fs_error(DurableOperationDirectory::open_existing(path))?;
        let metadata: Metadata = decode(bytes(&directory, OperationRecord::Metadata)?)?;
        ensure!(
            metadata.version == 1 && origin(&metadata.gateway)? == metadata.gateway,
            "egress_policy_invalid_local_metadata"
        );
        metadata
            .expected_scope
            .validate()
            .map_err(|_| anyhow!("egress_policy_invalid_local_metadata"))?;
        let request = Request::parse(
            metadata.action,
            bytes(&directory, OperationRecord::Request)?,
        )?;
        let held = directory
            .record(OperationRecord::Request)
            .ok_or_else(|| anyhow!("egress_policy_incomplete_local_operation"))?;
        ensure!(
            request.canonical()?.as_slice() == held.bytes()
                && metadata.request_sha256 == held.digest()
                && metadata.operation_id == request.operation_id().to_string()
                && &metadata.expected_scope == request.scope()
                && metadata.expected_revision == request.revision()
                && metadata.expected_epoch == request.epoch(),
            "egress_policy_local_preparation_mismatch"
        );
        ensure!(
            encode(&metadata)?.as_slice() == bytes(&directory, OperationRecord::Metadata)?,
            "egress_policy_noncanonical_local_metadata"
        );
        let state: State = decode(bytes(&directory, OperationRecord::State)?)?;
        validate_state(&state, &metadata)?;
        let pending = directory
            .record(OperationRecord::PendingState)
            .map(|file| decode::<State>(file.bytes()))
            .transpose()?;
        if let Some(next) = &pending {
            validate_state(next, &metadata)?;
            transition(&state, next)?;
        }
        let loaded = Self {
            directory,
            request,
            metadata,
            state,
            pending,
        };
        let receipt = loaded.receipt()?;
        ensure!(
            loaded.state.phase != Phase::ReceiptRecorded || receipt.is_some(),
            "egress_policy_missing_local_receipt"
        );
        ensure!(
            receipt.is_none() || loaded.state.attempts > 0,
            "egress_policy_unbound_local_receipt"
        );
        if loaded
            .pending
            .as_ref()
            .is_some_and(|state| state.phase == Phase::ReceiptRecorded)
        {
            ensure!(receipt.is_some(), "egress_policy_missing_pending_receipt");
        }
        loaded.revalidate()?;
        Ok(loaded)
    }

    pub(super) fn revalidate(&self) -> Result<()> {
        fs_error(self.directory.revalidate())
    }
    pub(super) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn receipt(&self) -> Result<Option<EgressPolicyReceiptV1>> {
        self.directory
            .record(OperationRecord::Receipt)
            .map(|file| {
                let receipt = EgressPolicyReceiptV1::from_canonical_bytes(file.bytes())
                    .map_err(|_| anyhow!("egress_policy_invalid_local_receipt"))?;
                self.request.validate_receipt(&receipt)?;
                Ok(receipt)
            })
            .transpose()
    }
    pub(super) fn recover_pending(&mut self) -> Result<()> {
        if let Some(next) = self.pending.as_ref() {
            validate_state(next, &self.metadata)?;
            transition(&self.state, next)?;
            let next = next.clone();
            fs_error(self.directory.publish_retained_pending_state())?;
            self.state = next;
            self.pending = None;
        }
        self.revalidate()
    }
    fn publish(&mut self, next: State) -> Result<()> {
        validate_state(&next, &self.metadata)?;
        transition(&self.state, &next)?;
        fs_error(self.directory.publish_state(&encode(&next)?))?;
        self.state = next;
        Ok(())
    }
    pub(super) fn begin_attempt(&mut self) -> Result<()> {
        ensure!(
            !self.has_pending() && self.receipt()?.is_none(),
            "egress_policy_resume_required"
        );
        let mut next = self.state.clone();
        next.attempts = next
            .attempts
            .checked_add(1)
            .ok_or_else(|| anyhow!("egress_policy_attempt_limit"))?;
        next.phase = Phase::RemoteOutcomeUnresolved;
        next.last_error = None;
        self.publish(next)
    }
    pub(super) fn failed_attempt(&mut self, code: &'static str) -> Result<()> {
        ensure!(safe_code(code), "egress_policy_invalid_error_code");
        let mut next = self.state.clone();
        next.last_error = Some(code.into());
        self.publish(next)
    }
    pub(super) fn acknowledge(&mut self, receipt: &EgressPolicyReceiptV1) -> Result<()> {
        self.request.validate_receipt(receipt)?;
        let canonical = receipt
            .canonical_bytes()
            .map_err(|_| anyhow!("egress_policy_invalid_receipt"))?;
        ensure!(canonical.len() <= 4096, "egress_policy_invalid_receipt");
        fs_error(self.directory.record_receipt(&canonical))?;
        if self.state.phase != Phase::ReceiptRecorded {
            let mut next = self.state.clone();
            next.phase = Phase::ReceiptRecorded;
            next.last_error = None;
            self.publish(next)?;
        }
        self.revalidate()
    }
}
