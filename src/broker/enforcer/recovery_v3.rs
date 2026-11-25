//! Explicit Stop3 transport. The ledger is not implemented by this module.
use super::*;
use serde::de::{self, MapAccess, SeqAccess, Visitor};

pub(super) const HANDSHAKE_WINDOW: Duration = Duration::from_secs(6);
const N3: &[u8] = b"ai-guard:enforcer-client-negotiate:v3\0";
const SESSION3: &[u8] = b"ai-guard:enforcer-session:v3\0";
const CHALLENGE3: &[u8] = b"ai-guard:enforcer-server-challenge:v3\0";
const AUTH3: &[u8] = b"ai-guard:enforcer-client-auth:v3\0";
const GRANT3: &[u8] = b"ai-guard:enforcer-server-grant:v3\0";
const REFUSAL3: &[u8] = b"ai-guard:enforcer-server-refusal:v3\0";
const COMMAND3: &[u8] = b"ai-guard:enforcer-client-command:v3\0";
const REPLY3: &[u8] = b"ai-guard:enforcer-server-reply:v3\0";
const WATCH3: &[u8] = b"ai-guard:enforcer-server-watch:v3\0";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct Uuid3(Uuid);
impl<'de> Deserialize<'de> for Uuid3 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let text = String::deserialize(decoder)?;
        let value = Uuid::parse_str(&text).map_err(de::Error::custom)?;
        if value.is_nil() || value.to_string() != text {
            return Err(de::Error::custom("noncanonical protocol-3 UUID"));
        }
        Ok(Self(value))
    }
}
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct Hash3(String);
impl<'de> Deserialize<'de> for Hash3 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let text = String::deserialize(decoder)?;
        if !hex_hash(&text) {
            return Err(de::Error::custom("noncanonical protocol-3 digest"));
        }
        Ok(Self(text))
    }
}
fn empty_proof() -> Hash3 {
    Hash3("0".repeat(64))
}

// The witness stores only structure. Decode keys before checking uniqueness,
// including unknown branches; never build a Value from unverified input.
#[derive(PartialEq, Eq)]
enum Shape3 {
    Null,
    Bool,
    Number,
    String,
    Array(Vec<Shape3>),
    Object(BTreeMap<String, Shape3>),
}
impl<'de> Deserialize<'de> for Shape3 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Shape;
        impl<'de> Visitor<'de> for Shape {
            type Value = Shape3;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON structure with unique decoded keys")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Shape3::Null)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Shape3::Bool)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Shape3::Number)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Shape3::Number)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Shape3::Number)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Shape3::String)
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut values: S) -> Result<Self::Value, S::Error> {
                let mut result = Vec::new();
                while let Some(value) = values.next_element::<Shape3>()? {
                    result.push(value);
                }
                Ok(Shape3::Array(result))
            }
            fn visit_map<M: MapAccess<'de>>(self, mut values: M) -> Result<Self::Value, M::Error> {
                let mut result = BTreeMap::new();
                while let Some(key) = values.next_key::<String>()? {
                    if result.contains_key(&key) {
                        return Err(de::Error::custom("duplicate embedded V3 field"));
                    }
                    let value = values.next_value::<Shape3>()?;
                    result.insert(key, value);
                }
                Ok(Shape3::Object(result))
            }
        }
        decoder.deserialize_any(Shape)
    }
}

