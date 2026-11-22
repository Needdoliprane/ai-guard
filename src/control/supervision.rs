//! Strict supervision contracts. These types do not activate a broker or schema.

use super::*;
use uuid::Uuid;

macro_rules! dto {
    ($name:ident { $($field:ident: $kind:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $kind),* }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostClockMethodV1 {
    LinuxBoottimeV1,
    DarwinMachContinuousV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisionProtocolV1 {
    BrokerSupervisionV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperPurposeV1 {
    Import,
    ImportVerification,
    FinalScan,
    Export,
}

dto!(HostClockSampleV1 {
    version: u32,
    method: HostClockMethodV1,
    boot_id: String,
    continuous_ns_decimal: String,
    observed_utc: Option<String>,
});

// Storage/protocol facts only; the Store never manufactures an HTTP role.
dto!(SupervisionStateV1 {
    version: u32,
    schema_version: u32,
    control_namespace_uuid: Option<String>,
    protocols: Vec<SupervisionProtocolV1>,
    active: bool,
});

dto!(ControlProtocolV1 {
    version: u32,
    schema_version: u32,
    control_namespace_uuid: Option<String>,
    tenant_id: String,
    role: String,
    protocols: Vec<SupervisionProtocolV1>,
    active: bool,
});

dto!(SupervisedAdmissionV1 {
    version: u32,
    protocol: SupervisionProtocolV1,
    intent_hash: String,
    admission: AdmissionV1,
});

dto!(BindLegacyIntentV1 {
    version: u32,
    operation_id: String,
    worker_id: String,
    attempt_id: String,
    expected_revision: u64,
    intent_hash: String,
});

dto!(ArmReceiptV1 {
    version: u32,
    arm_id: String,
    worker_id: String,
    attempt_id: String,
    input_hash: String,
    control_namespace_uuid: String,
    audit_seq: i64,
    audit_hash: String,
});

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperPermissionV1 {
    Admission {
        receipt: MutationReceiptV1,
    },
    ImportVerification {
        import_helper_id: String,
        import_arm: ArmReceiptV1,
        import_terminal: MutationReceiptV1,
        manifest_digest: String,
    },
    Export {
        receipt: MutationReceiptV1,
        destination_ref: String,
        destination_intent_hash: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArmSubjectV1 {
    Worker,
    Helper {
        helper_id: String,
        purpose: HelperPurposeV1,
        policy_version: u32,
        permission: Box<HelperPermissionV1>,
    },
}

dto!(ArmV1 {
    version: u32,
    operation_id: String,
    worker_id: String,
    attempt_id: String,
    expected_revision: u64,
    intent_hash: String,
    subject: ArmSubjectV1,
    inspection: RuntimeInspectionV1,
    authority_binding_hash: String,
    configuration_fingerprint_version: u32,
    configuration_fingerprint: String,
    clock: HostClockSampleV1,
    timeout_seconds: u32,
    deadline_ns_decimal: String,
});

dto!(ArmResultV1 {
    replayed: bool,
    arm: ArmV1,
    receipt: ArmReceiptV1,
});

dto!(ArmedStartClaimV1 {
    version: u32,
    operation_id: String,
    worker_id: String,
    attempt_id: String,
    expected_revision: u64,
    emitter_id: String,
    arm_receipt: ArmReceiptV1,
    clock_check: HostClockSampleV1,
});

dto!(HelperStartClaimV1 {
    version: u32,
    operation_id: String,
    helper_id: String,
    emitter_id: String,
    arm_receipt: ArmReceiptV1,
    clock_check: HostClockSampleV1,
});

dto!(HelperClaimResultV1 {
    receipt: MutationReceiptV1,
    disposition: ClaimDispositionV1,
});

dto!(ImportVerificationReportV1 {
    import_helper_id: String,
    expected_manifest_digest: String,
    observed_manifest_digest: String,
});

dto!(HelperObservationV1 {
    version: u32,
    operation_id: String,
    helper_id: String,
    arm_receipt: ArmReceiptV1,
    observation: RuntimeObservationV1,
    verification: Option<ImportVerificationReportV1>,
});

dto!(HelperCleanupV1 {
    version: u32,
    operation_id: String,
    helper_id: String,
    arm_receipt: ArmReceiptV1,
    removed: bool,
    error: Option<String>,
});

dto!(ExportRequestV1 {
    version: u32,
    operation_id: String,
    worker_id: String,
    attempt_id: String,
    destination_ref: String,
    destination_intent_hash: String,
});

// Shared normalized helper specification, without private paths or values.
dto!(HelperExecutionPolicyV1 {
    version: u32,
    parent_worker_id: String,
    parent_attempt_id: String,
    parent_spec_hash: String,
    session_id: String,
    workspace_manifest_digest: String,
    helper_id: String,
    purpose: HelperPurposeV1,
    authority_id: String,
    runtime_kind: RuntimeKindV1,
    image_id: String,
    stable_name: String,
    volume_name: String,
    command: CommandSpec,
    network: NetworkV1,
    security_preset: SecurityPresetV1,
    cpu_millis: u32,
    memory_bytes: u64,
    pids_limit: u32,
    timeout_seconds: u32,
    restart_policy: RestartPolicyV1,
    credential_binding_id: Option<String>,
    credential_names: Vec<String>,
});

fn counter(value: &str) -> anyhow::Result<u128> {
    invalid(
        !value.is_empty()
            && value.len() <= 39
            && (value == "0" || !value.starts_with('0'))
            && value.bytes().all(|byte| byte.is_ascii_digit()),
    )?;
    Ok(value
        .parse::<u128>()
        .map_err(|_| ControlFailure::InvalidRequest)?)
}

fn receipt_shape(receipt: &MutationReceiptV1, worker_id: &str) -> anyhow::Result<()> {
    uuid(&receipt.operation_id)?;
    hash(&receipt.input_hash)?;
    hash(&receipt.audit_hash)?;
    invalid(
        receipt.audit_seq > 0
            && receipt.worker_id.as_deref() == Some(worker_id)
            && receipt.revision.is_none_or(|revision| revision > 0),
    )
}

fn inspection_shape(inspection: &RuntimeInspectionV1) -> anyhow::Result<()> {
    uuid(&inspection.authority_id)?;
    bounded(&inspection.stable_name, 128)?;
    bounded(&inspection.volume_name, 128)?;
    hash(&inspection.container_id)?;
    hash(&inspection.spec_hash)?;
    hash(
        inspection
            .image_id
            .strip_prefix("sha256:")
            .ok_or(ControlFailure::InvalidRequest)?,
    )?;
    invalid(
        inspection.cpu_millis > 0
            && inspection.memory_bytes > 0
            && inspection.pids_limit > 0
            && inspection.security_preset.requires_no_engine_logs(),
    )
}

impl HostClockSampleV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1)?;
        uuid(&self.boot_id)?;
        invalid(!Uuid::parse_str(&self.boot_id)?.is_nil())?;
        counter(&self.continuous_ns_decimal)?;
        if let Some(value) = &self.observed_utc {
            bounded(value, 64)?;
            invalid(chrono::DateTime::parse_from_rfc3339(value).is_ok())?;
        }
        Ok(())
    }

    pub fn continuous_ns(&self) -> anyhow::Result<u128> {
        self.validate()?;
        counter(&self.continuous_ns_decimal)
    }
}

impl SupervisionStateV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
                    .contains(&self.schema_version),
        )?;
        if let Some(namespace) = &self.control_namespace_uuid {
            uuid(namespace)?;
        }
        if [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&self.schema_version) {
            invalid(
                self.control_namespace_uuid.is_some()
                    && self.protocols == [SupervisionProtocolV1::BrokerSupervisionV1],
            )
        } else {
            invalid(!self.active && self.protocols.is_empty())
        }
    }
}

