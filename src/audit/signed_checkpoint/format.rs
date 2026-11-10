//! Versioned, deterministic statements and their signed envelope.
//!
//! The signed message is the domain prefix followed by the exact bytes of the
//! `statement` member. A record is accepted only when those bytes, and the
//! whole envelope, are byte-for-byte the canonical serialization of their
//! typed content: unknown, missing, duplicated or reordered fields, optional
//! whitespace, uppercase hexadecimal and non-canonical base64 are all refused.

use super::{
    CHAIN_ALGORITHM, MAX_RECORD_BYTES, RECORD_FORMAT, STATEMENT_TYPE, STATEMENT_VERSION,
    report::{Code, Failure},
};
use crate::tenancy::validate_tenant_id;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

pub(crate) const ZERO_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
pub(crate) const KIND_CHECKPOINT: &str = "checkpoint";
pub(crate) const KIND_KEY_ROTATION: &str = "key_rotation";
pub(crate) const ENDORSEMENT_RETIRING_KEY: &str = "retiring_key";
pub(crate) const ENDORSEMENT_NONE: &str = "none";
const MAX_INDEX: u64 = i64::MAX as u64;
const SIGNATURE_BYTES: usize = 64;

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn is_canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, the only accepted time form.
pub(crate) fn is_canonical_time(value: &str) -> bool {
    value.len() == 24
        && chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|time| {
            time.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                == value
        })
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Decode canonical unpadded base64url of an exact length.
pub(crate) fn decode_base64url(value: &str, length: usize) -> Option<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    (bytes.len() == length && URL_SAFE_NO_PAD.encode(&bytes) == value).then_some(bytes)
}

pub(crate) fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A position of the audit chain: the number of events and the hash of the last
/// one. Zero events is the empty position with the all-zero hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub event_count: u64,
    pub head_hash: String,
}

impl Position {
    pub(crate) fn empty() -> Self {
        Self {
            event_count: 0,
            head_hash: ZERO_HASH.into(),
        }
    }
}

/// Statement signed by the current key: the audit chain of this tenant and
/// namespace had this head at this position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointStatement {
    #[serde(rename = "type")]
    pub statement_type: String,
    pub version: u32,
    pub kind: String,
    pub tenant_id: String,
    pub namespace_uuid: String,
    pub log_index: u64,
    pub previous_statement_sha256: String,
    /// Declared by the signer; not a trusted timestamp.
    pub created_at: String,
    pub signing_key_id: String,
    pub audit_schema_version: u32,
    pub chain_algorithm: String,
    pub event_count: u64,
    pub head_hash: String,
    pub control_genesis_seq: u64,
    pub control_genesis_hash: String,
    pub previous_event_count: u64,
    pub previous_head_hash: String,
}

/// Statement that hands the log over from a retiring key to its successor.
/// `previous_event_count` and `previous_head_hash` carry the last signed audit
/// position forward so that the log stays self-describing across a rotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationStatement {
    #[serde(rename = "type")]
    pub statement_type: String,
    pub version: u32,
    pub kind: String,
    pub tenant_id: String,
    pub namespace_uuid: String,
    pub log_index: u64,
    pub previous_statement_sha256: String,
    /// Declared by the signer; not a trusted timestamp.
    pub created_at: String,
    pub retiring_key_id: String,
    pub successor_key_id: String,
    pub endorsement: String,
    pub previous_event_count: u64,
    pub previous_head_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Checkpoint(CheckpointStatement),
    KeyRotation(RotationStatement),
}

fn malformed(message: impl Into<String>) -> Failure {
    Failure::new(Code::RecordMalformed, message)
}

fn require(condition: bool, message: &str) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(malformed(message))
    }
}