fn deserialize_embedded3<'de, D, T>(decoder: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Serialize,
{
    // Framing bounds the original allocation. RawValue preserves number and
    // string lexemes until T's original decoder has consumed those same bytes.
    let raw = Box::<serde_json::value::RawValue>::deserialize(decoder)?;
    if raw.get().len() > MAX_FRAME as usize {
        return Err(de::Error::custom("oversized embedded V3 record"));
    }
    // serde_json's normal depth limit applies independently to both visits.
    let observed: Shape3 = serde_json::from_str(raw.get())
        .map_err(|_| <D::Error as de::Error>::custom("invalid embedded V3 structure"))?;
    let value: T = serde_json::from_str(raw.get())
        .map_err(|_| <D::Error as de::Error>::custom("invalid embedded V3 record"))?;
    let encoded = serde_json::to_vec(&value)
        .map_err(|_| <D::Error as de::Error>::custom("invalid embedded V3 serialization"))?;
    if encoded.len() > MAX_FRAME as usize {
        return Err(de::Error::custom("oversized embedded V3 serialization"));
    }
    let expected: Shape3 = serde_json::from_slice(&encoded)
        .map_err(|_| <D::Error as de::Error>::custom("invalid embedded V3 serialization"))?;
    if observed != expected {
        return Err(de::Error::custom("noncanonical embedded V3 structure"));
    }
    Ok(value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrefaceFor3 {
    protocol: u32,
    role: String,
    epoch: Uuid3,
    nonce: Uuid3,
    root_binding: String,
    offered_roles: Vec<RequestedRole>,
    ledger_state: LedgerState,
}
pub(super) async fn read_preface(
    stream: &mut UnixStream,
    timeout: Duration,
) -> EnforcerResult<ChallengeV2> {
    let value: PrefaceFor3 = read_frame(stream, HANDSHAKE_FRAME, timeout).await?;
    Ok(ChallengeV2 {
        protocol: value.protocol,
        role: value.role,
        epoch: value.epoch.0,
        nonce: value.nonce.0,
        root_binding: value.root_binding,
        offered_roles: value.offered_roles,
        ledger_state: value.ledger_state,
    })
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LedgerReason3 {
    RecoveryTransportOnly,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum LedgerCoverage3 {
    Unverified {
        reason: LedgerReason3,
    },
    Complete {
        head: crate::broker::retirement::LedgerHeadV1,
    },
}
impl PartialEq for LedgerCoverage3 {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unverified { reason: a }, Self::Unverified { reason: b }) => a == b,
            (Self::Complete { head: a }, Self::Complete { head: b }) => {
                matches!((a.canonical_bytes(), b.canonical_bytes()), (Ok(a), Ok(b)) if a == b)
            }
            _ => false,
        }
    }
}
impl Eq for LedgerCoverage3 {}
fn ledger3() -> LedgerCoverage3 {
    LedgerCoverage3::Unverified {
        reason: LedgerReason3::RecoveryTransportOnly,
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CountReason3 {
    LedgerUnverified,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum RetirementCount3 {
    Unknown { reason: CountReason3 },
}
fn unknown_retirements() -> RetirementCount3 {
    RetirementCount3::Unknown {
        reason: CountReason3::LedgerUnverified,
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransportHealth3 {
    Healthy,
    Degraded,
}

/// Authenticated transport observations, not a ledger or engine certificate.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnforcerStopStatusV3 {
    epoch: Uuid3,
    root_binding: Hash3,
    granted_role: RequestedRole,
    known_stop_slots: u32,
    max_slots: u32,
    #[serde(default)]
    max_recovery_stop_owners: u32,
    #[serde(default)]
    recovery_attempt_owners: u32,
    #[serde(default)]
    owned_active: usize,
    #[serde(default)]
    recovery_fenced: bool,
    #[serde(default)]
    recovery_incomplete: bool,
    #[serde(default)]
    recovery_attempts: u64,
    #[serde(default)]
    recovery_attempt_failures: u64,
    #[serde(default)]
    recovery_attempt_in_flight: bool,
    #[serde(default)]
    retained_native_causes: u32,
    #[serde(default)]
    omitted_settled_causes: u64,
    #[serde(default)]
    cause_omissions_exact: bool,
    #[serde(default)]
    native_io_uncollected: bool,
    progress: u64,
    transport_health: TransportHealth3,
    clock_healthy: bool,
    admission_available: bool,
    ledger_state: LedgerCoverage3,
    retirement_count: RetirementCount3,
}
impl EnforcerStopStatusV3 {
    pub fn max_recovery_stop_owners(&self) -> u32 {
        self.max_recovery_stop_owners
    }
    pub fn owned_active(&self) -> usize {
        self.owned_active
    }
    pub fn recovery_fenced(&self) -> bool {
        self.recovery_fenced
    }
    pub fn recovery_incomplete(&self) -> bool {
        self.recovery_incomplete
    }
    pub fn recovery_attempts(&self) -> u64 {
        self.recovery_attempts
    }
    pub fn recovery_attempt_failures(&self) -> u64 {
        self.recovery_attempt_failures
    }
    pub fn recovery_attempt_in_flight(&self) -> bool {
        self.recovery_attempt_in_flight
    }
    pub fn retained_native_causes(&self) -> u32 {
        self.retained_native_causes
    }
    pub fn omitted_settled_causes(&self) -> u64 {
        self.omitted_settled_causes
    }
    pub fn cause_omissions_exact(&self) -> bool {
        self.cause_omissions_exact
    }
    pub fn native_io_uncollected(&self) -> bool {
        self.native_io_uncollected
    }
    pub fn known_stop_slot_count(&self) -> u32 {
        self.known_stop_slots
    }
    pub fn max_slots(&self) -> u32 {
        self.max_slots
    }
    pub fn progress(&self) -> u64 {
        self.progress
    }
    pub fn transport_healthy(&self) -> bool {
        self.transport_health == TransportHealth3::Healthy
    }
    pub fn clock_healthy(&self) -> bool {
        self.clock_healthy
    }
    pub fn admission_available(&self) -> bool {
        self.admission_available
    }
    pub fn retirement_count(&self) -> Option<u32> {
        None
    }
    pub fn ledger_state_reason(&self) -> &'static str {
        "recovery_transport_only"
    }
    fn from_legacy(value: EnforcerStopStatusV2, binding: &str) -> EnforcerResult<Self> {
        if !hex_hash(binding) {
            return Err(EnforcerError::UnsafeFilesystem);
        }
        Ok(Self {
            epoch: Uuid3(value.epoch),
            root_binding: Hash3(binding.into()),
            granted_role: RequestedRole::StopsOnly,
            known_stop_slots: u32::try_from(value.slot_count)
                .map_err(|_| EnforcerError::Capacity)?,
            max_slots: value.max_slots,
            max_recovery_stop_owners: value.max_recovery_stop_owners,
            recovery_attempt_owners: value.recovery_attempt_owners,
            owned_active: value.owned_active,
            recovery_fenced: value.recovery_fenced,
            recovery_incomplete: value.recovery_incomplete,
            recovery_attempts: value.recovery_attempts,
            recovery_attempt_failures: value.recovery_attempt_failures,
            recovery_attempt_in_flight: value.recovery_attempt_in_flight,
            retained_native_causes: value.retained_native_causes,
            omitted_settled_causes: value.omitted_settled_causes,
            cause_omissions_exact: value.cause_omissions_exact,
            native_io_uncollected: value.native_io_uncollected,
            progress: value.progress,
            transport_health: if value.transport_healthy {
                TransportHealth3::Healthy
            } else {
                TransportHealth3::Degraded
            },
            clock_healthy: value.clock_healthy,
            admission_available: false,
            ledger_state: ledger3(),
            retirement_count: unknown_retirements(),
        })
    }
    fn validate(&self, session: &Session3) -> EnforcerResult<()> {
        if self.epoch.0 != session.epoch
            || self.root_binding != session.root_binding
            || self.granted_role != RequestedRole::StopsOnly
            || self.admission_available
            || self.ledger_state != ledger3()
            || self.retirement_count != unknown_retirements()
            || !(1..=MAX_SLOTS).contains(&self.max_slots)
            || self.max_slots != session.max_slots
            || self.max_recovery_stop_owners > MAX_SLOTS
            || self.recovery_attempt_owners > self.max_recovery_stop_owners.min(1)
            || self.known_stop_slots
                > self.max_slots
                    + self
                        .max_recovery_stop_owners
                        .saturating_sub(self.recovery_attempt_owners)
            || self.owned_active > self.known_stop_slots as usize
            || self.retained_native_causes > 4 * (self.max_slots + self.max_recovery_stop_owners)
        {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(())
    }
    pub(super) fn legacy(&self) -> EnforcerStopStatusV2 {
        EnforcerStopStatusV2 {
            epoch: self.epoch.0,
            slot_count: self.known_stop_slots as usize,
            max_slots: self.max_slots,
            max_recovery_stop_owners: self.max_recovery_stop_owners,
            recovery_attempt_owners: self.recovery_attempt_owners,
            owned_active: self.owned_active,
            recovery_fenced: self.recovery_fenced,
            recovery_incomplete: self.recovery_incomplete,
            recovery_attempts: self.recovery_attempts,
            recovery_attempt_failures: self.recovery_attempt_failures,
            recovery_attempt_in_flight: self.recovery_attempt_in_flight,
            retained_native_causes: self.retained_native_causes,
            omitted_settled_causes: self.omitted_settled_causes,
            cause_omissions_exact: self.cause_omissions_exact,
            native_io_uncollected: self.native_io_uncollected,
            progress: self.progress,
            transport_healthy: self.transport_healthy(),
            clock_healthy: self.clock_healthy,
            admission_available: false,
            ledger_state: unverified_ledger(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct Negotiate3 {
    kind: String,
    protocol: u32,
    requested_role: RequestedRole,
    client_nonce: Uuid3,
    proof: Hash3,
}
pub(super) enum FirstAuthentication {
    V2(AuthenticationV2),
    V3(Negotiate3),
}
impl<'de> Deserialize<'de> for FirstAuthentication {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct First;
        impl<'de> Visitor<'de> for First {
            type Value = FirstAuthentication;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("closed Auth2 or Negotiate3")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let (mut role, mut proof, mut kind, mut protocol, mut nonce) =
                    (None, None, None, None, None);
                let mut seen = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom("duplicate authentication field"));
                    }
                    match key.as_str() {
                        "requested_role" => role = Some(map.next_value::<RequestedRole>()?),
                        "proof" => proof = Some(map.next_value::<String>()?),
                        "kind" => kind = Some(map.next_value::<String>()?),
                        "protocol" => protocol = Some(map.next_value::<u32>()?),
                        "client_nonce" => nonce = Some(map.next_value::<Uuid3>()?),
                        _ => return Err(de::Error::custom("unknown authentication field")),
                    }
                }
                let requested_role = required::<_, M::Error>(role)?;
                let proof: String = required::<_, M::Error>(proof)?;
                match (kind, protocol, nonce) {
                    (None, None, None) if seen.len() == 2 => {
                        Ok(FirstAuthentication::V2(AuthenticationV2 {
                            requested_role,
                            proof,
                        }))
                    }
                    (Some(kind), Some(3), Some(client_nonce))
                        if kind == "negotiate_v3" && seen.len() == 5 && hex_hash(&proof) =>
                    {
                        Ok(FirstAuthentication::V3(Negotiate3 {
                            kind,
                            protocol: 3,
                            requested_role,
                            client_nonce,
                            proof: Hash3(proof),
                        }))
                    }
                    _ => Err(de::Error::custom("mixed or incomplete authentication")),
                }
            }
        }
        decoder.deserialize_map(First)
    }
}
fn required<T, E: de::Error>(value: Option<T>) -> Result<T, E> {
    value.ok_or_else(|| de::Error::custom("missing protocol-3 field"))
}

#[derive(Serialize)]
struct Challenge3 {
    kind: String,
    protocol: u32,
    role: String,
    epoch: Uuid3,
    root_binding: Hash3,
    requested_role: RequestedRole,
    client_nonce: Uuid3,
    server_nonce: Uuid3,
    offered_roles: Vec<RequestedRole>,
    ledger_state: LedgerCoverage3,
    session_id: Hash3,
    proof: Hash3,
}
impl Challenge3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.role,
            self.epoch,
            &self.root_binding,
            self.requested_role,
            self.client_nonce,
            self.server_nonce,
            &self.offered_roles,
            &self.ledger_state,
        )
    }
}
#[derive(Serialize)]
struct Auth3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    requested_role: RequestedRole,
    epoch: Uuid3,
    root_binding: Hash3,
    client_nonce: Uuid3,
    server_nonce: Uuid3,
    proof: Hash3,
}
impl Auth3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.requested_role,
            self.epoch,
            &self.root_binding,
            self.client_nonce,
            self.server_nonce,
        )
    }
}
#[derive(Serialize)]
struct Grant3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    granted_role: RequestedRole,
    epoch: Uuid3,
    root_binding: Hash3,
    client_nonce: Uuid3,
    server_nonce: Uuid3,
    ledger_state: LedgerCoverage3,
    admission_available: bool,
    proof: Hash3,
}
impl Grant3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.granted_role,
            self.epoch,
            &self.root_binding,
            self.client_nonce,
            self.server_nonce,
            &self.ledger_state,
            self.admission_available,
        )
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RefusalStage3 {
    Negotiate,
    Authenticate,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RefusalCode3 {
    FullUnavailable,
    LedgerChanged,
    TransportUnavailable,
    ProtocolUnavailable,
}
#[derive(Serialize)]
struct Refusal3 {
    kind: String,
    protocol: u32,
    stage: RefusalStage3,
    requested_role: RequestedRole,
    epoch: Uuid3,
    root_binding: Hash3,
    client_nonce: Uuid3,
    server_nonce: Uuid3,
    session_id: Option<Hash3>,
    code: RefusalCode3,
    ledger_state: LedgerCoverage3,
    proof: Hash3,
}
impl Refusal3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            self.stage,
            self.requested_role,
            self.epoch,
            &self.root_binding,
            self.client_nonce,
            self.server_nonce,
            &self.session_id,
            &self.code,
            &self.ledger_state,
        )
    }
}
enum Handshake3 {
    Challenge(Challenge3),
    Auth(Auth3),
    Grant(Grant3),
    Refusal(Refusal3),
}
impl<'de> Deserialize<'de> for Handshake3 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Default)]
        struct Parts {
            kind: Option<String>,
            protocol: Option<u32>,
            role: Option<String>,
            epoch: Option<Uuid3>,
            root_binding: Option<Hash3>,
            requested_role: Option<RequestedRole>,
            client_nonce: Option<Uuid3>,
            server_nonce: Option<Uuid3>,
            offered_roles: Option<Vec<RequestedRole>>,
            ledger_state: Option<LedgerCoverage3>,
            session_id: Option<Option<Hash3>>,
            proof: Option<Hash3>,
            granted_role: Option<RequestedRole>,
            admission_available: Option<bool>,
            stage: Option<RefusalStage3>,
            code: Option<RefusalCode3>,
        }
        struct Frame;
        impl<'de> Visitor<'de> for Frame {
            type Value = Handshake3;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a closed protocol-3 handshake record")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut p = Parts::default();
                let mut seen = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom("duplicate handshake field"));
                    }
                    macro_rules! read {
                        ($name:ident) => {{
                            p.$name = Some(map.next_value()?);
                        }};
                    }
                    match key.as_str() {
                        "kind" => read!(kind),
                        "protocol" => read!(protocol),
                        "role" => read!(role),
                        "epoch" => read!(epoch),
                        "root_binding" => read!(root_binding),
                        "requested_role" => read!(requested_role),
                        "client_nonce" => read!(client_nonce),
                        "server_nonce" => read!(server_nonce),
                        "offered_roles" => read!(offered_roles),
                        "ledger_state" => read!(ledger_state),
                        "session_id" => read!(session_id),
                        "proof" => read!(proof),
                        "granted_role" => read!(granted_role),
                        "admission_available" => read!(admission_available),
                        "stage" => read!(stage),
                        "code" => read!(code),
                        _ => return Err(de::Error::custom("unknown handshake field")),
                    }
                }
                let kind: String = required::<_, M::Error>(p.kind)?;
                let allowed: &[&str] = match kind.as_str() {
                    "challenge_v3" => &[
                        "kind",
                        "protocol",
                        "role",
                        "epoch",
                        "root_binding",
                        "requested_role",
                        "client_nonce",
                        "server_nonce",
                        "offered_roles",
                        "ledger_state",
                        "session_id",
                        "proof",
                    ],
                    "auth_v3" => &[
                        "kind",
                        "protocol",
                        "session_id",
                        "requested_role",
                        "epoch",
                        "root_binding",
                        "client_nonce",
                        "server_nonce",
                        "proof",
                    ],
                    "granted_v3" => &[
                        "kind",
                        "protocol",
                        "session_id",
                        "granted_role",
                        "epoch",
                        "root_binding",
                        "client_nonce",
                        "server_nonce",
                        "ledger_state",
                        "admission_available",
                        "proof",
                    ],
                    "refused_v3" => &[
                        "kind",
                        "protocol",
                        "stage",
                        "requested_role",
                        "epoch",
                        "root_binding",
                        "client_nonce",
                        "server_nonce",
                        "session_id",
                        "code",
                        "ledger_state",
                        "proof",
                    ],
                    _ => return Err(de::Error::custom("unknown handshake kind")),
                };
                if seen.len() != allowed.len()
                    || seen.iter().any(|key| !allowed.contains(&key.as_str()))
                {
                    return Err(de::Error::custom("mixed or incomplete handshake"));
                }
                let protocol = required::<_, M::Error>(p.protocol)?;
                if protocol != 3 {
                    return Err(de::Error::custom("invalid handshake protocol"));
                }
                let epoch = required::<_, M::Error>(p.epoch)?;
                let root_binding = required::<_, M::Error>(p.root_binding)?;
                let client_nonce = required::<_, M::Error>(p.client_nonce)?;
                let server_nonce = required::<_, M::Error>(p.server_nonce)?;
                let proof = required::<_, M::Error>(p.proof)?;
                match kind.as_str() {
                    "challenge_v3" => Ok(Handshake3::Challenge(Challenge3 {
                        kind,
                        protocol,
                        role: required::<_, M::Error>(p.role)?,
                        epoch,
                        root_binding,
                        requested_role: required::<_, M::Error>(p.requested_role)?,
                        client_nonce,
                        server_nonce,
                        offered_roles: required::<_, M::Error>(p.offered_roles)?,
                        ledger_state: required::<_, M::Error>(p.ledger_state)?,
                        session_id: required::<_, M::Error>(required::<_, M::Error>(
                            p.session_id,
                        )?)?,
                        proof,
                    })),
                    "auth_v3" => Ok(Handshake3::Auth(Auth3 {
                        kind,
                        protocol,
                        session_id: required::<_, M::Error>(required::<_, M::Error>(
                            p.session_id,
                        )?)?,
                        requested_role: required::<_, M::Error>(p.requested_role)?,
                        epoch,
                        root_binding,
                        client_nonce,
                        server_nonce,
                        proof,
                    })),
                    "granted_v3" => Ok(Handshake3::Grant(Grant3 {
                        kind,
                        protocol,
                        session_id: required::<_, M::Error>(required::<_, M::Error>(
                            p.session_id,
                        )?)?,
                        granted_role: required::<_, M::Error>(p.granted_role)?,
                        epoch,
                        root_binding,
                        client_nonce,
                        server_nonce,
                        ledger_state: required::<_, M::Error>(p.ledger_state)?,
                        admission_available: required::<_, M::Error>(p.admission_available)?,
                        proof,
                    })),
                    "refused_v3" => Ok(Handshake3::Refusal(Refusal3 {
                        kind,
                        protocol,
                        stage: required::<_, M::Error>(p.stage)?,
                        requested_role: required::<_, M::Error>(p.requested_role)?,
                        epoch,
                        root_binding,
                        client_nonce,
                        server_nonce,
                        session_id: required::<_, M::Error>(p.session_id)?,
                        code: required::<_, M::Error>(p.code)?,
                        ledger_state: required::<_, M::Error>(p.ledger_state)?,
                        proof,
                    })),
                    _ => Err(de::Error::custom("invalid handshake kind")),
                }
            }
        }
        decoder.deserialize_map(Frame)
    }
}