/// The backend that verified the capabilities. It is evidence, not a switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlStorageBackendV1 {
    Sqlite,
    Mysql,
}

/// An operation the verified target actually serves, with its replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOperationV1 {
    RecoveryInspect,
    StopReconcile,
    SupervisedAdmission,
}

/// Private, runner-only answer to `GET /control/api/v1/control/capabilities`.
/// The client binds role, tenant and namespace before opening any journal and
/// never falls back to `ControlProtocolV1` or the public transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlCapabilitiesV1 {
    pub version: u32,
    pub role: String,
    pub tenant_id: String,
    pub control_namespace_uuid: Option<String>,
    pub storage: ControlStorageBackendV1,
    pub operations: Vec<ControlOperationV1>,
}

impl ControlCapabilitiesV1 {
    pub const MAX_BYTES: usize = 16 * 1024;

    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && self.role == "runner")?;
        crate::tenancy::validate_tenant_id(&self.tenant_id)?;
        if let Some(namespace) = &self.control_namespace_uuid {
            uuid(namespace)?;
        }
        // Closed, sorted and duplicate-free: one canonical answer per state.
        invalid(self.operations.windows(2).all(|pair| pair[0] < pair[1]))?;
        // An operation needs a verified namespace to be bound to.
        invalid(self.operations.is_empty() || self.control_namespace_uuid.is_some())
    }

    /// The SQLite mapping of its versioned supervision state. Admission also
    /// requires an active authority, as the proposal specifies.
    pub fn from_sqlite_state(tenant_id: &str, state: &SupervisionStateV1) -> Self {
        let served = state
            .protocols
            .contains(&SupervisionProtocolV1::BrokerSupervisionV1);
        let mut operations = Vec::new();
        if served {
            operations.push(ControlOperationV1::RecoveryInspect);
            operations.push(ControlOperationV1::StopReconcile);
            if state.active {
                operations.push(ControlOperationV1::SupervisedAdmission);
            }
        }
        Self {
            version: 1,
            role: "runner".into(),
            tenant_id: tenant_id.into(),
            control_namespace_uuid: state.control_namespace_uuid.clone(),
            storage: ControlStorageBackendV1::Sqlite,
            operations,
        }
    }
}