impl Statement {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Checkpoint(_) => KIND_CHECKPOINT,
            Self::KeyRotation(_) => KIND_KEY_ROTATION,
        }
    }

    pub fn tenant_id(&self) -> &str {
        match self {
            Self::Checkpoint(statement) => &statement.tenant_id,
            Self::KeyRotation(statement) => &statement.tenant_id,
        }
    }

    pub fn namespace_uuid(&self) -> &str {
        match self {
            Self::Checkpoint(statement) => &statement.namespace_uuid,
            Self::KeyRotation(statement) => &statement.namespace_uuid,
        }
    }

    pub fn log_index(&self) -> u64 {
        match self {
            Self::Checkpoint(statement) => statement.log_index,
            Self::KeyRotation(statement) => statement.log_index,
        }
    }

    pub fn previous_statement_sha256(&self) -> &str {
        match self {
            Self::Checkpoint(statement) => &statement.previous_statement_sha256,
            Self::KeyRotation(statement) => &statement.previous_statement_sha256,
        }
    }

    pub fn created_at(&self) -> &str {
        match self {
            Self::Checkpoint(statement) => &statement.created_at,
            Self::KeyRotation(statement) => &statement.created_at,
        }
    }

    /// The last signed audit position before this record.
    pub fn previous_position(&self) -> Position {
        let (event_count, head_hash) = match self {
            Self::Checkpoint(statement) => (
                statement.previous_event_count,
                &statement.previous_head_hash,
            ),
            Self::KeyRotation(statement) => (
                statement.previous_event_count,
                &statement.previous_head_hash,
            ),
        };
        Position {
            event_count,
            head_hash: head_hash.clone(),
        }
    }

    /// The last signed audit position once this record is accepted. A rotation
    /// signs no new position and carries the previous one forward.
    pub fn signed_position(&self) -> Position {
        match self {
            Self::Checkpoint(statement) => Position {
                event_count: statement.event_count,
                head_hash: statement.head_hash.clone(),
            },
            Self::KeyRotation(_) => self.previous_position(),
        }
    }

    /// The key that must sign the next record of the log.
    pub fn next_signer(&self) -> &str {
        match self {
            Self::Checkpoint(statement) => &statement.signing_key_id,
            Self::KeyRotation(statement) => &statement.successor_key_id,
        }
    }

    /// Key identifiers that must sign this record, in order.
    pub fn required_signers(&self) -> Vec<&str> {
        match self {
            Self::Checkpoint(statement) => vec![statement.signing_key_id.as_str()],
            Self::KeyRotation(statement) if statement.endorsement == ENDORSEMENT_RETIRING_KEY => {
                vec![
                    statement.retiring_key_id.as_str(),
                    statement.successor_key_id.as_str(),
                ]
            }
            Self::KeyRotation(statement) => vec![statement.successor_key_id.as_str()],
        }
    }

    fn validate(&self) -> Result<(), Failure> {
        let (statement_type, version, kind) = match self {
            Self::Checkpoint(statement) => (
                &statement.statement_type,
                statement.version,
                &statement.kind,
            ),
            Self::KeyRotation(statement) => (
                &statement.statement_type,
                statement.version,
                &statement.kind,
            ),
        };
        require(
            statement_type == STATEMENT_TYPE && version == STATEMENT_VERSION && kind == self.kind(),
            "statement type, version or kind is not supported",
        )?;
        require(
            validate_tenant_id(self.tenant_id()).is_ok(),
            "tenant_id is not a valid tenant identifier",
        )?;
        require(
            is_canonical_uuid(self.namespace_uuid()),
            "namespace_uuid is not a canonical lowercase UUID",
        )?;
        let index = self.log_index();
        require(
            (1..=MAX_INDEX).contains(&index),
            "log_index is out of range",
        )?;
        let previous = self.previous_statement_sha256();
        require(
            is_sha256_hex(previous) && ((index == 1) == (previous == ZERO_HASH)),
            "previous_statement_sha256 is not a lowercase SHA-256 consistent with log_index",
        )?;
        require(
            is_canonical_time(self.created_at()),
            "created_at is not a canonical UTC time with milliseconds",
        )?;
        let before = self.previous_position();
        require(
            before.event_count <= MAX_INDEX
                && is_sha256_hex(&before.head_hash)
                && ((before.event_count == 0) == (before.head_hash == ZERO_HASH)),
            "previous audit position is inconsistent",
        )?;
        match self {
            Self::Checkpoint(statement) => {
                require(
                    is_sha256_hex(&statement.signing_key_id),
                    "signing_key_id is not a lowercase SHA-256",
                )?;
                require(
                    statement.audit_schema_version >= 2
                        && statement.chain_algorithm == CHAIN_ALGORITHM,
                    "audit schema version or chain algorithm is not supported",
                )?;
                require(
                    (1..=MAX_INDEX).contains(&statement.event_count)
                        && is_sha256_hex(&statement.head_hash)
                        && statement.head_hash != ZERO_HASH,
                    "signed audit position is invalid",
                )?;
                require(
                    (1..=statement.event_count).contains(&statement.control_genesis_seq)
                        && is_sha256_hex(&statement.control_genesis_hash),
                    "control genesis reference is invalid",
                )?;
                require(
                    before.event_count <= statement.event_count
                        && (before.event_count != statement.event_count
                            || before.head_hash == statement.head_hash),
                    "signed audit position does not extend the previous one",
                )
            }
            Self::KeyRotation(statement) => {
                require(
                    index > 1,
                    "a key rotation cannot be the first record of a log",
                )?;
                require(
                    is_sha256_hex(&statement.retiring_key_id)
                        && is_sha256_hex(&statement.successor_key_id)
                        && statement.retiring_key_id != statement.successor_key_id,
                    "rotation key identifiers are invalid",
                )?;
                require(
                    statement.endorsement == ENDORSEMENT_RETIRING_KEY
                        || statement.endorsement == ENDORSEMENT_NONE,
                    "rotation endorsement is not supported",
                )?;
                require(
                    before.event_count >= 1,
                    "a key rotation must follow a signed audit position",
                )
            }
        }
    }

    /// Canonical bytes: the compact serialization of the typed fields in their
    /// declared order. These exact bytes are signed and hashed.
    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, Failure> {
        self.validate()?;
        let encoded = match self {
            Self::Checkpoint(statement) => serde_json::to_vec(statement),
            Self::KeyRotation(statement) => serde_json::to_vec(statement),
        };
        encoded.map_err(|_| malformed("statement cannot be serialized"))
    }
}