fn negotiate_core<'a>(challenge: &'a ChallengeV2, value: &'a Negotiate3) -> impl Serialize + 'a {
    (
        challenge.protocol,
        &challenge.role,
        challenge.epoch,
        challenge.nonce,
        &challenge.root_binding,
        &challenge.offered_roles,
        &challenge.ledger_state,
        3_u32,
        value.requested_role,
        value.client_nonce,
    )
}
fn raw_mac(key: &[u8; 32], domain: &[u8], bytes: &[u8]) -> EnforcerResult<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| EnforcerError::InvalidInput)?;
    mac.update(domain);
    mac.update(bytes);
    Ok(mac)
}
fn proof(key: &[u8; 32], domain: &[u8], value: &impl Serialize) -> EnforcerResult<Hash3> {
    let bytes = canonical(value).map_err(|_| EnforcerError::InvalidInput)?;
    Ok(Hash3(hex::encode(
        raw_mac(key, domain, &bytes)?.finalize().into_bytes(),
    )))
}
fn verify(
    key: &[u8; 32],
    domain: &[u8],
    value: &impl Serialize,
    signature: &Hash3,
) -> EnforcerResult<()> {
    let bytes = canonical(value).map_err(|_| EnforcerError::InvalidInput)?;
    verify_bytes(key, domain, &bytes, signature)
}
fn verify_bytes(
    key: &[u8; 32],
    domain: &[u8],
    bytes: &[u8],
    signature: &Hash3,
) -> EnforcerResult<()> {
    let decoded = hex::decode(&signature.0).map_err(|_| EnforcerError::Unauthorized)?;
    raw_mac(key, domain, bytes)?
        .verify_slice(&decoded)
        .map_err(|_| EnforcerError::Unauthorized)
}
fn negotiate_proof(
    key: &[u8; 32],
    challenge: &ChallengeV2,
    value: &Negotiate3,
) -> EnforcerResult<Hash3> {
    let bytes = serde_json::to_vec(&negotiate_core(challenge, value))
        .map_err(|_| EnforcerError::InvalidInput)?;
    Ok(Hash3(hex::encode(
        raw_mac(key, N3, &bytes)?.finalize().into_bytes(),
    )))
}
fn session_id(
    challenge: &ChallengeV2,
    negotiate: &Negotiate3,
    value: &Challenge3,
) -> EnforcerResult<Hash3> {
    let bytes = canonical(&(negotiate_core(challenge, negotiate), value.core()))
        .map_err(|_| EnforcerError::InvalidInput)?;
    let mut digest = Sha256::new();
    digest.update(SESSION3);
    digest.update(bytes);
    Ok(Hash3(hex::encode(digest.finalize())))
}
fn fresh_nonce(excluded: &[Uuid]) -> Uuid3 {
    loop {
        let nonce = Uuid::new_v4();
        if !nonce.is_nil() && !excluded.contains(&nonce) {
            return Uuid3(nonce);
        }
    }
}
pub(super) fn time_left(deadline: Instant) -> EnforcerResult<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .map(|remaining| remaining.min(IO_TIMEOUT))
        .ok_or(EnforcerError::Unavailable)
}
async fn handshake_read<T: serde::de::DeserializeOwned>(
    stream: &mut UnixStream,
    deadline: Instant,
) -> EnforcerResult<T> {
    let value = read_frame(stream, HANDSHAKE_FRAME, time_left(deadline)?).await?;
    time_left(deadline)?;
    Ok(value)
}
async fn handshake_write(
    stream: &mut UnixStream,
    value: &impl Serialize,
    deadline: Instant,
) -> EnforcerResult<()> {
    write_frame(stream, value, HANDSHAKE_FRAME, time_left(deadline)?).await?;
    time_left(deadline)?;
    Ok(())
}
async fn check_state(state: &Arc<RecoveryState>, deadline: Instant) -> EnforcerResult<()> {
    tokio::time::timeout(time_left(deadline)?, state.check_transport_v3())
        .await
        .map_err(|_| EnforcerError::Unavailable)??;
    time_left(deadline)?;
    Ok(())
}
pub(super) fn challenge_for(state: &RecoveryState) -> ChallengeV2 {
    ChallengeV2 {
        protocol: 2,
        role: "enforcer".into(),
        epoch: state.epoch,
        nonce: Uuid::new_v4(),
        root_binding: state.root_binding.clone(),
        offered_roles: vec![RequestedRole::StopsOnly],
        ledger_state: unverified_ledger(),
    }
}