impl ControlProtocolV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        crate::tenancy::validate_tenant_id(&self.tenant_id)?;
        invalid(matches!(self.role.as_str(), "runner" | "viewer"))?;
        SupervisionStateV1 {
            version: self.version,
            schema_version: self.schema_version,
            control_namespace_uuid: self.control_namespace_uuid.clone(),
            protocols: self.protocols.clone(),
            active: self.active,
        }
        .validate()
    }
}

impl SupervisedAdmissionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1)?;
        hash(&self.intent_hash)?;
        self.admission.validate()?;
        uuid(&self.admission.launch.authority_id)?;
        invalid(
            self.admission
                .launch
                .security_preset
                .requires_no_engine_logs(),
        )
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:supervised-admission:v1", self)
    }
}

impl BindLegacyIntentV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && self.expected_revision > 0)?;
        uuid(&self.operation_id)?;
        uuid(&self.worker_id)?;
        uuid(&self.attempt_id)?;
        hash(&self.intent_hash)
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:bind-legacy-intent:v1", self)
    }
}

impl ArmReceiptV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && self.audit_seq > 0)?;
        for value in [
            &self.arm_id,
            &self.worker_id,
            &self.attempt_id,
            &self.control_namespace_uuid,
        ] {
            uuid(value)?;
        }
        hash(&self.input_hash)?;
        hash(&self.audit_hash)
    }

    pub fn validate_for_arm(&self, arm: &ArmV1) -> anyhow::Result<()> {
        self.validate()?;
        invalid(
            self.arm_id == arm.operation_id
                && self.worker_id == arm.worker_id
                && self.attempt_id == arm.attempt_id
                && self.input_hash == arm.input_hash()?,
        )
    }
}

impl HelperPermissionV1 {
    fn validate_for_parent(
        &self,
        parent: &WorkerV1,
        purpose: HelperPurposeV1,
    ) -> anyhow::Result<()> {
        match (purpose, self) {
            (HelperPurposeV1::Import | HelperPurposeV1::FinalScan, Self::Admission { receipt }) => {
                receipt_shape(receipt, &parent.worker_id)
            }
            (
                HelperPurposeV1::ImportVerification,
                Self::ImportVerification {
                    import_helper_id,
                    import_arm,
                    import_terminal,
                    manifest_digest,
                },
            ) => {
                uuid(import_helper_id)?;
                import_arm.validate()?;
                receipt_shape(import_terminal, &parent.worker_id)?;
                hash(manifest_digest)?;
                invalid(
                    import_arm.worker_id == parent.worker_id
                        && import_arm.attempt_id == parent.attempt.attempt_id
                        && manifest_digest == &parent.launch.workspace_manifest_digest,
                )
            }
            (
                HelperPurposeV1::Export,
                Self::Export {
                    receipt,
                    destination_ref,
                    destination_intent_hash,
                },
            ) => {
                receipt_shape(receipt, &parent.worker_id)?;
                uuid(destination_ref)?;
                hash(destination_intent_hash)
            }
            _ => invalid(false),
        }
    }
}

