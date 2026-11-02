//! Closed durable D5-U records. Data here never grants admission or proves
//! actor revocation, a retained-FD lease, authenticated transport, or real join.
use super::journal::{self, JournalError, JournalResult, JournalScopeV1};
use crate::control::{
    ArmReceiptV1, ArmSubjectV1, ArmV1, HelperPermissionV1, HelperPurposeV1, HostClockSampleV1,
    MutationReceiptV1, RuntimeInspectionV1,
};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, Error as _},
};
use std::fmt;
use uuid::Uuid;

const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;

fn check(condition: bool) -> JournalResult<()> {
    if condition {
        Ok(())
    } else {
        Err(JournalError::InvalidInput)
    }
}

fn nonnil(id: Uuid) -> JournalResult<()> {
    check(!id.is_nil())
}

fn uuid_string(value: &str) -> JournalResult<()> {
    let id = Uuid::parse_str(value).map_err(|_| JournalError::InvalidInput)?;
    nonnil(id)?;
    check(id.hyphenated().to_string() == value)
}

pub(super) fn strict_uuid<'de, D: Deserializer<'de>>(d: D) -> Result<Uuid, D::Error> {
    let value = String::deserialize(d)?;
    uuid_string(&value).map_err(D::Error::custom)?;
    Uuid::parse_str(&value).map_err(D::Error::custom)
}

// Custom deserialization deliberately has no default: null is explicit, while
// an omitted Option field is an invalid durable record.
fn required_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

fn bytes(value: &impl Serialize) -> JournalResult<Vec<u8>> {
    let bytes = journal::canonical(value)?;
    if bytes.len() > MAX_RECORD_BYTES {
        Err(JournalError::TooLarge)
    } else {
        Ok(bytes)
    }
}

fn parse<T: DeserializeOwned + Serialize>(input: &[u8]) -> JournalResult<T> {
    if input.len() > MAX_RECORD_BYTES {
        return Err(JournalError::TooLarge);
    }
    let value: T = serde_json::from_slice(input).map_err(|_| JournalError::InvalidInput)?;
    // Also rejects duplicate keys, whitespace, alternative numeric spellings
    // and nested legacy UUID normalization. Historical codecs stay unchanged.
    check(bytes(&value)? == input)?;
    Ok(value)
}

fn same<T: Serialize>(left: &T, right: &T) -> JournalResult<bool> {
    Ok(bytes(left)? == bytes(right)?)
}

/// A validated commitment, not a signature, capability or probability.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct RetirementDigestV1(String);
impl RetirementDigestV1 {
    pub(crate) fn parse(value: &str) -> JournalResult<Self> {
        journal::valid_hash(value)?;
        Ok(Self(value.into()))
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for RetirementDigestV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetirementDigestV1(<commitment>)")
    }
}
impl<'de> Deserialize<'de> for RetirementDigestV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

// Every wire struct is closed and independently validates after decoding.
// The wire type stays private; constructors never accept arbitrary JSON.
// Helper selection affects only methods, never the serialized fields or codec.
macro_rules! record_helper {
    (; $item:item) => {
        $item
    };
    (prod; $item:item) => {
        $item
    };
    (test; $item:item) => {
        #[cfg(test)]
        $item
    };
    (none; $item:item) => {};
}