#[derive(Clone)]
pub(in crate::broker::enforcer) struct Session3 {
    key: [u8; 32],
    id: Hash3,
    epoch: Uuid,
    root_binding: Hash3,
    max_slots: u32,
}

use super::super::retirement::{FullCommand, FullReply};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FullRequest3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    epoch: Uuid3,
    request_id: Uuid3,
    #[serde(deserialize_with = "deserialize_embedded3")]
    command: FullCommand,
    proof: Hash3,
}
impl FullRequest3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.epoch,
            self.request_id,
            &self.command,
        )
    }
    fn new(session: &Session3, command: FullCommand) -> EnforcerResult<Self> {
        let mut request = Self {
            kind: "full_request_v3".into(),
            protocol: 3,
            session_id: session.id.clone(),
            epoch: Uuid3(session.epoch),
            request_id: Uuid3(Uuid::new_v4()),
            command,
            proof: empty_proof(),
        };
        let computed_proof = proof(
            &session.key,
            COMMAND3,
            &(&session.id, RequestedRole::Full, request.core()),
        )?;
        request.proof = computed_proof;
        Ok(request)
    }
    fn validate(&self, session: &Session3) -> EnforcerResult<()> {
        if self.kind != "full_request_v3"
            || self.protocol != 3
            || self.session_id != session.id
            || self.epoch.0 != session.epoch
        {
            return Err(EnforcerError::Unauthorized);
        }
        verify(
            &session.key,
            COMMAND3,
            &(&session.id, RequestedRole::Full, self.core()),
            &self.proof,
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FullReply3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    epoch: Uuid3,
    request_id: Uuid3,
    sequence: u64,
    #[serde(deserialize_with = "deserialize_embedded3")]
    result: FullReply,
    proof: Hash3,
}
impl FullReply3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.epoch,
            self.request_id,
            self.sequence,
            &self.result,
        )
    }
    fn validate(
        &self,
        session: &Session3,
        request: &FullRequest3,
        sequence: u64,
    ) -> EnforcerResult<()> {
        if self.kind != "full_reply_v3"
            || self.protocol != 3
            || self.session_id != session.id
            || self.epoch.0 != session.epoch
            || self.request_id != request.request_id
            || self.sequence != sequence
        {
            return Err(EnforcerError::Unauthorized);
        }
        verify(
            &session.key,
            REPLY3,
            &(
                &session.id,
                RequestedRole::Full,
                request.core(),
                self.core(),
            ),
            &self.proof,
        )
    }
}

async fn serve_full_command(
    mut stream: UnixStream,
    state: Arc<RecoveryState>,
    session: Session3,
) -> EnforcerResult<()> {
    use tokio::io::AsyncReadExt;
    let request: FullRequest3 = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT).await?;
    request.validate(&session)?;
    state.check_transport_v3().await?;
    let full = state.full_owner()?.ok_or(EnforcerError::Unauthorized)?;
    let watching = matches!(&request.command, FullCommand::Watch {});
    let shutting_down = matches!(&request.command, FullCommand::ShutdownIfIdle {});
    let (mut reader, mut writer) = stream.into_split();
    let mut sequence = 0_u64;
    loop {
        let command = if watching {
            FullCommand::Status {}
        } else {
            request.command.clone()
        };
        let wait = if matches!(
            &command,
            FullCommand::Register { .. }
                | FullCommand::ReserveProspective { .. }
                | FullCommand::BindProspective { .. }
        ) {
            REGISTER_TIMEOUT
        } else {
            IO_TIMEOUT
        };
        let dispatch = async {
            if shutting_down && state.max_recovery_stop_owners != 0 {
                state.finish_idle_with_recovery(&full).await?;
                Ok(FullReply::Acknowledged {})
            } else {
                full.dispatch(command, state.progress.load(Ordering::SeqCst))
                    .await
            }
        };
        let mut extra = [0_u8; 1];
        // Retirement and joint Full/W closure own their work and outlive
        // the IPC waiter, including EOF and this deadline.
        let result = tokio::select! {
            received = reader.read(&mut extra) => {
                let _ = received;
                return Err(EnforcerError::Unavailable);
            }
            dispatched = tokio::time::timeout(wait, dispatch) => {
                match dispatched { Ok(Ok(result)) => result, Ok(Err(code)) => FullReply::Error {code},
                    Err(_) => FullReply::Error {code:EnforcerError::Unavailable} }
            }
        };
        let mut reply = FullReply3 {
            kind: "full_reply_v3".into(),
            protocol: 3,
            session_id: session.id.clone(),
            epoch: Uuid3(session.epoch),
            request_id: request.request_id,
            sequence,
            result,
            proof: empty_proof(),
        };
        let computed_proof = proof(
            &session.key,
            REPLY3,
            &(
                &session.id,
                RequestedRole::Full,
                request.core(),
                reply.core(),
            ),
        )?;
        reply.proof = computed_proof;
        let mut extra = [0_u8; 1];
        tokio::select! {
            received = reader.read(&mut extra), if watching => {
                let _ = received;
                return Err(EnforcerError::Unauthorized);
            }
            sent = write_frame(&mut writer, &reply, MAX_FRAME, IO_TIMEOUT) => { sent?; }
        }
        if shutting_down && matches!(reply.result, FullReply::Acknowledged {}) {
            state.shutdown_requested.store(true, Ordering::SeqCst);
        }
        if !watching {
            return Ok(());
        }
        sequence = sequence.checked_add(1).ok_or(EnforcerError::Capacity)?;
        tokio::time::sleep(POLL).await;
        state.check_transport_v3().await?;
    }
}