impl ArmV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && self.expected_revision > 0
                && (1..=86400).contains(&self.timeout_seconds),
        )?;
        for value in [&self.operation_id, &self.worker_id, &self.attempt_id] {
            uuid(value)?;
        }
        for value in [
            &self.intent_hash,
            &self.authority_binding_hash,
            &self.configuration_fingerprint,
        ] {
            hash(value)?;
        }
        inspection_shape(&self.inspection)?;
        invalid(
            self.configuration_fingerprint_version
                == self
                    .inspection
                    .security_preset
                    .configuration_fingerprint_version(),
        )?;
        let armed = self.clock.continuous_ns()?;
        let duration = u128::from(self.timeout_seconds)
            .checked_mul(1_000_000_000)
            .ok_or(ControlFailure::InvalidRequest)?;
        invalid(armed.checked_add(duration) == Some(counter(&self.deadline_ns_decimal)?))?;
        if let ArmSubjectV1::Helper {
            helper_id,
            policy_version,
            ..
        } = &self.subject
        {
            uuid(helper_id)?;
            invalid(*policy_version == 1 && self.timeout_seconds == 120)?;
        }
        invalid(serde_json::to_vec(self)?.len() <= 65536)
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:execution-arm:v1", self)
    }

    pub fn subject_key(&self) -> anyhow::Result<String> {
        self.validate()?;
        Ok(match &self.subject {
            ArmSubjectV1::Worker => format!("worker:{}", self.attempt_id),
            ArmSubjectV1::Helper {
                helper_id, purpose, ..
            } => match purpose {
                HelperPurposeV1::Import => format!("import:{}", self.attempt_id),
                HelperPurposeV1::ImportVerification => format!("import_verification:{helper_id}"),
                HelperPurposeV1::FinalScan => format!("final_scan:{helper_id}"),
                HelperPurposeV1::Export => format!("export:{helper_id}"),
            },
        })
    }

    /// Shape/parent checks only; durable permission and receipt lookup belongs to Store.
    pub fn validate_for_parent(&self, parent: &WorkerV1) -> anyhow::Result<()> {
        self.validate()?;
        invalid(
            self.worker_id == parent.worker_id
                && self.attempt_id == parent.attempt.attempt_id
                && self.expected_revision == parent.revision
                && self.inspection.authority_id == parent.launch.authority_id
                && self.inspection.runtime_kind == parent.launch.runtime_kind,
        )?;
        match &self.subject {
            ArmSubjectV1::Worker => invalid(
                parent.state == WorkerStateV1::Created
                    && parent.quota_held
                    && parent.attempt.start_operation_id.is_none()
                    && parent.attempt.container_id.as_deref()
                        == Some(self.inspection.container_id.as_str())
                    && parent.attempt.created_inspection.as_ref() == Some(&self.inspection)
                    && self.inspection.spec_hash == parent.spec_hash
                    && self.timeout_seconds == parent.launch.timeout_seconds,
            ),
            ArmSubjectV1::Helper {
                helper_id,
                purpose,
                permission,
                ..
            } => {
                permission.validate_for_parent(parent, *purpose)?;
                let helper =
                    Uuid::parse_str(helper_id).map_err(|_| ControlFailure::InvalidRequest)?;
                let policy = helper_execution_policy_v1(parent, *purpose, helper)?;
                policy.validate_inspection(&self.inspection)?;
                match purpose {
                    HelperPurposeV1::Import | HelperPurposeV1::ImportVerification => invalid(
                        parent.state == WorkerStateV1::Preparing
                            && parent.quota_held
                            && parent.attempt.create_operation_id.is_none()
                            && parent.attempt.start_operation_id.is_none(),
                    ),
                    HelperPurposeV1::FinalScan | HelperPurposeV1::Export => {
                        invalid(parent.state == WorkerStateV1::Terminal && !parent.quota_held)
                    }
                }
            }
        }
    }

    pub fn validate_clock_check(&self, check: &HostClockSampleV1) -> anyhow::Result<()> {
        self.validate()?;
        let current = check.continuous_ns()?;
        invalid(
            check.method == self.clock.method
                && check.boot_id == self.clock.boot_id
                && current >= self.clock.continuous_ns()?
                && current < counter(&self.deadline_ns_decimal)?,
        )
    }
}