macro_rules! record {
    ($name:ident, $version:literal, $encode:ident, $decode:ident, {
        $($(#[$attr:meta])* $field:ident: $ty:ty $(=> $getter:ident)?),* $(,)?
    }) => {
        #[derive(Clone, Serialize)]
        pub(crate) struct $name { version: u32, $($(#[$attr])* $field: $ty),* }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "(<durable data>)"))
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Wire { version: u32, $($(#[$attr])* $field: $ty),* }
                let mut ignored = false;
                let wire: Wire = serde_ignored::deserialize(d, |_| ignored = true)
                    .map_err(|_| D::Error::custom("invalid retirement record"))?;
                if ignored { return Err(D::Error::custom("unknown retirement field")); }
                let value = Self { version: wire.version, $($field: wire.$field),* };
                value.validate().map_err(D::Error::custom)?;
                Ok(value)
            }
        }
        impl $name {
            fn version_valid(&self) -> JournalResult<()> {
                if self.version == $version { Ok(()) } else { Err(JournalError::UnsupportedVersion) }
            }
            record_helper!($encode; pub(crate) fn canonical_bytes(&self) -> JournalResult<Vec<u8>> {
                self.validate()?;
                bytes(self)
            });
            record_helper!($decode; pub(crate) fn from_canonical_bytes(input: &[u8]) -> JournalResult<Self> { parse(input) });
            $(record_helper!($($getter)?; pub(crate) fn $field(&self) -> &$ty { &self.$field });)*
        }
    };
}

macro_rules! hashed {
    ($name:ident, $domain:literal) => {
        impl $name {
            pub(crate) fn digest(&self) -> JournalResult<RetirementDigestV1> {
                RetirementDigestV1::parse(&journal::digest($domain, &self.canonical_bytes()?))
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementReasonV1 {
    UnusedPrepareRevoked,
}

record!(RetirementIdentityV1, 1, prod, test, {
    scope: JournalScopeV1,
    #[serde(deserialize_with = "strict_uuid")] job_operation_id: Uuid,
    intent_hash: RetirementDigestV1,
    #[serde(deserialize_with = "strict_arm")] arm: ArmV1,
    arm_receipt: ArmReceiptV1,
    registration_hash: RetirementDigestV1,
    #[serde(deserialize_with = "strict_uuid")] original_epoch: Uuid,
    generation: u64,
    #[serde(deserialize_with = "strict_uuid")] retirement_id: Uuid,
    reason: RetirementReasonV1 => none,
});
hashed!(
    RetirementIdentityV1,
    "ai-guard:broker-retirement-identity:v1"
);

// Internally tagged unit variants consume buffered unknown fields in Serde,
// even with deny_unknown_fields. An outer serde_ignored guard cannot see them.
// Empty struct variants close only this new wire boundary, preserving the
// historical Arm codec and existing in-memory subject variants.
fn strict_arm_subject<'de, D: Deserializer<'de>>(d: D) -> Result<ArmSubjectV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Wire {
        Worker {},
        Helper {
            helper_id: String,
            purpose: HelperPurposeV1,
            policy_version: u32,
            permission: Box<HelperPermissionV1>,
        },
    }
    match Wire::deserialize(d)? {
        Wire::Worker {} => Ok(ArmSubjectV1::Worker),
        Wire::Helper {
            helper_id,
            purpose,
            policy_version,
            permission,
        } => Ok(ArmSubjectV1::Helper {
            helper_id,
            purpose,
            policy_version,
            permission,
        }),
    }
}

fn strict_arm<'de, D: Deserializer<'de>>(d: D) -> Result<ArmV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(remote = "ArmV1", deny_unknown_fields)]
    struct Wire {
        version: u32,
        operation_id: String,
        worker_id: String,
        attempt_id: String,
        expected_revision: u64,
        intent_hash: String,
        #[serde(deserialize_with = "strict_arm_subject")]
        subject: ArmSubjectV1,
        inspection: RuntimeInspectionV1,
        authority_binding_hash: String,
        configuration_fingerprint_version: u32,
        configuration_fingerprint: String,
        clock: HostClockSampleV1,
        timeout_seconds: u32,
        deadline_ns_decimal: String,
    }
    Wire::deserialize(d)
}

fn receipt_uuids(receipt: &ArmReceiptV1) -> JournalResult<()> {
    for id in [
        &receipt.arm_id,
        &receipt.worker_id,
        &receipt.attempt_id,
        &receipt.control_namespace_uuid,
    ] {
        uuid_string(id)?;
    }
    Ok(())
}

fn mutation_uuids(receipt: &MutationReceiptV1) -> JournalResult<()> {
    uuid_string(&receipt.operation_id)?;
    if let Some(id) = &receipt.worker_id {
        uuid_string(id)?;
    }
    Ok(())
}

fn arm_uuids(arm: &ArmV1) -> JournalResult<()> {
    for id in [
        &arm.operation_id,
        &arm.worker_id,
        &arm.attempt_id,
        &arm.inspection.authority_id,
        &arm.clock.boot_id,
    ] {
        uuid_string(id)?;
    }
    if let ArmSubjectV1::Helper {
        helper_id,
        permission,
        ..
    } = &arm.subject
    {
        uuid_string(helper_id)?;
        match permission.as_ref() {
            HelperPermissionV1::Admission { receipt }
            | HelperPermissionV1::Export { receipt, .. } => mutation_uuids(receipt)?,
            HelperPermissionV1::ImportVerification {
                import_helper_id,
                import_arm,
                import_terminal,
                ..
            } => {
                uuid_string(import_helper_id)?;
                receipt_uuids(import_arm)?;
                mutation_uuids(import_terminal)?;
            }
        }
        if let HelperPermissionV1::Export {
            destination_ref, ..
        } = permission.as_ref()
        {
            uuid_string(destination_ref)?;
        }
    }
    Ok(())
}

impl RetirementIdentityV1 {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        scope: JournalScopeV1,
        job_operation_id: Uuid,
        intent_hash: RetirementDigestV1,
        arm: ArmV1,
        arm_receipt: ArmReceiptV1,
        registration_hash: RetirementDigestV1,
        original_epoch: Uuid,
        generation: u64,
        retirement_id: Uuid,
        reason: RetirementReasonV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            scope,
            job_operation_id,
            intent_hash,
            arm,
            arm_receipt,
            registration_hash,
            original_epoch,
            generation,
            retirement_id,
            reason,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        // Scope validation is retained without changing its legacy codec.
        let _: JournalScopeV1 =
            serde_json::from_slice(&bytes(&self.scope)?).map_err(|_| JournalError::InvalidInput)?;
        nonnil(self.scope.installation_id())?;
        nonnil(self.scope.control_namespace_uuid())?;
        nonnil(self.job_operation_id)?;
        nonnil(self.original_epoch)?;
        nonnil(self.retirement_id)?;
        self.arm
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        self.arm_receipt
            .validate_for_arm(&self.arm)
            .map_err(|_| JournalError::InvalidInput)?;
        arm_uuids(&self.arm)?;
        receipt_uuids(&self.arm_receipt)?;
        check(
            self.intent_hash.as_str() == self.arm.intent_hash
                && self.arm_receipt.control_namespace_uuid
                    == self.scope.control_namespace_uuid().to_string(),
        )?;
        // The registration digest binds the complete private snapshot and
        // invocation. Only its retained owner can check it against live state.
        Ok(())
    }
}

record!(RetirementEvidenceV1, 1, prod, none, {
    identity_hash: RetirementDigestV1,
    #[serde(deserialize_with = "strict_uuid")] original_actor_nonce: Uuid,
    #[serde(deserialize_with = "strict_uuid")] prepare_revocation_nonce: Uuid,
});
hashed!(
    RetirementEvidenceV1,
    "ai-guard:broker-retirement-evidence:v1"
);
impl RetirementEvidenceV1 {
    pub(crate) fn new(
        identity_hash: RetirementDigestV1,
        original_actor_nonce: Uuid,
        prepare_revocation_nonce: Uuid,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            identity_hash,
            original_actor_nonce,
            prepare_revocation_nonce,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        nonnil(self.original_actor_nonce)?;
        nonnil(self.prepare_revocation_nonce)
    }
}

record!(RetirementAuditAckV1, 1, none, none, {
    #[serde(deserialize_with = "strict_uuid")] event_id: Uuid,
    input_hash: RetirementDigestV1,
    audit_seq: i64,
});
impl RetirementAuditAckV1 {
    pub(crate) fn new(
        event_id: Uuid,
        input_hash: RetirementDigestV1,
        audit_seq: i64,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            event_id,
            input_hash,
            audit_seq,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        nonnil(self.event_id)?;
        check(self.audit_seq > 0)
    }
}

record!(GenesisCoreV1, 1, prod, prod, {
    #[serde(deserialize_with = "strict_uuid")] installation_id: Uuid => none,
    #[serde(deserialize_with = "strict_uuid")] role_id: Uuid => none,
    #[serde(deserialize_with = "strict_uuid")] init_id: Uuid => none,
    native_binding: [u64; 6],
});
hashed!(GenesisCoreV1, "ai-guard:enforcer-retirement-genesis:v1");
impl GenesisCoreV1 {
    /// The tuple is data from the retained lease, not proof of filesystem state.
    /// Its order is root dev/ino/uid, lock dev/ino, effective uid.
    pub(crate) fn new(
        installation_id: Uuid,
        role_id: Uuid,
        init_id: Uuid,
        native_binding: [u64; 6],
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            installation_id,
            role_id,
            init_id,
            native_binding,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        nonnil(self.installation_id)?;
        nonnil(self.role_id)?;
        nonnil(self.init_id)
    }
}

record!(LedgerIdentityV1, 1, none, none, {
    #[serde(deserialize_with = "strict_uuid")] installation_id: Uuid,
    #[serde(deserialize_with = "strict_uuid")] role_id: Uuid => test,
    #[serde(deserialize_with = "strict_uuid")] init_id: Uuid => test,
    native_binding: [u64; 6],
    genesis_hash: RetirementDigestV1 => none,
});
impl LedgerIdentityV1 {
    pub(crate) fn from_genesis(genesis: &GenesisCoreV1) -> JournalResult<Self> {
        genesis.validate()?;
        Ok(Self {
            version: 1,
            installation_id: genesis.installation_id,
            role_id: genesis.role_id,
            init_id: genesis.init_id,
            native_binding: genesis.native_binding,
            genesis_hash: genesis.digest()?,
        })
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        let genesis = GenesisCoreV1::new(
            self.installation_id,
            self.role_id,
            self.init_id,
            self.native_binding,
        )?;
        check(genesis.digest()? == self.genesis_hash)
    }
    pub(crate) fn matches(&self, other: &Self) -> JournalResult<bool> {
        self.validate()?;
        other.validate()?;
        same(self, other)
    }
}

record!(RoleSealV2, 2, prod, prod, { ledger: LedgerIdentityV1 });
#[cfg(test)]
hashed!(RoleSealV2, "ai-guard:enforcer-role-seal:v2");
impl RoleSealV2 {
    pub(crate) fn new(ledger: LedgerIdentityV1) -> JournalResult<Self> {
        let value = Self { version: 2, ledger };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.ledger.validate()
    }
}

record!(LedgerHeadV1, 1, prod, prod, { ledger: LedgerIdentityV1, revision: u64, hash: RetirementDigestV1 });
impl LedgerHeadV1 {
    pub(crate) fn new(
        ledger: LedgerIdentityV1,
        revision: u64,
        hash: RetirementDigestV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            ledger,
            revision,
            hash,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.ledger.validate()
    }
}

record!(LedgerFloorV1, 1, prod, prod, {
    ledger: LedgerIdentityV1,
    revision: u64,
    #[serde(deserialize_with = "required_option")] delta_hash: Option<RetirementDigestV1>,
    #[serde(deserialize_with = "required_option")] previous_floor_hash: Option<RetirementDigestV1>,
});
hashed!(LedgerFloorV1, "ai-guard:enforcer-ledger-floor:v1");
impl LedgerFloorV1 {
    pub(crate) fn new(
        ledger: LedgerIdentityV1,
        revision: u64,
        delta_hash: Option<RetirementDigestV1>,
        previous_floor_hash: Option<RetirementDigestV1>,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            ledger,
            revision,
            delta_hash,
            previous_floor_hash,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.ledger.validate()?;
        check(if self.revision == 0 {
            self.delta_hash.is_none() && self.previous_floor_hash.is_none()
        } else {
            self.delta_hash.is_some() && self.previous_floor_hash.is_some()
        })
    }
}

record!(ClosingAcceptanceV1, 1, prod, none, {
    identity: RetirementIdentityV1,
    evidence: RetirementEvidenceV1,
    audit_ack: RetirementAuditAckV1,
    #[serde(deserialize_with = "strict_uuid")] closing_nonce: Uuid => test,
});
hashed!(
    ClosingAcceptanceV1,
    "ai-guard:enforcer-closing-acceptance:v1"
);
impl ClosingAcceptanceV1 {
    pub(crate) fn new(
        identity: RetirementIdentityV1,
        evidence: RetirementEvidenceV1,
        audit_ack: RetirementAuditAckV1,
        closing_nonce: Uuid,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            identity,
            evidence,
            audit_ack,
            closing_nonce,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.identity.validate()?;
        self.evidence.validate()?;
        self.audit_ack.validate()?;
        nonnil(self.closing_nonce)?;
        check(self.identity.digest()? == self.evidence.identity_hash)
        // audit_ack.input_hash belongs to the intention event, not identity.
    }
}

record!(RetiredTombstoneV1, 1, prod, none, {
    ledger: LedgerIdentityV1,
    identity_hash: RetirementDigestV1,
    closing_hash: RetirementDigestV1,
});
hashed!(RetiredTombstoneV1, "ai-guard:enforcer-retired-tombstone:v1");
impl RetiredTombstoneV1 {
    pub(crate) fn new(
        ledger: LedgerIdentityV1,
        identity_hash: RetirementDigestV1,
        closing_hash: RetirementDigestV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            ledger,
            identity_hash,
            closing_hash,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.ledger.validate()
    }
}

record!(JoinedQuiescenceV1, 1, test, none, {
    identity_hash: RetirementDigestV1,
    closing_hash: RetirementDigestV1,
    tombstone_hash: RetirementDigestV1,
    #[serde(deserialize_with = "strict_uuid")] original_epoch: Uuid,
    #[serde(deserialize_with = "strict_uuid")] original_actor_nonce: Uuid,
});
#[cfg(test)]
hashed!(JoinedQuiescenceV1, "ai-guard:enforcer-joined-quiescence:v1");
impl JoinedQuiescenceV1 {
    pub(crate) fn new(
        identity_hash: RetirementDigestV1,
        closing_hash: RetirementDigestV1,
        tombstone_hash: RetirementDigestV1,
        original_epoch: Uuid,
        original_actor_nonce: Uuid,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            identity_hash,
            closing_hash,
            tombstone_hash,
            original_epoch,
            original_actor_nonce,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        nonnil(self.original_epoch)?;
        nonnil(self.original_actor_nonce)
    }
}

record!(FinalizationCoreV1, 1, test, none, {
    identity_hash: RetirementDigestV1 => none,
    closing_hash: RetirementDigestV1,
    tombstone_hash: RetirementDigestV1,
    joined: JoinedQuiescenceV1,
});
#[cfg(test)]
hashed!(FinalizationCoreV1, "ai-guard:enforcer-finalization-core:v1");
impl FinalizationCoreV1 {
    pub(crate) fn new(
        identity_hash: RetirementDigestV1,
        closing_hash: RetirementDigestV1,
        tombstone_hash: RetirementDigestV1,
        joined: JoinedQuiescenceV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            identity_hash,
            closing_hash,
            tombstone_hash,
            joined,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.joined.validate()?;
        check(
            self.identity_hash == self.joined.identity_hash
                && self.closing_hash == self.joined.closing_hash
                && self.tombstone_hash == self.joined.tombstone_hash,
        )
    }
}

record!(RetiredReceiptV1, 1, prod, none, {
    ledger: LedgerIdentityV1,
    #[serde(deserialize_with = "strict_uuid")] retirement_id: Uuid,
    identity_hash: RetirementDigestV1,
    finalization: FinalizationCoreV1,
});
hashed!(RetiredReceiptV1, "ai-guard:enforcer-retired-receipt:v1");
impl RetiredReceiptV1 {
    pub(crate) fn new(
        ledger: LedgerIdentityV1,
        retirement_id: Uuid,
        identity_hash: RetirementDigestV1,
        finalization: FinalizationCoreV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            ledger,
            retirement_id,
            identity_hash,
            finalization,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.ledger.validate()?;
        nonnil(self.retirement_id)?;
        self.finalization.validate()?;
        check(self.identity_hash == self.finalization.identity_hash)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RetirementLedgerPhaseV1 {
    Closing,
    Tombstone {
        tombstone_hash: RetirementDigestV1,
    },
    Final {
        tombstone_hash: RetirementDigestV1,
        receipt: Box<RetiredReceiptV1>,
    },
}

impl<'de> Deserialize<'de> for RetirementLedgerPhaseV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Closing {},
            Tombstone {
                tombstone_hash: RetirementDigestV1,
            },
            Final {
                tombstone_hash: RetirementDigestV1,
                receipt: Box<RetiredReceiptV1>,
            },
        }
        match Wire::deserialize(d)? {
            Wire::Closing {} => Ok(Self::Closing),
            Wire::Tombstone { tombstone_hash } => Ok(Self::Tombstone { tombstone_hash }),
            Wire::Final {
                tombstone_hash,
                receipt,
            } => Ok(Self::Final {
                tombstone_hash,
                receipt,
            }),
        }
    }
}

record!(RetirementLedgerEntryV1, 1, prod, test, {
    accepted: ClosingAcceptanceV1,
    expected_tombstone_hash: RetirementDigestV1,
    phase: RetirementLedgerPhaseV1,
});
hashed!(
    RetirementLedgerEntryV1,
    "ai-guard:enforcer-retirement-entry:v1"
);
impl RetirementLedgerEntryV1 {
    pub(crate) fn new(
        accepted: ClosingAcceptanceV1,
        expected_tombstone_hash: RetirementDigestV1,
        phase: RetirementLedgerPhaseV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            accepted,
            expected_tombstone_hash,
            phase,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.accepted.validate()?;
        match &self.phase {
            RetirementLedgerPhaseV1::Closing => Ok(()),
            RetirementLedgerPhaseV1::Tombstone { tombstone_hash } => {
                check(*tombstone_hash == self.expected_tombstone_hash)
            }
            RetirementLedgerPhaseV1::Final {
                tombstone_hash,
                receipt,
            } => {
                receipt.validate()?;
                let identity = &self.accepted.identity;
                let core = &receipt.finalization;
                check(
                    *tombstone_hash == self.expected_tombstone_hash
                        && core.tombstone_hash == self.expected_tombstone_hash
                        && receipt.identity_hash == identity.digest()?
                        && receipt.retirement_id == identity.retirement_id
                        && receipt.ledger.installation_id == identity.scope.installation_id()
                        && core.closing_hash == self.accepted.digest()?
                        && core.joined.original_epoch == identity.original_epoch
                        && core.joined.original_actor_nonce
                            == self.accepted.evidence.original_actor_nonce,
                )
            }
        }
    }
    /// Pure data correlation with the ledger. Does not prove a lease or writer.
    pub(crate) fn validate_for_ledger(&self, ledger: &LedgerIdentityV1) -> JournalResult<()> {
        self.validate()?;
        ledger.validate()?;
        check(ledger.installation_id == self.accepted.identity.scope.installation_id())?;
        let tombstone = RetiredTombstoneV1::new(
            ledger.clone(),
            self.accepted.identity.digest()?,
            self.accepted.digest()?,
        )?;
        check(tombstone.digest()? == self.expected_tombstone_hash)?;
        if let RetirementLedgerPhaseV1::Final { receipt, .. } = &self.phase {
            check(receipt.ledger.matches(ledger)?)?;
        }
        Ok(())
    }
    /// Validate only immutable data and phase progression. Exact replay is
    /// allowed; authority, revision/CAS, floors and budgets belong to the owner.
    pub(crate) fn validate_successor(previous: Option<&Self>, next: &Self) -> JournalResult<()> {
        next.validate()?;
        let Some(previous) = previous else {
            return check(matches!(next.phase, RetirementLedgerPhaseV1::Closing));
        };
        previous.validate()?;
        if previous.expected_tombstone_hash != next.expected_tombstone_hash
            || !same(&previous.accepted, &next.accepted)?
        {
            return Err(JournalError::Conflict);
        }
        if same(previous, next)? {
            return Ok(());
        }
        check(matches!(
            (&previous.phase, &next.phase),
            (
                RetirementLedgerPhaseV1::Closing,
                RetirementLedgerPhaseV1::Tombstone { .. }
            ) | (
                RetirementLedgerPhaseV1::Tombstone { .. },
                RetirementLedgerPhaseV1::Final { .. }
            )
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "entry",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum RetirementLedgerTransitionV1 {
    BeginClosing(RetirementLedgerEntryV1),
    CommitTombstone(RetirementLedgerEntryV1),
    Finalize(RetirementLedgerEntryV1),
}
impl RetirementLedgerTransitionV1 {
    pub(crate) fn entry(&self) -> &RetirementLedgerEntryV1 {
        match self {
            Self::BeginClosing(entry) | Self::CommitTombstone(entry) | Self::Finalize(entry) => {
                entry
            }
        }
    }
    fn validate(&self) -> JournalResult<()> {
        self.entry().validate()?;
        check(matches!(
            (self, &self.entry().phase),
            (Self::BeginClosing(_), RetirementLedgerPhaseV1::Closing)
                | (
                    Self::CommitTombstone(_),
                    RetirementLedgerPhaseV1::Tombstone { .. }
                )
                | (Self::Finalize(_), RetirementLedgerPhaseV1::Final { .. })
        ))
    }
}

record!(RetirementDeltaV1, 1, prod, prod, {
    ledger: LedgerIdentityV1,
    revision: u64,
    previous_hash: RetirementDigestV1,
    #[serde(deserialize_with = "strict_uuid")] arm_id: Uuid,
    #[serde(deserialize_with = "required_option")] expected_entry_hash: Option<RetirementDigestV1> => none,
    transition: RetirementLedgerTransitionV1,
});
hashed!(RetirementDeltaV1, "ai-guard:enforcer-retirement-delta:v1");
impl RetirementDeltaV1 {
    pub(crate) fn new(
        ledger: LedgerIdentityV1,
        revision: u64,
        previous_hash: RetirementDigestV1,
        arm_id: Uuid,
        expected_entry_hash: Option<RetirementDigestV1>,
        transition: RetirementLedgerTransitionV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            ledger,
            revision,
            previous_hash,
            arm_id,
            expected_entry_hash,
            transition,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        self.version_valid()?;
        self.ledger.validate()?;
        nonnil(self.arm_id)?;
        check(self.revision > 0)?;
        self.transition.validate()?;
        let entry = self.transition.entry();
        entry.validate_for_ledger(&self.ledger)?;
        check(entry.accepted.identity.arm.operation_id == self.arm_id.to_string())?;
        check(match self.transition {
            RetirementLedgerTransitionV1::BeginClosing(_) => self.expected_entry_hash.is_none(),
            _ => self.expected_entry_hash.is_some(),
        })
    }
    /// Data-only expected-entry CAS check. Ledger owner separately verifies the
    /// actual retained head/revision/floor and commits the transition durably.
    pub(crate) fn validate_previous(
        &self,
        previous: Option<&RetirementLedgerEntryV1>,
    ) -> JournalResult<()> {
        self.validate()?;
        match (previous, &self.expected_entry_hash) {
            (None, None) => (),
            (Some(entry), Some(hash)) => check(entry.digest()? == *hash)?,
            _ => return Err(JournalError::Conflict),
        }
        RetirementLedgerEntryV1::validate_successor(previous, self.transition.entry())
    }
}

#[cfg(test)]
#[path = "retirement_tests.rs"]
mod tests;