pub(in crate::broker::enforcer) async fn connect_full(
    stream: &mut UnixStream,
    key: &[u8; 32],
    epoch: Uuid,
    binding: &str,
    max_slots: u32,
) -> EnforcerResult<(Session3, crate::broker::retirement::LedgerIdentityV1)> {
    let deadline = Instant::now() + HANDSHAKE_WINDOW;
    let challenge = read_preface(stream, time_left(deadline)?).await?;
    if challenge.protocol != 2
        || challenge.role != "enforcer"
        || challenge.epoch != epoch
        || challenge.nonce.is_nil()
        || challenge.root_binding != binding
    {
        return Err(EnforcerError::Unauthorized);
    }
    let (session, ledger) = client_handshake_checked(
        stream,
        &challenge,
        key,
        deadline,
        RequestedRole::Full,
        max_slots,
    )
    .await?;
    Ok((session, ledger.ok_or(EnforcerError::Unauthorized)?))
}

// Closed Full recovery request target; production wiring pending.
#[allow(dead_code)]
pub(in crate::broker::enforcer) async fn request_full(
    stream: &mut UnixStream,
    session: &Session3,
    command: FullCommand,
    timeout: Duration,
) -> EnforcerResult<FullReply> {
    request_full_before_write(stream, session, command, timeout, || Ok(())).await
}