impl ArmedStartClaimV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && self.expected_revision > 0)?;
        for value in [
            &self.operation_id,
            &self.worker_id,
            &self.attempt_id,
            &self.emitter_id,
        ] {
            uuid(value)?;
        }
        self.arm_receipt.validate()?;
        self.clock_check.validate()?;
        invalid(
            self.worker_id == self.arm_receipt.worker_id
                && self.attempt_id == self.arm_receipt.attempt_id,
        )
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:armed-start:v1", self)
    }

    pub fn validate_for_arm(&self, arm: &ArmV1) -> anyhow::Result<()> {
        self.validate()?;
        self.arm_receipt.validate_for_arm(arm)?;
        invalid(matches!(&arm.subject, ArmSubjectV1::Worker))?;
        invalid(self.expected_revision == arm.expected_revision)?;
        arm.validate_clock_check(&self.clock_check)
    }
}

impl HelperStartClaimV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1)?;
        for value in [&self.operation_id, &self.helper_id, &self.emitter_id] {
            uuid(value)?;
        }
        self.arm_receipt.validate()?;
        self.clock_check.validate()
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:helper-start:v1", self)
    }

    pub fn validate_for_arm(&self, arm: &ArmV1) -> anyhow::Result<()> {
        self.validate()?;
        self.arm_receipt.validate_for_arm(arm)?;
        invalid(
            matches!(&arm.subject, ArmSubjectV1::Helper { helper_id, .. } if helper_id == &self.helper_id),
        )?;
        arm.validate_clock_check(&self.clock_check)
    }
}

impl HelperObservationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1)?;
        uuid(&self.operation_id)?;
        uuid(&self.helper_id)?;
        self.arm_receipt.validate()?;
        inspection_shape(&self.observation.inspection)?;
        bounded(&self.observation.observed_at, 64)?;
        invalid(chrono::DateTime::parse_from_rfc3339(&self.observation.observed_at).is_ok())?;
        for value in [&self.observation.started_at, &self.observation.finished_at]
            .into_iter()
            .flatten()
        {
            bounded(value, 64)?;
        }
        if let Some(report) = &self.verification {
            uuid(&report.import_helper_id)?;
            hash(&report.expected_manifest_digest)?;
            hash(&report.observed_manifest_digest)?;
        }
        Ok(())
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:helper-observation:v1", self)
    }
}

impl HelperCleanupV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && (!self.removed || self.error.is_none()))?;
        uuid(&self.operation_id)?;
        uuid(&self.helper_id)?;
        self.arm_receipt.validate()?;
        if let Some(value) = &self.error {
            bounded(value, 1024)?;
        }
        Ok(())
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:helper-cleanup:v1", self)
    }
}

impl ExportRequestV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1)?;
        for value in [
            &self.operation_id,
            &self.worker_id,
            &self.attempt_id,
            &self.destination_ref,
        ] {
            uuid(value)?;
        }
        hash(&self.destination_intent_hash)
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:export-request:v1", self)
    }
}

fn helper_command(purpose: HelperPurposeV1) -> CommandSpec {
    let arguments: &[&str] = match purpose {
        HelperPurposeV1::Import => &[
            "/bin/tar",
            "--extract",
            "--file=-",
            "--directory=/workspace",
            "--no-same-owner",
            "--no-same-permissions",
        ],
        HelperPurposeV1::ImportVerification
        | HelperPurposeV1::FinalScan
        | HelperPurposeV1::Export => &[
            "/bin/tar",
            "--create",
            "--file=-",
            "--directory=/workspace",
            ".",
        ],
    };
    CommandSpec {
        argv: arguments.iter().map(|value| (*value).to_owned()).collect(),
        cwd: "/workspace".into(),
    }
}

