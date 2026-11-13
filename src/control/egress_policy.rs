//! Closed policy administration data. No DTO is a control or network authority.
use crate::egress::{Authority, EgressPolicyV1, EgressTransport, PolicyLimits};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub use crate::audit::egress::{
    EgressPolicyReceiptV1, EgressPolicySelectorV1, EgressPolicyStateV1,
};

pub const REQUEST_BYTES: usize = 65_536;
pub const RECEIPT_BYTES: usize = 4_096;
type Result<T> = std::result::Result<T, EgressPolicyAdminErrorV1>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressPolicyAdminErrorV1 {
    InvalidRequest,
    InvalidResponse,
    Unauthenticated,
    Forbidden,
    ScopeConflict,
    Conflict,
    MigrationRequired,
    Capacity,
    Busy,
    Clock,
    Unavailable,
    Transport,
}
impl EgressPolicyAdminErrorV1 {
    pub fn code(&self) -> &'static str {
        use EgressPolicyAdminErrorV1::*;
        match self {
            InvalidRequest => "egress_policy_invalid_request",
            InvalidResponse => "egress_policy_invalid_response",
            Unauthenticated => "egress_policy_unauthenticated",
            Forbidden => "egress_policy_forbidden",
            ScopeConflict => "egress_policy_scope_conflict",
            Conflict => "egress_policy_conflict",
            MigrationRequired => "egress_policy_migration_required",
            Capacity => "egress_policy_capacity_exhausted",
            Busy => "egress_policy_busy",
            Clock => "egress_policy_clock_unavailable",
            Unavailable => "egress_policy_unavailable",
            Transport => "egress_policy_transport_unresolved",
        }
    }
    pub fn http_status(&self) -> Option<u16> {
        use EgressPolicyAdminErrorV1::*;
        Some(match self {
            InvalidRequest => 400,
            Unauthenticated => 401,
            Forbidden => 403,
            ScopeConflict | Conflict | MigrationRequired => 409,
            Capacity => 429,
            Busy | Clock | Unavailable => 503,
            InvalidResponse | Transport => return None,
        })
    }
}
impl fmt::Display for EgressPolicyAdminErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for EgressPolicyAdminErrorV1 {}

fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(EgressPolicyAdminErrorV1::InvalidRequest)
    }
}
fn number(value: u64, zero: bool) -> Result<()> {
    require((zero || value > 0) && value <= i64::MAX as u64)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn canonical_uuid<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Uuid, D::Error> {
    let raw = String::deserialize(input)?;
    let id =
        Uuid::parse_str(&raw).map_err(|_| serde::de::Error::custom("invalid canonical UUID"))?;
    if id.is_nil() || id.to_string() != raw {
        return Err(serde::de::Error::custom("invalid canonical UUID"));
    }
    Ok(id)
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
    require(bytes.len() <= REQUEST_BYTES)?;
    Ok(bytes)
}
macro_rules! private_debug {
    ($($name:ty),+ $(,)?) => {$(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("EgressPolicyData(<private metadata>)")
            }
        }
    )+};
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicyExpectedScopeV1 {
    tenant_id: String,
    control_namespace_uuid: String,
}
impl EgressPolicyExpectedScopeV1 {
    pub fn new(tenant_id: &str, control_namespace_uuid: &str) -> Result<Self> {
        crate::tenancy::validate_tenant_id(tenant_id)
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        require(control_namespace_uuid.len() == 36)?;
        let result = Self {
            tenant_id: tenant_id.into(),
            control_namespace_uuid: control_namespace_uuid.into(),
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<()> {
        crate::tenancy::validate_tenant_id(&self.tenant_id)
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        let id = Uuid::parse_str(&self.control_namespace_uuid)
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        require(!id.is_nil() && id.to_string() == self.control_namespace_uuid)
    }
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn control_namespace_uuid(&self) -> &str {
        &self.control_namespace_uuid
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Destination {
    #[serde(deserialize_with = "bounded_name")]
    name: String,
    port: u16,
    transport: Transport,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Transport {
    ConnectTcpOpaque,
}

fn bounded_name<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<String, D::Error> {
    struct Name;
    impl<'de> serde::de::Visitor<'de> for Name {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded exact DNS name")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.len() > 253 {
                return Err(E::custom("DNS name exceeds bound"));
            }
            Ok(value.to_owned())
        }
    }
    input.deserialize_str(Name)
}
fn bounded_destinations<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Vec<Destination>, D::Error> {
    struct Destinations;
    impl<'de> serde::de::Visitor<'de> for Destinations {
        type Value = Vec<Destination>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most128 exact destinations")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(128));
            while let Some(value) = sequence.next_element::<Destination>()? {
                if values.len() == 128 {
                    return Err(serde::de::Error::custom("destination count exceeds bound"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    input.deserialize_seq(Destinations)
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct EgressPolicyInputV1 {
    not_before_ms: u64,
    expires_at_ms: u64,
    destinations: Vec<Destination>,
    max_destinations: u16,
    max_resolution_ips: u8,
    #[serde(skip)]
    resolved_limits: PolicyLimits,
}
impl<'de> Deserialize<'de> for EgressPolicyInputV1 {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            not_before_ms: u64,
            expires_at_ms: u64,
            #[serde(deserialize_with = "bounded_destinations")]
            destinations: Vec<Destination>,
            max_destinations: u16,
            max_resolution_ips: u8,
        }
        let raw = Raw::deserialize(input)?;
        let limits = PolicyLimits::new(
            raw.max_destinations as usize,
            raw.max_resolution_ips as usize,
        )
        .map_err(|_| serde::de::Error::custom("invalid policy limits"))?;
        let value = Self {
            not_before_ms: raw.not_before_ms,
            expires_at_ms: raw.expires_at_ms,
            destinations: raw.destinations,
            max_destinations: raw.max_destinations,
            max_resolution_ips: raw.max_resolution_ips,
            resolved_limits: limits,
        };
        let policy = value
            .to_policy(1)
            .map_err(|_| serde::de::Error::custom("invalid policy input"))?;
        Ok(Self::from_core(&policy))
    }
}
impl EgressPolicyInputV1 {
    pub fn new(
        not_before_ms: u64,
        expires_at_ms: u64,
        destinations: &[Authority],
        limits: PolicyLimits,
    ) -> Result<Self> {
        number(not_before_ms, true)?;
        number(expires_at_ms, false)?;
        let core = EgressPolicyV1::new(1, not_before_ms, expires_at_ms, destinations, limits)
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        Ok(Self::from_core(&core))
    }
    fn from_core(policy: &EgressPolicyV1) -> Self {
        Self {
            not_before_ms: policy.not_before_ms(),
            expires_at_ms: policy.expires_at_ms(),
            destinations: policy
                .destinations()
                .iter()
                .map(|d| Destination {
                    name: d.name().into(),
                    port: d.port(),
                    transport: Transport::ConnectTcpOpaque,
                })
                .collect(),
            max_destinations: policy.limits().max_destinations() as u16,
            max_resolution_ips: policy.limits().max_resolution_ips() as u8,
            resolved_limits: policy.limits(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.to_policy(1).map(|_| ())
    }
    pub fn to_policy(&self, revision: u64) -> Result<EgressPolicyV1> {
        number(revision, false)?;
        number(self.not_before_ms, true)?;
        number(self.expires_at_ms, false)?;
        require(
            self.destinations.len() <= 128 && self.destinations.iter().all(|d| d.name.len() <= 253),
        )?;
        let limits = PolicyLimits::new(
            self.max_destinations as usize,
            self.max_resolution_ips as usize,
        )
        .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        let destinations = self
            .destinations
            .iter()
            .map(|d| {
                Authority::new(&d.name, d.port, EgressTransport::ConnectTcpOpaque)
                    .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)
            })
            .collect::<Result<Vec<_>>>()?;
        EgressPolicyV1::new(
            revision,
            self.not_before_ms,
            self.expires_at_ms,
            &destinations,
            limits,
        )
        .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)
    }
    pub fn not_before_ms(&self) -> u64 {
        self.not_before_ms
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
    pub fn limits(&self) -> PolicyLimits {
        self.resolved_limits
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicyTenantCeilingV1 {
    revision: u64,
    epoch: u64,
    digest: String,
}
impl EgressPolicyTenantCeilingV1 {
    pub fn new(revision: u64, epoch: u64, digest: [u8; 32]) -> Result<Self> {
        let value = Self {
            revision,
            epoch,
            digest: hex::encode(digest),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        number(self.revision, false)?;
        number(self.epoch, false)?;
        require(digest(&self.digest))
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressReplacePolicyRequestV1 {
    version: u32,
    expected_scope: EgressPolicyExpectedScopeV1,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: Uuid,
    selector: EgressPolicySelectorV1,
    expected_revision: u64,
    expected_epoch: u64,
    policy: EgressPolicyInputV1,
    tenant_ceiling: Option<EgressPolicyTenantCeilingV1>,
}
impl EgressReplacePolicyRequestV1 {
    pub fn new(
        operation_id: Uuid,
        expected_scope: EgressPolicyExpectedScopeV1,
        selector: EgressPolicySelectorV1,
        expected_revision: u64,
        expected_epoch: u64,
        policy: EgressPolicyInputV1,
        tenant_ceiling: Option<EgressPolicyTenantCeilingV1>,
    ) -> Result<Self> {
        Self {
            version: 1,
            operation_id,
            expected_scope,
            selector,
            expected_revision,
            expected_epoch,
            policy,
            tenant_ceiling,
        }
        .normalized()
    }
    fn normalized(mut self) -> Result<Self> {
        self.validate()?;
        self.policy =
            EgressPolicyInputV1::from_core(&self.policy.to_policy(self.expected_revision + 1)?);
        Ok(self)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() <= REQUEST_BYTES)?;
        serde_json::from_slice::<Self>(bytes)
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?
            .normalized()
    }
    pub fn validate(&self) -> Result<()> {
        require(self.version == 1 && !self.operation_id.is_nil())?;
        self.expected_scope.validate()?;
        self.selector
            .validate()
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        number(self.expected_revision, true)?;
        number(self.expected_epoch, true)?;
        require((self.expected_revision == 0) == (self.expected_epoch == 0))?;
        let revision = self
            .expected_revision
            .checked_add(1)
            .ok_or(EgressPolicyAdminErrorV1::InvalidRequest)?;
        number(revision, false)?;
        number(
            self.expected_epoch
                .checked_add(1)
                .ok_or(EgressPolicyAdminErrorV1::InvalidRequest)?,
            false,
        )?;
        self.policy.to_policy(revision)?;
        match (&self.selector, &self.tenant_ceiling) {
            (EgressPolicySelectorV1::Tenant, None) => {}
            (EgressPolicySelectorV1::Session(_), Some(c)) => c.validate()?,
            _ => return Err(EgressPolicyAdminErrorV1::InvalidRequest),
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(&self.clone().normalized()?)
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn expected_scope(&self) -> &EgressPolicyExpectedScopeV1 {
        &self.expected_scope
    }
    pub fn selector(&self) -> &EgressPolicySelectorV1 {
        &self.selector
    }
    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }
    pub fn expected_epoch(&self) -> u64 {
        self.expected_epoch
    }
    pub fn policy(&self) -> &EgressPolicyInputV1 {
        &self.policy
    }
    pub fn tenant_ceiling(&self) -> Option<&EgressPolicyTenantCeilingV1> {
        self.tenant_ceiling.as_ref()
    }
    pub(crate) fn store_request(
        &self,
    ) -> anyhow::Result<crate::audit::egress::EgressReplacePolicyV1> {
        self.validate()?;
        let ceiling = self
            .tenant_ceiling
            .as_ref()
            .map(|c| {
                let bytes: [u8; 32] = hex::decode(c.digest())?
                    .try_into()
                    .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
                crate::audit::egress::EgressTenantCeilingV1::new(c.revision(), c.epoch(), bytes)
            })
            .transpose()?;
        crate::audit::egress::EgressReplacePolicyV1::new(
            self.operation_id,
            self.selector.clone(),
            self.expected_revision,
            self.expected_epoch,
            self.policy.to_policy(self.expected_revision + 1)?,
            ceiling,
        )
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressRevokePolicyRequestV1 {
    version: u32,
    expected_scope: EgressPolicyExpectedScopeV1,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: Uuid,
    selector: EgressPolicySelectorV1,
    expected_revision: u64,
    expected_epoch: u64,
}
impl EgressRevokePolicyRequestV1 {
    pub fn new(
        operation_id: Uuid,
        expected_scope: EgressPolicyExpectedScopeV1,
        selector: EgressPolicySelectorV1,
        expected_revision: u64,
        expected_epoch: u64,
    ) -> Result<Self> {
        let result = Self {
            version: 1,
            operation_id,
            expected_scope,
            selector,
            expected_revision,
            expected_epoch,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() <= REQUEST_BYTES)?;
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        require(self.version == 1 && !self.operation_id.is_nil())?;
        self.expected_scope.validate()?;
        self.selector
            .validate()
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidRequest)?;
        number(self.expected_revision, false)?;
        number(self.expected_epoch, false)?;
        number(
            self.expected_epoch
                .checked_add(1)
                .ok_or(EgressPolicyAdminErrorV1::InvalidRequest)?,
            false,
        )
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn expected_scope(&self) -> &EgressPolicyExpectedScopeV1 {
        &self.expected_scope
    }
    pub fn selector(&self) -> &EgressPolicySelectorV1 {
        &self.selector
    }
    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }
    pub fn expected_epoch(&self) -> u64 {
        self.expected_epoch
    }
    pub(crate) fn store_request(
        &self,
    ) -> anyhow::Result<crate::audit::egress::EgressRevokePolicyV1> {
        self.validate()?;
        crate::audit::egress::EgressRevokePolicyV1::new(
            self.operation_id,
            self.selector.clone(),
            self.expected_revision,
            self.expected_epoch,
        )
    }
}
private_debug!(
    EgressPolicyExpectedScopeV1,
    EgressPolicyInputV1,
    EgressPolicyTenantCeilingV1,
    EgressReplacePolicyRequestV1,
    EgressRevokePolicyRequestV1
);

impl EgressPolicyReceiptV1 {
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > RECEIPT_BYTES {
            return Err(EgressPolicyAdminErrorV1::InvalidResponse);
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| EgressPolicyAdminErrorV1::InvalidResponse)?;
        value.validate_common()?;
        if value
            .canonical_bytes()
            .map_err(|_| EgressPolicyAdminErrorV1::InvalidResponse)?
            != bytes
        {
            return Err(EgressPolicyAdminErrorV1::InvalidResponse);
        }
        Ok(value)
    }
    fn validate_common(&self) -> Result<()> {
        let valid = self.version() == 1
            && !self.operation_id().is_nil()
            && self.event_id() == self.operation_id().to_string()
            && self.event_seq() > 0
            && digest(self.event_hash())
            && digest(self.policy_digest())
            && self.selector().validate().is_ok()
            && self.revision() > 0
            && self.revision() <= i64::MAX as u64
            && self.before_epoch() <= i64::MAX as u64
            && self.after_epoch() <= i64::MAX as u64
            && self.before_epoch().checked_add(1) == Some(self.after_epoch())
            && matches!(
                (self.action(), self.state()),
                ("replace", EgressPolicyStateV1::Active)
                    | ("revoke", EgressPolicyStateV1::RevokeRequested)
            );
        let time = chrono::DateTime::parse_from_rfc3339(self.received_at()).ok();
        let canonical_time = time.is_some_and(|time| {
            crate::broker_registry::format_time(time.with_timezone(&chrono::Utc))
                == self.received_at()
        });
        if !valid || !canonical_time {
            return Err(EgressPolicyAdminErrorV1::InvalidResponse);
        }
        Ok(())
    }
    pub fn validate_for_replace(&self, request: &EgressReplacePolicyRequestV1) -> Result<()> {
        request.validate()?;
        self.validate_common()?;
        let policy = request
            .policy()
            .to_policy(request.expected_revision() + 1)?;
        if self.action() != "replace"
            || self.state() != EgressPolicyStateV1::Active
            || self.operation_id() != request.operation_id()
            || self.selector() != request.selector()
            || self.revision() != request.expected_revision() + 1
            || self.before_epoch() != request.expected_epoch()
            || self.policy_digest() != hex::encode(policy.digest())
        {
            return Err(EgressPolicyAdminErrorV1::InvalidResponse);
        }
        Ok(())
    }
    pub fn validate_for_revoke(&self, request: &EgressRevokePolicyRequestV1) -> Result<()> {
        request.validate()?;
        self.validate_common()?;
        if self.action() != "revoke"
            || self.state() != EgressPolicyStateV1::RevokeRequested
            || self.operation_id() != request.operation_id()
            || self.selector() != request.selector()
            || self.revision() != request.expected_revision()
            || self.before_epoch() != request.expected_epoch()
        {
            return Err(EgressPolicyAdminErrorV1::InvalidResponse);
        }
        // Deliberately no digest-value correlation: revoke carries no policy.
        Ok(())
    }
}

#[cfg(test)]
mod tests;