pub(in crate::broker::enforcer) async fn request_full_before_write<F>(
    stream: &mut UnixStream,
    session: &Session3,
    command: FullCommand,
    timeout: Duration,
    before_write: F,
) -> EnforcerResult<FullReply>
where
    F: FnOnce() -> EnforcerResult<()> + Send,
{
    if matches!(&command, FullCommand::Watch {}) {
        return Err(EnforcerError::InvalidInput);
    }
    let request = FullRequest3::new(session, command)?;
    // Construction/proof validation precede this synchronous transition. Once
    // it succeeds, even frame encoding/write0/cancellation is SubmittedUnknown.
    before_write()?;
    write_frame(stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
    let response: FullReply3 = read_frame(stream, MAX_FRAME, timeout).await?;
    response.validate(session, &request, 0)?;
    Ok(response.result)
}

#[cfg(test)]
pub(in crate::broker::enforcer) async fn send_full_request_only_for_test(
    stream: &mut UnixStream,
    session: &Session3,
    command: FullCommand,
) -> EnforcerResult<()> {
    let request = FullRequest3::new(session, command)?;
    write_frame(stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
    Ok(())
}

#[cfg(test)]
pub(in crate::broker::enforcer) async fn deliver_synthetic_stop_only_for_test(
    root: &Directory,
    registration: RegistrationV1,
) -> EnforcerResult<StopSlotV1> {
    // Synthetic closed audit/prefix data exercises authenticated guardian
    // dispatch, not a central audit producer or an engine effect.
    let head = RevisionRefV1::new(1, &"b".repeat(64)).map_err(|_| EnforcerError::InvalidInput)?;
    let cancel = CancelReferenceV1 {
        command_id: Uuid::new_v4(),
        input_hash: "c".repeat(64),
        confirmed_head: head.clone(),
    };
    let value = RestoreRequiredStopV1 {
        version: 1,
        request_id: Uuid::new_v4(),
        registration_hash: registration.digest()?,
        source: StopSourceV1 {
            scope_hash: "d".repeat(64),
            job_id: Uuid::new_v4(),
            intent_hash: registration.arm.intent_hash.clone(),
            confirmed_head: head,
            arm_operation_index: 0,
        },
        registration,
        cancel_generation: 1,
        durable_cancel: Some(cancel.clone()),
        cancel_prefix: vec![cancel],
    };
    let mut output = None;
    for command in [
        StopRequest::RestoreRequiredStop {
            value: Box::new(value.clone()),
        },
        StopRequest::DeliverDurableStop {
            value: Box::new(value),
        },
    ] {
        let endpoint = load_endpoint(root)?.ok_or(EnforcerError::Unavailable)?;
        let endpoints = root
            .persistent_child("endpoints", false)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?
            .ok_or(EnforcerError::UnsafeFilesystem)?;
        let mut stream = endpoint.connect_checked(&endpoints).await?;
        let deadline = Instant::now() + HANDSHAKE_WINDOW;
        let challenge = read_preface(&mut stream, time_left(deadline)?).await?;
        if challenge.epoch != endpoint.epoch()
            || challenge.root_binding != recovery_root_binding(root, &endpoint)?
        {
            return Err(EnforcerError::Unauthorized);
        }
        let session = client_handshake(
            &mut stream,
            &challenge,
            &capability(root, false)?,
            deadline,
            RequestedRole::StopsOnly,
            8,
        )
        .await?;
        let mut connection = StopConnection {
            stream,
            epoch: challenge.epoch,
            v3: Some(session),
        };
        match request(&mut connection, command).await? {
            ReplyValue3::StopReserved { value, .. } | ReplyValue3::StopAccepted { value } => {
                output = Some(value)
            }
            ReplyValue3::Error { code } => return Err(code),
            _ => return Err(EnforcerError::Unauthorized),
        }
    }
    output.ok_or(EnforcerError::Unavailable)
}

pub(in crate::broker::enforcer) struct FullWatchReader {
    session: Session3,
    request: FullRequest3,
    sequence: u64,
}
impl FullWatchReader {
    pub(in crate::broker::enforcer) async fn start(
        stream: &mut UnixStream,
        session: Session3,
    ) -> EnforcerResult<Self> {
        let request = FullRequest3::new(&session, FullCommand::Watch {})?;
        write_frame(stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
        Ok(Self {
            session,
            request,
            sequence: 0,
        })
    }
    pub(in crate::broker::enforcer) async fn read(
        &mut self,
        stream: &mut UnixStream,
    ) -> EnforcerResult<FullReply> {
        let reply: FullReply3 = read_frame(stream, MAX_FRAME, IO_TIMEOUT).await?;
        reply.validate(&self.session, &self.request, self.sequence)?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(EnforcerError::Capacity)?;
        Ok(reply.result)
    }
}

#[cfg(test)]
pub(super) async fn serve(
    stream: UnixStream,
    state: Arc<RecoveryState>,
    challenge: ChallengeV2,
    negotiate: Negotiate3,
    deadline: Instant,
) -> EnforcerResult<()> {
    serve_admitted(stream, state, challenge, negotiate, deadline, None).await
}

pub(super) async fn serve_admitted(
    mut stream: UnixStream,
    state: Arc<RecoveryState>,
    challenge: ChallengeV2,
    negotiate: Negotiate3,
    deadline: Instant,
    initial_permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> EnforcerResult<()> {
    time_left(deadline)?;
    if negotiate.client_nonce.0 == challenge.nonce {
        return Err(EnforcerError::Unauthorized);
    }
    let bytes = serde_json::to_vec(&negotiate_core(&challenge, &negotiate))
        .map_err(|_| EnforcerError::InvalidInput)?;
    verify_bytes(&state.capability, N3, &bytes, &negotiate.proof)?;
    let role_budget = match negotiate.requested_role {
        RequestedRole::Full => &state.full_connections,
        RequestedRole::StopsOnly => &state.stop_connections,
    };
    let _role_permit = Arc::clone(role_budget)
        .try_acquire_owned()
        .map_err(|_| EnforcerError::Unavailable)?;
    drop(initial_permit);
    check_state(&state, deadline).await?;
    let server_nonce = fresh_nonce(&[challenge.nonce, negotiate.client_nonce.0]);
    let full_head = if negotiate.requested_role == RequestedRole::Full {
        match state.full_owner()? {
            Some(full) => full.complete().await.ok(),
            None => None,
        }
    } else {
        None
    };
    if negotiate.requested_role == RequestedRole::Full && full_head.is_none() {
        let mut refused = Refusal3 {
            kind: "refused_v3".into(),
            protocol: 3,
            stage: RefusalStage3::Negotiate,
            requested_role: negotiate.requested_role,
            epoch: Uuid3(challenge.epoch),
            root_binding: Hash3(challenge.root_binding.clone()),
            client_nonce: negotiate.client_nonce,
            server_nonce,
            session_id: None,
            code: RefusalCode3::FullUnavailable,
            ledger_state: ledger3(),
            proof: empty_proof(),
        };
        let computed_proof = proof(
            &state.capability,
            REFUSAL3,
            &(
                negotiate_core(&challenge, &negotiate),
                None::<()>,
                None::<()>,
                refused.core(),
            ),
        )?;
        refused.proof = computed_proof;
        return handshake_write(&mut stream, &refused, deadline).await;
    }
    let mut offered = Challenge3 {
        kind: "challenge_v3".into(),
        protocol: 3,
        role: "enforcer".into(),
        epoch: Uuid3(challenge.epoch),
        root_binding: Hash3(challenge.root_binding.clone()),
        requested_role: negotiate.requested_role,
        client_nonce: negotiate.client_nonce,
        server_nonce,
        offered_roles: vec![negotiate.requested_role],
        ledger_state: match full_head {
            Some(head) => LedgerCoverage3::Complete { head },
            None => ledger3(),
        },
        session_id: empty_proof(),
        proof: empty_proof(),
    };
    offered.session_id = session_id(&challenge, &negotiate, &offered)?;
    let computed_proof = proof(
        &state.capability,
        CHALLENGE3,
        &(
            negotiate_core(&challenge, &negotiate),
            offered.core(),
            &offered.session_id,
        ),
    )?;
    offered.proof = computed_proof;
    handshake_write(&mut stream, &offered, deadline).await?;
    let Handshake3::Auth(authentication) = handshake_read(&mut stream, deadline).await? else {
        return Err(EnforcerError::Unauthorized);
    };
    if authentication.session_id != offered.session_id
        || authentication.requested_role != negotiate.requested_role
        || authentication.epoch != offered.epoch
        || authentication.root_binding != offered.root_binding
        || authentication.client_nonce != offered.client_nonce
        || authentication.server_nonce != offered.server_nonce
    {
        return Err(EnforcerError::Unauthorized);
    }
    verify(
        &state.capability,
        AUTH3,
        &(
            negotiate_core(&challenge, &negotiate),
            offered.core(),
            &offered.session_id,
            authentication.core(),
        ),
        &authentication.proof,
    )?;
    check_state(&state, deadline).await?;
    if negotiate.requested_role == RequestedRole::Full {
        let full = state.full_owner()?.ok_or(EnforcerError::Unavailable)?;
        let current = LedgerCoverage3::Complete {
            head: full.complete().await?,
        };
        if current != offered.ledger_state {
            return Err(EnforcerError::Conflict);
        }
    }
    let mut granted = Grant3 {
        kind: "granted_v3".into(),
        protocol: 3,
        session_id: offered.session_id.clone(),
        granted_role: negotiate.requested_role,
        epoch: offered.epoch,
        root_binding: offered.root_binding.clone(),
        client_nonce: offered.client_nonce,
        server_nonce: offered.server_nonce,
        ledger_state: offered.ledger_state.clone(),
        admission_available: negotiate.requested_role == RequestedRole::Full,
        proof: empty_proof(),
    };
    let computed_proof = proof(
        &state.capability,
        GRANT3,
        &(
            negotiate_core(&challenge, &negotiate),
            offered.core(),
            &offered.session_id,
            authentication.core(),
            granted.core(),
        ),
    )?;
    granted.proof = computed_proof;
    handshake_write(&mut stream, &granted, deadline).await?;
    let session = Session3 {
        key: state.capability,
        id: offered.session_id,
        epoch: state.epoch,
        root_binding: Hash3(state.root_binding.clone()),
        max_slots: state.max_slots,
    };
    if negotiate.requested_role == RequestedRole::Full {
        serve_full_command(stream, state, session).await
    } else {
        serve_command(stream, state, session).await
    }
}

pub(super) async fn client_handshake(
    stream: &mut UnixStream,
    challenge: &ChallengeV2,
    key: &[u8; 32],
    deadline: Instant,
    requested_role: RequestedRole,
    max_slots: u32,
) -> EnforcerResult<Session3> {
    client_handshake_checked(stream, challenge, key, deadline, requested_role, max_slots)
        .await
        .map(|result| result.0)
}

async fn client_handshake_checked(
    stream: &mut UnixStream,
    challenge: &ChallengeV2,
    key: &[u8; 32],
    deadline: Instant,
    requested_role: RequestedRole,
    max_slots: u32,
) -> EnforcerResult<(
    Session3,
    Option<crate::broker::retirement::LedgerIdentityV1>,
)> {
    time_left(deadline)?;
    let mut negotiate = Negotiate3 {
        kind: "negotiate_v3".into(),
        protocol: 3,
        requested_role,
        client_nonce: fresh_nonce(&[challenge.nonce]),
        proof: empty_proof(),
    };
    negotiate.proof = negotiate_proof(key, challenge, &negotiate)?;
    handshake_write(stream, &negotiate, deadline).await?;
    let offered = match handshake_read::<Handshake3>(stream, deadline).await? {
        Handshake3::Challenge(value) => value,
        Handshake3::Refusal(value) => {
            validate_refusal(challenge, &negotiate, None, None, &value, key)?;
            return Err(EnforcerError::Unavailable);
        }
        _ => return Err(EnforcerError::Unauthorized),
    };
    if offered.role != "enforcer"
        || offered.epoch.0 != challenge.epoch
        || offered.root_binding.0 != challenge.root_binding
        || offered.requested_role != requested_role
        || offered.client_nonce != negotiate.client_nonce
        || offered.server_nonce.0 == challenge.nonce
        || offered.server_nonce == negotiate.client_nonce
        || offered.offered_roles != [requested_role]
        || match requested_role {
            RequestedRole::StopsOnly => offered.ledger_state != ledger3(),
            RequestedRole::Full => {
                !matches!(offered.ledger_state, LedgerCoverage3::Complete { .. })
            }
        }
        || offered.session_id != session_id(challenge, &negotiate, &offered)?
    {
        return Err(EnforcerError::Unauthorized);
    }
    verify(
        key,
        CHALLENGE3,
        &(
            negotiate_core(challenge, &negotiate),
            offered.core(),
            &offered.session_id,
        ),
        &offered.proof,
    )?;
    let mut authentication = Auth3 {
        kind: "auth_v3".into(),
        protocol: 3,
        session_id: offered.session_id.clone(),
        requested_role,
        epoch: offered.epoch,
        root_binding: offered.root_binding.clone(),
        client_nonce: offered.client_nonce,
        server_nonce: offered.server_nonce,
        proof: empty_proof(),
    };
    let computed_proof = proof(
        key,
        AUTH3,
        &(
            negotiate_core(challenge, &negotiate),
            offered.core(),
            &offered.session_id,
            authentication.core(),
        ),
    )?;
    authentication.proof = computed_proof;
    handshake_write(stream, &authentication, deadline).await?;
    let granted = match handshake_read::<Handshake3>(stream, deadline).await? {
        Handshake3::Grant(value) => value,
        Handshake3::Refusal(value) => {
            validate_refusal(
                challenge,
                &negotiate,
                Some(&offered),
                Some(&authentication),
                &value,
                key,
            )?;
            return Err(EnforcerError::Unavailable);
        }
        _ => return Err(EnforcerError::Unauthorized),
    };
    if granted.session_id != offered.session_id
        || granted.granted_role != requested_role
        || granted.epoch != offered.epoch
        || granted.root_binding != offered.root_binding
        || granted.client_nonce != offered.client_nonce
        || granted.server_nonce != offered.server_nonce
        || granted.ledger_state != offered.ledger_state
        || granted.admission_available != (requested_role == RequestedRole::Full)
    {
        return Err(EnforcerError::Unauthorized);
    }
    verify(
        key,
        GRANT3,
        &(
            negotiate_core(challenge, &negotiate),
            offered.core(),
            &offered.session_id,
            authentication.core(),
            granted.core(),
        ),
        &granted.proof,
    )?;
    time_left(deadline)?;
    let ledger = match granted.ledger_state {
        LedgerCoverage3::Complete { head } => Some(head.ledger().clone()),
        _ => None,
    };
    Ok((
        Session3 {
            key: *key,
            id: granted.session_id,
            epoch: granted.epoch.0,
            root_binding: granted.root_binding,
            max_slots,
        },
        ledger,
    ))
}
fn validate_refusal(
    challenge: &ChallengeV2,
    negotiate: &Negotiate3,
    offered: Option<&Challenge3>,
    authentication: Option<&Auth3>,
    value: &Refusal3,
    key: &[u8; 32],
) -> EnforcerResult<()> {
    let stage_valid = matches!(
        (value.stage, offered, authentication),
        (RefusalStage3::Negotiate, None, None) | (RefusalStage3::Authenticate, Some(_), Some(_))
    );
    if !stage_valid
        || value.epoch.0 != challenge.epoch
        || value.root_binding.0 != challenge.root_binding
        || value.requested_role != negotiate.requested_role
        || value.client_nonce != negotiate.client_nonce
        || value.server_nonce.0 == challenge.nonce
        || value.server_nonce == negotiate.client_nonce
        || value.session_id.as_ref() != offered.map(|offered| &offered.session_id)
        || offered.is_some_and(|offered| offered.server_nonce != value.server_nonce)
        || value.ledger_state != ledger3()
    {
        return Err(EnforcerError::Unauthorized);
    }
    verify(
        key,
        REFUSAL3,
        &(
            negotiate_core(challenge, negotiate),
            offered.map(Challenge3::core),
            authentication.map(Auth3::core),
            value.core(),
        ),
        &value.proof,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    epoch: Uuid3,
    request_id: Uuid3,
    #[serde(deserialize_with = "deserialize_embedded3")]
    command: StopRequest,
    proof: Hash3,
}
impl Request3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.epoch,
            self.request_id,
            &self.command,
        )
    }
    fn new(session: &Session3, command: StopRequest) -> EnforcerResult<Self> {
        let mut value = Self {
            kind: "request_v3".into(),
            protocol: 3,
            session_id: session.id.clone(),
            epoch: Uuid3(session.epoch),
            request_id: Uuid3(Uuid::new_v4()),
            command,
            proof: empty_proof(),
        };
        let computed_proof = proof(&session.key, COMMAND3, &(&session.id, value.core()))?;
        value.proof = computed_proof;
        Ok(value)
    }
    fn validate(&self, session: &Session3) -> EnforcerResult<()> {
        if self.kind != "request_v3"
            || self.protocol != 3
            || self.session_id != session.id
            || self.epoch.0 != session.epoch
        {
            return Err(EnforcerError::Unauthorized);
        }
        verify(
            &session.key,
            COMMAND3,
            &(&session.id, self.core()),
            &self.proof,
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ReplyValue3 {
    RecoveryFenced {
        generation: u64,
        max_recovery_stop_owners: u32,
    },
    StatusStops {
        value: EnforcerStopStatusV3,
    },
    StopReserved {
        value: StopSlotV1,
        replayed: bool,
    },
    StopStatus {
        value: StopSlotV1,
    },
    StopAccepted {
        value: StopSlotV1,
    },
    Error {
        code: EnforcerError,
    },
}
impl ReplyValue3 {
    fn from_legacy(value: StopResponse, binding: &str) -> EnforcerResult<Self> {
        Ok(match value {
            StopResponse::RecoveryFenced {
                generation,
                max_recovery_stop_owners,
            } => Self::RecoveryFenced {
                generation,
                max_recovery_stop_owners,
            },
            StopResponse::StatusStops { value } => Self::StatusStops {
                value: EnforcerStopStatusV3::from_legacy(value, binding)?,
            },
            StopResponse::StopReserved { value, replayed } => {
                Self::StopReserved { value, replayed }
            }
            StopResponse::StopStatus { value } => Self::StopStatus { value },
            StopResponse::StopAccepted { value } => Self::StopAccepted { value },
            StopResponse::Error { code } => Self::Error { code },
        })
    }
    pub(super) fn into_legacy(self) -> StopResponse {
        match self {
            Self::RecoveryFenced {
                generation,
                max_recovery_stop_owners,
            } => StopResponse::RecoveryFenced {
                generation,
                max_recovery_stop_owners,
            },
            Self::StatusStops { value } => StopResponse::StatusStops {
                value: value.legacy(),
            },
            Self::StopReserved { value, replayed } => {
                StopResponse::StopReserved { value, replayed }
            }
            Self::StopStatus { value } => StopResponse::StopStatus { value },
            Self::StopAccepted { value } => StopResponse::StopAccepted { value },
            Self::Error { code } => StopResponse::Error { code },
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    epoch: Uuid3,
    request_id: Uuid3,
    sequence: u64,
    #[serde(deserialize_with = "deserialize_embedded3")]
    result: ReplyValue3,
    proof: Hash3,
}
impl Reply3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.epoch,
            self.request_id,
            self.sequence,
            &self.result,
        )
    }
    fn validate(&self, session: &Session3, request: &Request3) -> EnforcerResult<()> {
        if self.kind != "reply_v3"
            || self.protocol != 3
            || self.session_id != session.id
            || self.epoch.0 != session.epoch
            || self.request_id != request.request_id
            || self.sequence != 0
        {
            return Err(EnforcerError::Unauthorized);
        }
        verify(
            &session.key,
            REPLY3,
            &(&session.id, request.core(), self.core()),
            &self.proof,
        )?;
        if let ReplyValue3::StatusStops { value } = &self.result {
            value.validate(session)?;
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Watch3 {
    kind: String,
    protocol: u32,
    session_id: Hash3,
    epoch: Uuid3,
    request_id: Uuid3,
    sequence: u64,
    value: EnforcerStopStatusV3,
    proof: Hash3,
}
impl Watch3 {
    fn core(&self) -> impl Serialize + '_ {
        (
            &self.kind,
            self.protocol,
            &self.session_id,
            self.epoch,
            self.request_id,
            self.sequence,
            &self.value,
        )
    }
}
async fn serve_command(
    mut stream: UnixStream,
    state: Arc<RecoveryState>,
    session: Session3,
) -> EnforcerResult<()> {
    let request: Request3 = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT).await?;
    request.validate(&session)?;
    state.check_transport_v3().await?;
    #[cfg(test)]
    if state.faults.close_before(&request.command) {
        return Err(EnforcerError::Unavailable);
    }
    if matches!(&request.command, StopRequest::WatchStops {}) {
        use tokio::io::AsyncReadExt;
        let (mut reader, mut writer) = stream.into_split();
        let mut extra = [0_u8; 1];
        let mut sequence = 0_u64;
        loop {
            state.check_transport_v3().await?;
            let mut frame = Watch3 {
                kind: "watch_v3".into(),
                protocol: 3,
                session_id: session.id.clone(),
                epoch: Uuid3(session.epoch),
                request_id: request.request_id,
                sequence,
                value: EnforcerStopStatusV3::from_legacy(state.status()?, &state.root_binding)?,
                proof: empty_proof(),
            };
            let computed_proof = proof(
                &session.key,
                WATCH3,
                &(&session.id, request.core(), frame.core()),
            )?;
            frame.proof = computed_proof;
            tokio::select! {
                incoming = reader.read(&mut extra) => {
                    let _ = incoming;
                    // Watch is half duplex. Any further client bytes, EOF or
                    // read error closes it; there is never a role renegotiation.
                    return Err(EnforcerError::Unauthorized);
                },
                sent = async {
                    write_frame(&mut writer, &frame, MAX_FRAME, IO_TIMEOUT).await?;
                    tokio::time::sleep(POLL).await;
                    Ok::<(), EnforcerError>(())
                } => sent?,
            }
            sequence = sequence.checked_add(1).ok_or(EnforcerError::Capacity)?;
        }
    }
    let response = match dispatch(&state, request.command.clone()) {
        Ok(response) => response,
        Err(code) => StopResponse::Error { code },
    };
    #[cfg(test)]
    state.faults.pause_after_reserved(&response).await?;
    let mut reply = Reply3 {
        kind: "reply_v3".into(),
        protocol: 3,
        session_id: session.id.clone(),
        epoch: Uuid3(session.epoch),
        request_id: request.request_id,
        sequence: 0,
        result: ReplyValue3::from_legacy(response, &state.root_binding)?,
        proof: empty_proof(),
    };
    let computed_proof = proof(
        &session.key,
        REPLY3,
        &(&session.id, request.core(), reply.core()),
    )?;
    reply.proof = computed_proof;
    write_frame(&mut stream, &reply, MAX_FRAME, IO_TIMEOUT).await?;
    Ok(())
}
pub(super) async fn request(
    connection: &mut StopConnection,
    command: StopRequest,
) -> EnforcerResult<ReplyValue3> {
    let session = connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?;
    let request = Request3::new(session, command)?;
    write_frame(&mut connection.stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
    let reply: Reply3 = read_frame(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await?;
    reply.validate(session, &request)?;
    match reply.result {
        ReplyValue3::Error { code } => Err(code),
        value => Ok(value),
    }
}

pub(super) struct Watch3Reader {
    session: Session3,
    request: Request3,
    next_sequence: u64,
}
impl Watch3Reader {
    pub(super) async fn read(
        &mut self,
        stream: &mut UnixStream,
    ) -> EnforcerResult<EnforcerStopStatusV2> {
        let frame: Watch3 = read_frame(stream, MAX_FRAME, IO_TIMEOUT).await?;
        if frame.kind != "watch_v3"
            || frame.protocol != 3
            || frame.session_id != self.session.id
            || frame.epoch.0 != self.session.epoch
            || frame.request_id != self.request.request_id
            || frame.sequence != self.next_sequence
        {
            return Err(EnforcerError::Unauthorized);
        }
        verify(
            &self.session.key,
            WATCH3,
            &(&self.session.id, self.request.core(), frame.core()),
            &frame.proof,
        )?;
        frame.value.validate(&self.session)?;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(EnforcerError::Capacity)?;
        Ok(frame.value.legacy())
    }
}
pub(super) async fn start_watch(connection: &mut StopConnection) -> EnforcerResult<Watch3Reader> {
    let session = connection
        .v3
        .as_ref()
        .ok_or(EnforcerError::Unauthorized)?
        .clone();
    let request = Request3::new(&session, StopRequest::WatchStops {})?;
    write_frame(&mut connection.stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
    Ok(Watch3Reader {
        session,
        request,
        next_sequence: 0,
    })
}

/// Distinct Stop3 client. Its private core shares only STOP algorithms with V2.
#[derive(Clone)]
pub struct EnforcerStopClientV3 {
    inner: EnforcerStopClient,
}
impl fmt::Debug for EnforcerStopClientV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerStopClientV3(<private>)")
    }
}
impl EnforcerStopClientV3 {
    pub async fn connect_or_spawn_existing_sealed_with_budget(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
        max_recovery_stop_owners: u32,
    ) -> EnforcerResult<Self> {
        Ok(Self {
            inner: EnforcerStopClient::connect_existing_sealed_protocol_budget(
                root,
                executable,
                max_slots,
                max_recovery_stop_owners,
                StopProtocol::V3,
            )
            .await?,
        })
    }
    pub async fn connect_or_spawn_for_recovery_with_budget(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
        max_recovery_stop_owners: u32,
    ) -> EnforcerResult<Self> {
        Ok(Self {
            inner: EnforcerStopClient::connect_with_affinity_budget(
                root,
                executable,
                max_slots,
                max_recovery_stop_owners,
                StopProtocol::V3,
                StopAffinity::Legacy,
            )
            .await?,
        })
    }
    pub async fn connect_or_spawn_existing_sealed(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
    ) -> EnforcerResult<Self> {
        Ok(Self {
            inner: EnforcerStopClient::connect_existing_sealed_protocol(
                root,
                executable,
                max_slots,
                StopProtocol::V3,
            )
            .await?,
        })
    }
    pub async fn connect_or_spawn_for_recovery(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
    ) -> EnforcerResult<Self> {
        Ok(Self {
            inner: EnforcerStopClient::connect_with_protocol(
                root,
                executable,
                max_slots,
                StopProtocol::V3,
            )
            .await?,
        })
    }
    pub async fn status(&self) -> EnforcerResult<EnforcerStopStatusV3> {
        let mut connection = self.inner.ensure_open().await?;
        let value = request(&mut connection, StopRequest::StatusStops {}).await?;
        let current_epoch = self
            .inner
            .0
            .cache
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .epoch;
        match value {
            ReplyValue3::StatusStops { value }
                if value.max_slots == self.inner.0.max_slots
                    && value.max_recovery_stop_owners == self.inner.0.max_recovery_stop_owners
                    && current_epoch == Some(connection.epoch)
                    && !self.inner.0.watch_failed.load(Ordering::SeqCst) =>
            {
                Ok(value)
            }
            _ => Err(EnforcerError::Unavailable),
        }
    }
    pub async fn query_stop(
        &self,
        arm_id: &str,
        registration_hash: &str,
    ) -> EnforcerResult<StopSlotV1> {
        self.inner.query_stop(arm_id, registration_hash).await
    }
    pub async fn restore_report_job(
        &self,
        report: Arc<StopRecoveryReportV1>,
        job_index: usize,
        generation: u64,
    ) -> (usize, bool) {
        self.inner
            .restore_report_job(report, job_index, generation)
            .await
    }
    pub async fn deliver_projection(
        &self,
        projection: Arc<CancelStopProjectionV1>,
        generation: u64,
    ) -> EnforcerResult<()> {
        self.inner.deliver_projection(projection, generation).await
    }
}

#[cfg(test)]
#[path = "recovery_v3_tests.rs"]
mod tests;

#[cfg(test)]
mod full_core_invariance_tests {
    use super::*;

    #[test]
    fn full_request_and_reply_core_hashes_do_not_change_when_proof_is_assigned() {
        let session = Session3 {
            key: [0x61; 32],
            id: Hash3("a".repeat(64)),
            epoch: Uuid::from_u128(1),
            root_binding: Hash3("b".repeat(64)),
            max_slots: 8,
        };
        let mut request = FullRequest3::new(&session, FullCommand::Status {}).unwrap();
        let original_proof = std::mem::replace(&mut request.proof, empty_proof());
        let request_before =
            Sha256::digest(canonical(&(&session.id, RequestedRole::Full, request.core())).unwrap());
        request.proof = original_proof;
        let request_after =
            Sha256::digest(canonical(&(&session.id, RequestedRole::Full, request.core())).unwrap());
        assert_eq!(request_before, request_after);
        request.validate(&session).unwrap();

        let mut reply = FullReply3 {
            kind: "full_reply_v3".into(),
            protocol: 3,
            session_id: session.id.clone(),
            epoch: Uuid3(session.epoch),
            request_id: request.request_id,
            sequence: 0,
            result: FullReply::Acknowledged {},
            proof: empty_proof(),
        };
        let reply_before = Sha256::digest(
            canonical(&(
                &session.id,
                RequestedRole::Full,
                request.core(),
                reply.core(),
            ))
            .unwrap(),
        );
        let computed_proof = proof(
            &session.key,
            REPLY3,
            &(
                &session.id,
                RequestedRole::Full,
                request.core(),
                reply.core(),
            ),
        )
        .unwrap();
        reply.proof = computed_proof;
        let reply_after = Sha256::digest(
            canonical(&(
                &session.id,
                RequestedRole::Full,
                request.core(),
                reply.core(),
            ))
            .unwrap(),
        );
        assert_eq!(reply_before, reply_after);
        reply.validate(&session, &request, 0).unwrap();
    }
}