pub fn helper_execution_policy_v1(
    parent: &WorkerV1,
    purpose: HelperPurposeV1,
    helper_id: Uuid,
) -> anyhow::Result<HelperExecutionPolicyV1> {
    parent.launch.validate()?;
    uuid(&parent.worker_id)?;
    uuid(&parent.attempt.attempt_id)?;
    invalid(!helper_id.is_nil() && parent.spec_hash == parent.launch.spec_hash()?)?;
    let policy = HelperExecutionPolicyV1 {
        version: 1,
        parent_worker_id: parent.worker_id.clone(),
        parent_attempt_id: parent.attempt.attempt_id.clone(),
        parent_spec_hash: parent.spec_hash.clone(),
        session_id: parent.launch.session_id.clone(),
        workspace_manifest_digest: parent.launch.workspace_manifest_digest.clone(),
        helper_id: helper_id.to_string(),
        purpose,
        authority_id: parent.launch.authority_id.clone(),
        runtime_kind: parent.launch.runtime_kind,
        image_id: parent.launch.image_id.clone(),
        stable_name: format!("{}-h-{}", parent.attempt.stable_name, helper_id.simple()),
        volume_name: parent.attempt.volume_name.clone(),
        command: helper_command(purpose),
        network: NetworkV1::None,
        security_preset: SecurityPresetV1::FixedSandboxV2,
        cpu_millis: 2000,
        memory_bytes: 1073741824,
        pids_limit: 256,
        timeout_seconds: 120,
        restart_policy: RestartPolicyV1::No,
        credential_binding_id: None,
        credential_names: Vec::new(),
    };
    policy.validate()?;
    Ok(policy)
}

impl HelperExecutionPolicyV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && self.command == helper_command(self.purpose)
                && self.network == NetworkV1::None
                && self.security_preset == SecurityPresetV1::FixedSandboxV2
                && self.cpu_millis == 2000
                && self.memory_bytes == 1073741824
                && self.pids_limit == 256
                && self.timeout_seconds == 120
                && self.restart_policy == RestartPolicyV1::No
                && self.credential_binding_id.is_none()
                && self.credential_names.is_empty(),
        )?;
        for value in [
            &self.parent_worker_id,
            &self.parent_attempt_id,
            &self.session_id,
            &self.helper_id,
            &self.authority_id,
        ] {
            uuid(value)?;
        }
        hash(&self.parent_spec_hash)?;
        hash(&self.workspace_manifest_digest)?;
        hash(
            self.image_id
                .strip_prefix("sha256:")
                .ok_or(ControlFailure::InvalidRequest)?,
        )?;
        bounded(&self.stable_name, 128)?;
        bounded(&self.volume_name, 128)
    }

    pub fn spec_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:helper-execution-policy:v1", self)
    }

    pub fn validate_inspection(&self, inspection: &RuntimeInspectionV1) -> anyhow::Result<()> {
        self.validate()?;
        inspection_shape(inspection)?;
        invalid(
            inspection.authority_id == self.authority_id
                && inspection.runtime_kind == self.runtime_kind
                && inspection.stable_name == self.stable_name
                && inspection.volume_name == self.volume_name
                && inspection.image_id == self.image_id
                && inspection.spec_hash == self.spec_hash()?
                && inspection.cpu_millis == self.cpu_millis
                && inspection.memory_bytes == self.memory_bytes
                && inspection.pids_limit == self.pids_limit
                && inspection.network == self.network
                && inspection.security_preset == self.security_preset
                && inspection.restart_policy == self.restart_policy,
        )
    }
}

#[cfg(test)]
mod schema11_compatibility_tests {
    use super::*;

    fn supervised(schema_version: u32) -> SupervisionStateV1 {
        SupervisionStateV1 {
            version: 1,
            schema_version,
            control_namespace_uuid: Some(Uuid::from_u128(1).to_string()),
            protocols: vec![SupervisionProtocolV1::BrokerSupervisionV1],
            active: false,
        }
    }

    #[test]
    fn schema11_retains_supervision_v1_and_the_closed_schema_range() {
        for schema_version in 3..=15 {
            let state = supervised(schema_version);
            assert!(state.validate().is_ok(), "schema {schema_version}");
        }
        for schema_version in [1, 2] {
            let mut state = supervised(schema_version);
            state.control_namespace_uuid = None;
            state.protocols.clear();
            assert!(state.validate().is_ok());
            state.active = true;
            assert!(state.validate().is_err());
        }
        for schema_version in [0, 16, u32::MAX] {
            let mut state = supervised(schema_version);
            assert!(state.validate().is_err());
            // Unknown schemas cannot enter through the legacy inactive shape.
            state.control_namespace_uuid = None;
            state.protocols.clear();
            assert!(state.validate().is_err());
        }
    }

