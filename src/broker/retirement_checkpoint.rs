//! Durable Full3 assignment data. A loaded binding is never an original-actor
//! witness, an authenticated connection, or authority to retire an actor.
use super::{
    journal::{JournalError, JournalResult},
    retirement::{LedgerIdentityV1, RetirementDigestV1, strict_uuid},
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::retirement::GenesisCoreV1;
    use serde_json::json;

    fn binding() -> Full3RegistrationBindingV1 {
        let genesis = GenesisCoreV1::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            [1, 2, 3, 4, 5, 6],
        )
        .unwrap();
        Full3RegistrationBindingV1::new(
            Uuid::from_u128(4),
            RetirementDigestV1::parse(&"a".repeat(64)).unwrap(),
            LedgerIdentityV1::from_genesis(&genesis).unwrap(),
            Uuid::from_u128(5),
        )
        .unwrap()
    }

    // Data-only codec fixtures. They never construct Unused, Authorized,
    // VerifiedRetirementReceipt or a retained JoinHandle witness.
    fn pending_data() -> PendingRetirementV1 {
        use crate::broker::retirement::{RetirementEvidenceV1, RetirementIdentityV1};
        let arm:crate::control::ArmV1=serde_json::from_value(json!({"version":1,"operation_id":Uuid::from_u128(10),
            "worker_id":Uuid::from_u128(11),"attempt_id":Uuid::from_u128(12),"expected_revision":1,"intent_hash":"e".repeat(64),
            "subject":{"kind":"worker"},"inspection":{"authority_id":Uuid::from_u128(13),"runtime_kind":"docker",
            "container_id":"a".repeat(64),"stable_name":"data-only","volume_name":"data-only-work","image_id":format!("sha256:{}","b".repeat(64)),
            "spec_hash":"d".repeat(64),"cpu_millis":1000,"memory_bytes":1073741824_u64,"pids_limit":256,"network":"none","security_preset":"fixed_sandbox_v2","restart_policy":"no"},
            "authority_binding_hash":"f".repeat(64),"configuration_fingerprint_version":2,"configuration_fingerprint":"c".repeat(64),
            "clock":{"version":1,"method":"darwin_mach_continuous_v1","boot_id":Uuid::from_u128(14),"continuous_ns_decimal":"1","observed_utc":null},
            "timeout_seconds":60,"deadline_ns_decimal":"60000000001"})).unwrap();
        let receipt:crate::control::ArmReceiptV1=serde_json::from_value(json!({"version":1,"arm_id":arm.operation_id,"worker_id":arm.worker_id,"attempt_id":arm.attempt_id,
            "input_hash":arm.input_hash().unwrap(),"control_namespace_uuid":Uuid::from_u128(15),"audit_seq":1,"audit_hash":"b".repeat(64)})).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-data",
            "http://127.0.0.1:8787",
            Uuid::from_u128(15),
            "data-only",
        )
        .unwrap();
        let identity = RetirementIdentityV1::new(
            scope,
            Uuid::from_u128(16),
            RetirementDigestV1::parse(&"e".repeat(64)).unwrap(),
            arm,
            receipt,
            RetirementDigestV1::parse(&"d".repeat(64)).unwrap(),
            Uuid::from_u128(5),
            0,
            Uuid::from_u128(17),
            crate::broker::retirement::RetirementReasonV1::UnusedPrepareRevoked,
        )
        .unwrap();
        let evidence = RetirementEvidenceV1::new(
            identity.digest().unwrap(),
            Uuid::from_u128(18),
            Uuid::from_u128(19),
        )
        .unwrap();
        PendingRetirementV1::new(
            identity,
            evidence,
            Uuid::from_u128(20),
            RetirementDigestV1::parse(&"a".repeat(64)).unwrap(),
            Uuid::from_u128(21),
            1024 * 1024,
            4096,
        )
        .unwrap()
    }

    #[test]
    fn pending_retirement_codec_uses_frozen_fields_and_explicit_ack() {
        let pending = pending_data();
        let value = serde_json::to_value(&pending).unwrap();
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "audit_ack",
                "evidence",
                "final_event_id",
                "identity",
                "intent_event_id",
                "intent_input_hash",
                "reserved_final_checkpoint_bytes",
                "reserved_final_event_bytes",
                "version"
            ]
        );
        let decoded: PendingRetirementV1 = serde_json::from_value(value.clone()).unwrap();
        assert!(pending.same_request(&decoded).unwrap());
        for key in [
            "audit_ack",
            "intent_input_hash",
            "reserved_final_checkpoint_bytes",
            "reserved_final_event_bytes",
        ] {
            let mut bad = value.clone();
            bad.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<PendingRetirementV1>(bad).is_err());
        }
        let mut insufficient = value.clone();
        insufficient["reserved_final_checkpoint_bytes"] = json!(1);
        assert!(serde_json::from_value::<PendingRetirementV1>(insufficient).is_err());
        let mut bad = value.clone();
        bad["version"] = json!(2);
        assert!(serde_json::from_value::<PendingRetirementV1>(bad).is_err());
        let mut bad = value;
        bad["accepted"] = json!({});
        assert!(serde_json::from_value::<PendingRetirementV1>(bad).is_err());
    }

    #[test]
    fn final_retirement_codec_preserves_pending_and_has_no_closing_or_head_extension() {
        use crate::broker::retirement::{FinalizationCoreV1, JoinedQuiescenceV1, RetiredReceiptV1};
        let mut pending = pending_data();
        pending.audit_ack = Some(
            RetirementAuditAckV1::new(
                pending.intention_event_id(),
                pending.intent_input_hash().clone(),
                1,
            )
            .unwrap(),
        );
        let identity = pending.identity();
        let ledger = binding().ledger().clone();
        let closing = RetirementDigestV1::parse(&"b".repeat(64)).unwrap();
        let tombstone =
            RetiredTombstoneV1::new(ledger.clone(), identity.digest().unwrap(), closing.clone())
                .unwrap()
                .digest()
                .unwrap();
        let joined = JoinedQuiescenceV1::new(
            identity.digest().unwrap(),
            closing.clone(),
            tombstone.clone(),
            *identity.original_epoch(),
            *pending.evidence().original_actor_nonce(),
        )
        .unwrap();
        let core = FinalizationCoreV1::new(identity.digest().unwrap(), closing, tombstone, joined)
            .unwrap();
        let receipt = RetiredReceiptV1::new(
            ledger,
            *identity.retirement_id(),
            identity.digest().unwrap(),
            core,
        )
        .unwrap();
        let final_entry = FinalRetirementV1::new(
            pending.clone(),
            receipt,
            RetirementDigestV1::parse(&"c".repeat(64)).unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(&final_entry).unwrap();
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, ["final_input_hash", "pending", "receipt", "version"]);
        let decoded: FinalRetirementV1 = serde_json::from_value(value.clone()).unwrap();
        assert!(equal(decoded.pending(), &pending).unwrap());
        for (key, extra) in [
            ("accepted", json!({})),
            ("ledger_head", json!({})),
            ("closing_nonce", json!(Uuid::new_v4())),
        ] {
            let mut bad = value.clone();
            bad[key] = extra;
            assert!(serde_json::from_value::<FinalRetirementV1>(bad).is_err());
        }
        let entry = RetirementEntryV1::Final(Box::new(final_entry));
        let wire = serde_json::to_value(entry).unwrap();
        assert_eq!(wire["kind"], json!("final"));
        assert_eq!(wire.as_object().unwrap().len(), 2);
    }

    #[test]
    fn full3_binding_roundtrips_closed_assignment_without_live_nonce() {
        let value = binding();
        let json = serde_json::to_value(&value).unwrap();
        let decoded: Full3RegistrationBindingV1 = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(decoded.arm_id(), value.arm_id());
        assert_eq!(decoded.original_epoch(), value.original_epoch());
        assert_eq!(decoded.registration_hash(), value.registration_hash());
        assert_eq!(json.as_object().unwrap().len(), 5);
        assert!(json.get("actor_nonce").is_none());
        assert!(json.get("generation").is_none());
    }

    #[test]
    fn full3_binding_rejects_extensions_versions_and_noncanonical_ids() {
        let valid = serde_json::to_value(binding()).unwrap();
        for (key, bad) in [
            ("version", json!(2)),
            ("arm_id", json!(Uuid::nil().to_string())),
            ("original_epoch", json!("00000000000000000000000000000005")),
            ("actor_nonce", json!(Uuid::from_u128(6).to_string())),
        ] {
            let mut value = valid.clone();
            value[key] = bad;
            assert!(serde_json::from_value::<Full3RegistrationBindingV1>(value).is_err());
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Full3RegistrationBindingV1 {
    version: u32,
    arm_id: Uuid,
    registration_hash: RetirementDigestV1,
    ledger: LedgerIdentityV1,
    original_epoch: Uuid,
}

impl<'de> Deserialize<'de> for Full3RegistrationBindingV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            #[serde(deserialize_with = "strict_uuid")]
            arm_id: Uuid,
            registration_hash: RetirementDigestV1,
            ledger: LedgerIdentityV1,
            #[serde(deserialize_with = "strict_uuid")]
            original_epoch: Uuid,
        }
        let wire = Wire::deserialize(decoder)
            .map_err(|_| D::Error::custom("invalid private Full3 assignment"))?;
        if wire.version != 1 {
            return Err(D::Error::custom("invalid private Full3 assignment"));
        }
        Self::new(
            wire.arm_id,
            wire.registration_hash,
            wire.ledger,
            wire.original_epoch,
        )
        .map_err(|_| D::Error::custom("invalid private Full3 assignment"))
    }
}