/// The control genesis that a checkpoint names, with the log index of that
/// checkpoint.
///
/// The control genesis of an audit database is one event, written once when
/// its control namespace is created, and no migration moves it: every
/// checkpoint signed over one database names the same sequence and hash. A
/// database or a later checkpoint that names another one is not what the
/// earlier checkpoints signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignedGenesis {
    /// Log index of the checkpoint that signed this genesis.
    pub log_index: u64,
    pub sequence: u64,
    pub hash: String,
}

impl SignedGenesis {
    /// The genesis named by a checkpoint record; a rotation names none.
    pub(crate) fn of(record: &SignedRecord) -> Option<Self> {
        match record.statement() {
            Statement::Checkpoint(statement) => Some(Self {
                log_index: record.log_index(),
                sequence: statement.control_genesis_seq,
                hash: statement.control_genesis_hash.clone(),
            }),
            Statement::KeyRotation(_) => None,
        }
    }

    pub(crate) fn is(&self, sequence: u64, hash: &str) -> bool {
        self.sequence == sequence && self.hash == hash
    }
}

/// One signature of an envelope. `key_id` only names the expected key; the
/// public key itself is taken from the trust anchor, never from the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordSignature {
    pub key_id: String,
    pub signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeIn<'a> {
    format: String,
    #[serde(borrow)]
    statement: &'a RawValue,
    signatures: Vec<RecordSignature>,
}

#[derive(Serialize)]
struct EnvelopeOut<'a> {
    format: &'a str,
    statement: &'a RawValue,
    signatures: &'a [RecordSignature],
}

#[derive(Deserialize)]
struct KindProbe {
    kind: String,
}

/// A strictly parsed, canonical record. Its signatures are not verified here:
/// verification needs a public key pinned by a trust anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRecord {
    statement: Statement,
    statement_bytes: Vec<u8>,
    statement_sha256: String,
    signatures: Vec<RecordSignature>,
}

impl SignedRecord {
    /// Assemble a record from a statement and its signatures, then prove that
    /// the result survives the strict parser.
    pub(crate) fn seal(
        statement: Statement,
        signatures: Vec<RecordSignature>,
    ) -> Result<Self, Failure> {
        let statement_bytes = statement.canonical_bytes()?;
        let record = Self {
            statement_sha256: sha256_hex(&statement_bytes),
            statement,
            statement_bytes,
            signatures,
        };
        let parsed = Self::parse(&record.to_bytes()?)?;
        if parsed != record {
            return Err(malformed("sealed record does not round-trip"));
        }
        Ok(record)
    }