    #[test]
    fn schema11_keeps_control_wire_roles_namespace_and_fields_closed() {
        let state = supervised(11);
        let valid = ControlProtocolV1 {
            version: state.version,
            schema_version: state.schema_version,
            control_namespace_uuid: state.control_namespace_uuid,
            tenant_id: "tenant-a".into(),
            role: "runner".into(),
            protocols: state.protocols,
            active: false,
        };
        assert!(valid.validate().is_ok());
        let mut viewer = valid.clone();
        viewer.role = "viewer".into();
        assert!(viewer.validate().is_ok());

        let mut invalid = valid.clone();
        invalid.version = 2;
        assert!(invalid.validate().is_err());
        let mut invalid = valid.clone();
        invalid.role = "worker".into();
        assert!(invalid.validate().is_err());
        let mut invalid = valid.clone();
        invalid.tenant_id.clear();
        assert!(invalid.validate().is_err());
        let mut invalid = valid.clone();
        invalid.control_namespace_uuid = None;
        assert!(invalid.validate().is_err());
        let mut invalid = valid.clone();
        invalid.protocols.clear();
        assert!(invalid.validate().is_err());
        let mut invalid = valid.clone();
        invalid
            .protocols
            .push(SupervisionProtocolV1::BrokerSupervisionV1);
        assert!(invalid.validate().is_err());

        let mut wire = serde_json::to_value(&valid).unwrap();
        wire["session_authority"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ControlProtocolV1>(wire).is_err());
    }
}

#[cfg(test)]
mod schema14_human_source_metadata_tests {
    use super::*;

    #[test]
    fn schema14_describes_supervision_without_creating_activation_or_authority() {
        let state = SupervisionStateV1 {
            version: 1,
            schema_version: 14,
            control_namespace_uuid: Some(Uuid::from_u128(1).to_string()),
            protocols: vec![SupervisionProtocolV1::BrokerSupervisionV1],
            active: false,
        };
        assert!(state.validate().is_ok());
        assert!(!state.active);
        let wire = ControlProtocolV1 {
            version: state.version,
            schema_version: state.schema_version,
            control_namespace_uuid: state.control_namespace_uuid.clone(),
            tenant_id: "A".into(),
            role: "runner".into(),
            protocols: state.protocols.clone(),
            active: state.active,
        };
        assert!(wire.validate().is_ok());
        assert!(!wire.active);
        let mut missing_namespace = state.clone();
        missing_namespace.control_namespace_uuid = None;
        assert!(missing_namespace.validate().is_err());
        let mut missing_protocol = state.clone();
        missing_protocol.protocols.clear();
        assert!(missing_protocol.validate().is_err());
        let mut duplicate_protocol = state.clone();
        duplicate_protocol
            .protocols
            .push(SupervisionProtocolV1::BrokerSupervisionV1);
        assert!(duplicate_protocol.validate().is_err());
        let mut worker = wire.clone();
        worker.role = "worker".into();
        assert!(worker.validate().is_err());
        let mut unknown = state;
        unknown.schema_version = 16;
        assert!(unknown.validate().is_err());
    }
}

#[cfg(test)]
mod schema15_diagnostic_tests {
    use super::*;

    #[test]
    fn schema15_diagnostic_preserves_facts_and_the_closed_wire_contract() {
        let state = SupervisionStateV1 {
            version: 1,
            schema_version: 15,
            control_namespace_uuid: Some(Uuid::from_u128(1).to_string()),
            protocols: vec![SupervisionProtocolV1::BrokerSupervisionV1],
            active: false,
        };
        assert!(state.validate().is_ok());
        for role in ["runner", "viewer"] {
            let wire = ControlProtocolV1 {
                version: state.version,
                schema_version: state.schema_version,
                control_namespace_uuid: state.control_namespace_uuid.clone(),
                tenant_id: "A".into(),
                role: role.into(),
                protocols: state.protocols.clone(),
                active: state.active,
            };
            assert!(wire.validate().is_ok());
            assert!(!wire.active);
            let mut unknown = serde_json::to_value(&wire).unwrap();
            unknown["egress_enabled"] = serde_json::json!(true);
            assert!(serde_json::from_value::<ControlProtocolV1>(unknown).is_err());
        }
        let mut missing_namespace = state.clone();
        missing_namespace.control_namespace_uuid = None;
        assert!(missing_namespace.validate().is_err());
        let mut missing_protocol = state;
        missing_protocol.protocols.clear();
        assert!(missing_protocol.validate().is_err());
    }
}