impl Full3RegistrationBindingV1 {
    pub(crate) fn new(
        arm_id: Uuid,
        registration_hash: RetirementDigestV1,
        ledger: LedgerIdentityV1,
        original_epoch: Uuid,
    ) -> JournalResult<Self> {
        if arm_id.is_nil() || original_epoch.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        ledger.validate()?;
        Ok(Self {
            version: 1,
            arm_id,
            registration_hash,
            ledger,
            original_epoch,
        })
    }
    pub(crate) fn arm_id(&self) -> Uuid {
        self.arm_id
    }
    pub(crate) fn registration_hash(&self) -> &RetirementDigestV1 {
        &self.registration_hash
    }
    pub(crate) fn ledger(&self) -> &LedgerIdentityV1 {
        &self.ledger
    }
    pub(crate) fn original_epoch(&self) -> Uuid {
        self.original_epoch
    }
}

use super::{
    checkpoint::SupervisionOperationV1,
    journal::{JournalScopeV1, canonical},
    outbox::{BrokerCheckpointV4, OutboxEventV1},
    retirement::{
        RetiredReceiptV1, RetiredTombstoneV1, RetirementAuditAckV1, RetirementEvidenceV1,
        RetirementIdentityV1,
    },
};
use std::collections::HashSet;

fn require(valid: bool) -> JournalResult<()> {
    if valid {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}
fn equal(a: &impl Serialize, b: &impl Serialize) -> JournalResult<bool> {
    Ok(canonical(a)? == canonical(b)?)
}
fn required_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// Durable data only; decoding cannot reconstruct the live original witness.
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingRetirementV1 {
    version: u32,
    identity: RetirementIdentityV1,
    evidence: RetirementEvidenceV1,
    intent_event_id: Uuid,
    intent_input_hash: RetirementDigestV1,
    final_event_id: Uuid,
    audit_ack: Option<RetirementAuditAckV1>,
    reserved_final_checkpoint_bytes: u64,
    reserved_final_event_bytes: u64,
}
impl<'de> Deserialize<'de> for PendingRetirementV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            identity: RetirementIdentityV1,
            evidence: RetirementEvidenceV1,
            #[serde(deserialize_with = "strict_uuid")]
            intent_event_id: Uuid,
            intent_input_hash: RetirementDigestV1,
            #[serde(deserialize_with = "strict_uuid")]
            final_event_id: Uuid,
            #[serde(deserialize_with = "required_option")]
            audit_ack: Option<RetirementAuditAckV1>,
            reserved_final_checkpoint_bytes: u64,
            reserved_final_event_bytes: u64,
        }
        let w = Wire::deserialize(decoder)?;
        let value = Self {
            version: w.version,
            identity: w.identity,
            evidence: w.evidence,
            intent_event_id: w.intent_event_id,
            intent_input_hash: w.intent_input_hash,
            final_event_id: w.final_event_id,
            audit_ack: w.audit_ack,
            reserved_final_checkpoint_bytes: w.reserved_final_checkpoint_bytes,
            reserved_final_event_bytes: w.reserved_final_event_bytes,
        };
        value
            .validate()
            .map_err(|_| D::Error::custom("invalid private retirement pending"))?;
        Ok(value)
    }
}
impl PendingRetirementV1 {
    pub(crate) fn new(
        identity: RetirementIdentityV1,
        evidence: RetirementEvidenceV1,
        intent_event_id: Uuid,
        intent_input_hash: RetirementDigestV1,
        final_event_id: Uuid,
        reserved_final_checkpoint_bytes: u64,
        reserved_final_event_bytes: u64,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            identity,
            evidence,
            intent_event_id,
            intent_input_hash,
            final_event_id,
            audit_ack: None,
            reserved_final_checkpoint_bytes,
            reserved_final_event_bytes,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> JournalResult<()> {
        require(self.version == 1)?;
        self.identity.validate()?;
        self.evidence.validate()?;
        require(
            self.evidence.identity_hash() == &self.identity.digest()?
                && !self.intent_event_id.is_nil()
                && !self.final_event_id.is_nil()
                && self.intent_event_id != self.final_event_id
                && self.intent_event_id != *self.identity.retirement_id()
                && self.final_event_id != *self.identity.retirement_id()
                && self.reserved_final_checkpoint_bytes > 0
                && self.reserved_final_checkpoint_bytes <= 32 * 1024 * 1024
                && self.reserved_final_event_bytes > 0
                && self.reserved_final_event_bytes <= 120 * 1024,
        )?;
        let mut unacked = self.clone();
        unacked.audit_ack = None;
        require(
            (canonical(&unacked)?.len() as u64)
                .checked_add(4096)
                .is_some_and(|bound| self.reserved_final_checkpoint_bytes >= bound),
        )?;
        if let Some(ack) = &self.audit_ack {
            ack.validate()?;
            require(
                *ack.event_id() == self.intent_event_id
                    && ack.input_hash() == &self.intent_input_hash,
            )?;
        }
        Ok(())
    }
    pub(crate) fn identity(&self) -> &RetirementIdentityV1 {
        &self.identity
    }
    pub(crate) fn evidence(&self) -> &RetirementEvidenceV1 {
        &self.evidence
    }
    pub(crate) fn intention_event_id(&self) -> Uuid {
        self.intent_event_id
    }
    pub(crate) fn intent_input_hash(&self) -> &RetirementDigestV1 {
        &self.intent_input_hash
    }
    pub(crate) fn final_event_id(&self) -> Uuid {
        self.final_event_id
    }
    pub(crate) fn audit_ack(&self) -> Option<&RetirementAuditAckV1> {
        self.audit_ack.as_ref()
    }
    pub(crate) fn reserved_final_checkpoint_bytes(&self) -> u64 {
        self.reserved_final_checkpoint_bytes
    }
    pub(crate) fn reserved_final_event_bytes(&self) -> u64 {
        self.reserved_final_event_bytes
    }
    pub(crate) fn same_request(&self, other: &Self) -> JournalResult<bool> {
        let mut next = self.clone();
        next.audit_ack = other.audit_ack.clone();
        equal(&next, other)
    }
    fn with_ack(&self, outbox: &BrokerCheckpointV4) -> JournalResult<Self> {
        let event = outbox
            .events()
            .iter()
            .find(|event| event.event_id() == self.intent_event_id)
            .ok_or(JournalError::Conflict)?;
        require(event.input_hash() == self.intent_input_hash.as_str())?;
        let ack = event
            .audit_seq()
            .map(|seq| {
                RetirementAuditAckV1::new(event.event_id(), self.intent_input_hash.clone(), seq)
            })
            .transpose()?;
        if let Some(previous) = &self.audit_ack {
            require(equal(
                previous,
                ack.as_ref().ok_or(JournalError::Conflict)?,
            )?)?;
        }
        let mut next = self.clone();
        next.audit_ack = ack;
        Ok(next)
    }
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalRetirementV1 {
    version: u32,
    pending: PendingRetirementV1,
    receipt: RetiredReceiptV1,
    final_input_hash: RetirementDigestV1,
}
impl<'de> Deserialize<'de> for FinalRetirementV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            pending: PendingRetirementV1,
            receipt: RetiredReceiptV1,
            final_input_hash: RetirementDigestV1,
        }
        let w = Wire::deserialize(decoder)?;
        if w.version != 1 {
            return Err(D::Error::custom("invalid private retirement final"));
        }
        Self::new(w.pending, w.receipt, w.final_input_hash)
            .map_err(|_| D::Error::custom("invalid private retirement final"))
    }
}
impl FinalRetirementV1 {
    pub(crate) fn new(
        pending: PendingRetirementV1,
        receipt: RetiredReceiptV1,
        final_input_hash: RetirementDigestV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            pending,
            receipt,
            final_input_hash,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> JournalResult<()> {
        require(self.version == 1)?;
        self.pending.validate()?;
        self.receipt.validate()?;
        let identity = self.pending.identity();
        let core = self.receipt.finalization();
        require(
            self.pending.audit_ack().is_some()
                && self.receipt.identity_hash() == &identity.digest()?
                && self.receipt.retirement_id() == identity.retirement_id()
                && core.joined().original_epoch() == identity.original_epoch()
                && core.joined().original_actor_nonce()
                    == self.pending.evidence().original_actor_nonce()
                && core.tombstone_hash()
                    == &RetiredTombstoneV1::new(
                        self.receipt.ledger().clone(),
                        identity.digest()?,
                        core.closing_hash().clone(),
                    )?
                    .digest()?,
        )?;
        // This bound is an immutable promise for the complete closed Final entry.
        require(canonical(self)?.len() as u64 <= self.pending.reserved_final_checkpoint_bytes())
    }
    pub(crate) fn receipt(&self) -> &RetiredReceiptV1 {
        &self.receipt
    }
    pub(crate) fn pending(&self) -> &PendingRetirementV1 {
        &self.pending
    }
    pub(crate) fn final_input_hash(&self) -> &RetirementDigestV1 {
        &self.final_input_hash
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "entry",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum RetirementEntryV1 {
    Pending(Box<PendingRetirementV1>),
    Final(Box<FinalRetirementV1>),
}
impl RetirementEntryV1 {
    pub(crate) fn pending(value: PendingRetirementV1) -> Self {
        Self::Pending(Box::new(value))
    }
    pub(crate) fn request(&self) -> &PendingRetirementV1 {
        match self {
            Self::Pending(v) => v,
            Self::Final(v) => v.pending(),
        }
    }
    pub(crate) fn finalized(&self) -> Option<&FinalRetirementV1> {
        match self {
            Self::Pending(_) => None,
            Self::Final(v) => Some(v),
        }
    }
    fn with_ack(&self, outbox: &BrokerCheckpointV4) -> JournalResult<Self> {
        match self {
            Self::Pending(p) => Ok(Self::Pending(Box::new(p.with_ack(outbox)?))),
            Self::Final(f) => {
                require(equal(&f.pending, &f.pending.with_ack(outbox)?)?)?;
                Ok(self.clone())
            }
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrokerCheckpointV5 {
    version: u32,
    outbox: BrokerCheckpointV4,
    full3_registrations: Vec<Full3RegistrationBindingV1>,
    retirements: Vec<RetirementEntryV1>,
}
impl<'de> Deserialize<'de> for BrokerCheckpointV5 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            outbox: BrokerCheckpointV4,
            full3_registrations: Vec<Full3RegistrationBindingV1>,
            retirements: Vec<RetirementEntryV1>,
        }
        let mut ignored = false;
        let w: Wire = serde_ignored::deserialize(decoder, |_| ignored = true)
            .map_err(|_| D::Error::custom("invalid private retirement checkpoint"))?;
        if ignored || w.version != 5 {
            return Err(D::Error::custom("invalid private retirement checkpoint"));
        }
        Self::new(w.outbox, w.full3_registrations, w.retirements)
            .map_err(|_| D::Error::custom("invalid private retirement checkpoint"))
    }
}
impl BrokerCheckpointV5 {
    pub(crate) fn new(
        outbox: BrokerCheckpointV4,
        full3_registrations: Vec<Full3RegistrationBindingV1>,
        retirements: Vec<RetirementEntryV1>,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 5,
            outbox,
            full3_registrations,
            retirements,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn outbox(&self) -> &BrokerCheckpointV4 {
        &self.outbox
    }
    pub(crate) fn bindings(&self) -> &[Full3RegistrationBindingV1] {
        &self.full3_registrations
    }
    pub(crate) fn retirements(&self) -> &[RetirementEntryV1] {
        &self.retirements
    }
    pub(crate) fn with_outbox(&self, outbox: BrokerCheckpointV4) -> JournalResult<Self> {
        let retirements = self
            .retirements
            .iter()
            .map(|entry| entry.with_ack(&outbox))
            .collect::<JournalResult<Vec<_>>>()?;
        Self::new(outbox, self.full3_registrations.clone(), retirements)
    }
    pub(crate) fn validate(&self) -> JournalResult<()> {
        require(self.version == 5)?;
        self.outbox.canonical_bytes()?;
        let operations = self.outbox.lifecycle().broker().supervision().operations();
        let mut arms = HashSet::new();
        for binding in &self.full3_registrations {
            require(arms.insert(binding.arm_id()))?;
            acknowledged_arm(&self.outbox, &binding.arm_id().to_string())?;
        }
        let mut retired = HashSet::new();
        let mut ids = HashSet::new();
        for command in self.outbox.lifecycle().commands() {
            ids.insert(command.command_id());
        }
        for operation in operations {
            if let SupervisionOperationV1::Export { request, .. } = operation {
                ids.insert(
                    Uuid::parse_str(&request.operation_id).map_err(|_| JournalError::Conflict)?,
                );
            }
        }
        for entry in &self.retirements {
            let pending = entry.request();
            pending.validate()?;
            let identity = pending.identity();
            let arm_id = Uuid::parse_str(&identity.arm().operation_id)
                .map_err(|_| JournalError::Conflict)?;
            require(
                retired.insert(arm_id)
                    && ids.insert(*identity.retirement_id())
                    && ids.insert(pending.intention_event_id())
                    && ids.insert(pending.final_event_id()),
            )?;
            let binding = self
                .full3_registrations
                .iter()
                .find(|v| v.arm_id() == arm_id)
                .ok_or(JournalError::Conflict)?;
            require(
                binding.registration_hash() == identity.registration_hash()
                    && binding.original_epoch() == *identity.original_epoch(),
            )?;
            let (_, receipt) = acknowledged_arm(&self.outbox, &identity.arm().operation_id)?;
            require(equal(receipt, identity.arm_receipt())?)?;
            ensure_unclaimed(&self.outbox, identity.arm())?;
            let signal = self
                .outbox
                .events()
                .iter()
                .find(|event| event.event_id() == *identity.retirement_id())
                .ok_or(JournalError::Conflict)?;
            require(signal.audit_seq().is_some() && is_abandon_signal(signal, identity))?;
            let intention = self
                .outbox
                .events()
                .iter()
                .find(|event| event.event_id() == pending.intention_event_id())
                .ok_or(JournalError::Conflict)?;
            require(
                (canonical(intention)?.len() as u64)
                    .checked_add(512)
                    .is_some_and(|bound| pending.reserved_final_event_bytes() >= bound),
            )?;
            require(
                intention.input_hash() == pending.intent_input_hash().as_str()
                    && intention.event().kind == "broker.retirement.pending"
                    && intention.event().data
                        == serde_json::json!({"identity_hash":identity.digest()?,"evidence_hash":pending.evidence().digest()?})
                    && equal(pending, &pending.with_ack(&self.outbox)?)?,
            )?;
            if let Some(finalized) = entry.finalized() {
                finalized.validate()?;
                require(finalized.receipt().ledger().matches(binding.ledger())?)?;
                let event = self
                    .outbox
                    .events()
                    .iter()
                    .find(|event| event.event_id() == pending.final_event_id())
                    .ok_or(JournalError::Conflict)?;
                require(
                    event.event().kind == "broker.retirement.final"
                        && event.input_hash() == finalized.final_input_hash().as_str()
                        && event.event().data
                            == serde_json::json!({"identity_hash":identity.digest()?,"receipt_hash":finalized.receipt().digest()?})
                        && canonical(event)?.len() as u64 <= pending.reserved_final_event_bytes(),
                )?;
            } else {
                require(
                    self.outbox
                        .events()
                        .iter()
                        .all(|event| event.event_id() != pending.final_event_id()),
                )?;
            }
        }
        for event in self.outbox.events() {
            if ids.contains(&event.event_id()) {
                require(self.retirements.iter().any(|entry| {
                    let pending = entry.request();
                    (*pending.identity().retirement_id() == event.event_id()
                        && is_abandon_signal(event, pending.identity()))
                        || pending.intention_event_id() == event.event_id()
                        || (entry.finalized().is_some()
                            && pending.final_event_id() == event.event_id())
                }))?;
            }
        }
        if canonical(self)?.len() > 32 * 1024 * 1024 {
            return Err(JournalError::TooLarge);
        }
        Ok(())
    }
    pub(crate) fn validate_context(
        &self,
        scope: &JournalScopeV1,
        operation: Uuid,
        intent_hash: &str,
    ) -> JournalResult<()> {
        self.validate()?;
        for binding in &self.full3_registrations {
            require(binding.ledger().installation_id() == &scope.installation_id())?;
        }
        for entry in &self.retirements {
            let identity = entry.request().identity();
            require(
                equal(identity.scope(), scope)?
                    && *identity.job_operation_id() == operation
                    && identity.intent_hash().as_str() == intent_hash,
            )?;
        }
        Ok(())
    }
    pub(crate) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        self.validate()?;
        let Some(old) = previous else {
            return require(
                self.retirements
                    .iter()
                    .all(|entry| entry.finalized().is_none()),
            );
        };
        require(
            self.full3_registrations.len() >= old.full3_registrations.len()
                && self.retirements.len() >= old.retirements.len(),
        )?;
        for (a, b) in old
            .full3_registrations
            .iter()
            .zip(&self.full3_registrations)
        {
            require(equal(a, b)?)?;
        }
        for (index, entry) in self.retirements.iter().enumerate() {
            match old.retirements.get(index) {
                Some(prior) => {
                    require(prior.request().same_request(entry.request())?)?;
                    if prior.request().audit_ack().is_some() {
                        require(equal(prior.request(), entry.request())?)?;
                    }
                    match (prior.finalized(), entry.finalized()) {
                        (Some(a), Some(b)) => require(equal(a, b)?)?,
                        (Some(_), None) => return Err(JournalError::Conflict),
                        (None, Some(_)) => {
                            audit_ack(old.outbox(), prior.request())?;
                            require(equal(prior.request(), entry.request())?)?;
                        }
                        (None, None) => {}
                    }
                }
                None => {
                    require(entry.finalized().is_none() && entry.request().audit_ack().is_none())?
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn audit_ack(
    outbox: &BrokerCheckpointV4,
    pending: &PendingRetirementV1,
) -> JournalResult<RetirementAuditAckV1> {
    let entry = outbox
        .events()
        .iter()
        .find(|event| event.event_id() == pending.intention_event_id())
        .ok_or(JournalError::Conflict)?;
    let ack = pending.audit_ack().ok_or(JournalError::Conflict)?;
    require(
        entry.input_hash() == ack.input_hash().as_str()
            && entry.audit_seq() == Some(*ack.audit_seq())
            && entry.event_id() == *ack.event_id(),
    )?;
    Ok(ack.clone())
}
pub(crate) fn acknowledged_arm<'a>(
    outbox: &'a BrokerCheckpointV4,
    arm_id: &str,
) -> JournalResult<(&'a crate::control::ArmV1, &'a crate::control::ArmReceiptV1)> {
    outbox
        .lifecycle()
        .broker()
        .supervision()
        .operations()
        .iter()
        .find_map(|operation| match operation {
            SupervisionOperationV1::Arm {
                request,
                response: Some(response),
                ..
            } if request.operation_id == arm_id => Some((request, &response.receipt)),
            _ => None,
        })
        .ok_or(JournalError::Conflict)
}
pub(crate) fn ensure_unclaimed(
    outbox: &BrokerCheckpointV4,
    arm: &crate::control::ArmV1,
) -> JournalResult<()> {
    for operation in outbox.lifecycle().broker().supervision().operations() {
        match operation {
            SupervisionOperationV1::WorkerClaim { request, .. }
                if request.attempt_id == arm.attempt_id =>
            {
                return Err(JournalError::Conflict);
            }
            SupervisionOperationV1::HelperClaim { request, .. } => {
                if let crate::control::ArmSubjectV1::Helper { helper_id, .. } = &arm.subject
                    && request.helper_id == *helper_id
                {
                    return Err(JournalError::Conflict);
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn is_abandon_signal(event: &OutboxEventV1, identity: &RetirementIdentityV1) -> bool {
    let value = event.event();
    value.kind == "broker.abandon-before-claim.requested"
        && value.source == "ai-guard-broker"
        && value.provenance == "observed"
        && value.coverage == "native"
        && value.data
            == serde_json::json!({"version":1,"operation_id":identity.job_operation_id(),"arm_id":identity.arm().operation_id,
            "expected_epoch":identity.original_epoch(),"generation":identity.generation(),"command_id":identity.retirement_id()})
}