    /// Strict parser of one envelope.
    pub fn parse(bytes: &[u8]) -> Result<Self, Failure> {
        require(
            bytes.len() as u64 <= MAX_RECORD_BYTES,
            "record exceeds its size bound",
        )?;
        require(
            bytes.is_ascii(),
            "record contains bytes outside printable ASCII JSON",
        )?;
        crate::json::decode_unique(bytes, 8)
            .map_err(|error| malformed(format!("record is not strict JSON: {error}")))?;
        let envelope: EnvelopeIn<'_> = serde_json::from_slice(bytes)
            .map_err(|error| malformed(format!("record envelope is invalid: {error}")))?;
        require(
            envelope.format == RECORD_FORMAT,
            "record format is not supported",
        )?;
        let raw = envelope.statement.get().as_bytes();
        let probe: KindProbe = serde_json::from_slice(raw)
            .map_err(|error| malformed(format!("statement kind is unreadable: {error}")))?;
        let typed = match probe.kind.as_str() {
            KIND_CHECKPOINT => {
                serde_json::from_slice::<CheckpointStatement>(raw).map(Statement::Checkpoint)
            }
            KIND_KEY_ROTATION => {
                serde_json::from_slice::<RotationStatement>(raw).map(Statement::KeyRotation)
            }
            _ => return Err(malformed("statement kind is not supported")),
        };
        let statement =
            typed.map_err(|error| malformed(format!("statement is invalid: {error}")))?;
        let canonical = statement.canonical_bytes()?;
        if canonical != raw {
            return Err(Failure::new(
                Code::RecordNotCanonical,
                "statement bytes are not the canonical serialization of their content",
            ));
        }
        require(
            (1..=2).contains(&envelope.signatures.len()),
            "record must carry one or two signatures",
        )?;
        for signature in &envelope.signatures {
            require(
                is_sha256_hex(&signature.key_id)
                    && decode_base64url(&signature.signature, SIGNATURE_BYTES).is_some(),
                "signature entry is not a key identifier with a canonical 64-byte signature",
            )?;
        }
        let record = Self {
            statement_sha256: sha256_hex(&canonical),
            statement,
            statement_bytes: canonical,
            signatures: envelope.signatures,
        };
        if record.to_bytes()? != bytes {
            return Err(Failure::new(
                Code::RecordNotCanonical,
                "record bytes are not the canonical serialization of the envelope",
            ));
        }
        Ok(record)
    }

    /// Exact file content of this record.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Failure> {
        let text = String::from_utf8(self.statement_bytes.clone())
            .map_err(|_| malformed("statement is not UTF-8"))?;
        let statement =
            RawValue::from_string(text).map_err(|_| malformed("statement is not JSON"))?;
        serde_json::to_vec(&EnvelopeOut {
            format: RECORD_FORMAT,
            statement: &statement,
            signatures: &self.signatures,
        })
        .map_err(|_| malformed("record cannot be serialized"))
    }

    pub fn statement(&self) -> &Statement {
        &self.statement
    }

    /// The exact signed bytes, without the domain prefix.
    pub fn statement_bytes(&self) -> &[u8] {
        &self.statement_bytes
    }

    pub fn statement_sha256(&self) -> &str {
        &self.statement_sha256
    }

    pub fn signatures(&self) -> &[RecordSignature] {
        &self.signatures
    }

    pub fn log_index(&self) -> u64 {
        self.statement.log_index()
    }

    /// Content-addressed file name of this record.
    pub fn file_name(&self) -> String {
        record_file_name(self.log_index(), &self.statement_sha256)
    }
}

pub(crate) fn record_file_name(log_index: u64, statement_sha256: &str) -> String {
    format!("cp-{log_index:020}-{statement_sha256}.json")
}

/// Split `cp-<20 digits>-<64 hex>.json` into its log index and statement hash.
pub(crate) fn parse_record_file_name(name: &str) -> Option<(u64, &str)> {
    let rest = name.strip_prefix("cp-")?.strip_suffix(".json")?;
    let (index, hash) = rest.split_once('-')?;
    if index.len() != 20 || !index.bytes().all(|byte| byte.is_ascii_digit()) || !is_sha256_hex(hash)
    {
        return None;
    }
    let index = index.parse::<u64>().ok()?;
    (1..=MAX_INDEX).contains(&index).then_some((index, hash))
}
