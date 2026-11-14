//! Historical registry metadata, never a policy, permission or execution proof.
use super::{
    EgressPolicyAdminErrorV1 as E, EgressPolicyExpectedScopeV1, EgressPolicySelectorV1,
    EgressPolicyStateV1, RECEIPT_BYTES, REQUEST_BYTES, Result, digest, encode, number, require,
};
use serde::{Deserialize, Serialize};
use std::fmt;

// Serde's internally tagged unit visitor discards additional fields. Struct
// variants keep the selector closed even before canonical-byte correlation.
fn strict_selector<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<EgressPolicySelectorV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Selector {
        Tenant {},
        Session { session_id: String },
    }
    match Selector::deserialize(input)? {
        Selector::Tenant {} => Ok(EgressPolicySelectorV1::Tenant),
        Selector::Session { session_id } => EgressPolicySelectorV1::session(&session_id)
            .map_err(|_| serde::de::Error::custom("invalid policy selector")),
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicyInspectionRequestV1 {
    version: u32,
    #[serde(deserialize_with = "strict_selector")]
    selector: EgressPolicySelectorV1,
}
impl EgressPolicyInspectionRequestV1 {
    pub fn new(selector: EgressPolicySelectorV1) -> Result<Self> {
        let value = Self { version: 1, selector };
        value.validate()?;
        Ok(value)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() <= REQUEST_BYTES)?;
        let value: Self = serde_json::from_slice(bytes).map_err(|_| E::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        require(self.version == 1)?;
        self.selector.validate().map_err(|_| E::InvalidRequest)
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn selector(&self) -> &EgressPolicySelectorV1 {
        &self.selector
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "presence", rename_all = "snake_case", deny_unknown_fields)]
pub enum EgressPolicyObservedHeadV1 {
    Absent,
    Present {
        revision: u64,
        epoch: u64,
        state: EgressPolicyStateV1,
    },
}
impl<'de> Deserialize<'de> for EgressPolicyObservedHeadV1 {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "presence", rename_all = "snake_case", deny_unknown_fields)]
        enum Head {
            Absent {},
            Present { revision: u64, epoch: u64, state: EgressPolicyStateV1 },
        }
        Ok(match Head::deserialize(input)? {
            Head::Absent {} => Self::Absent,
            Head::Present { revision, epoch, state } => Self::Present { revision, epoch, state },
        })
    }
}
impl EgressPolicyObservedHeadV1 {
    fn validate(&self) -> Result<()> {
        if let Self::Present { revision, epoch, .. } = self {
            number(*revision, false)?;
            number(*epoch, false)?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum EgressPolicyObservedCeilingV1 {
    NotApplicable,
    Absent,
    Inactive { revision: u64, epoch: u64 },
    Active { revision: u64, epoch: u64, digest: String },
}
impl<'de> Deserialize<'de> for EgressPolicyObservedCeilingV1 {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
        enum Ceiling {
            NotApplicable {},
            Absent {},
            Inactive { revision: u64, epoch: u64 },
            Active { revision: u64, epoch: u64, digest: String },
        }
        Ok(match Ceiling::deserialize(input)? {
            Ceiling::NotApplicable {} => Self::NotApplicable,
            Ceiling::Absent {} => Self::Absent,
            Ceiling::Inactive { revision, epoch } => Self::Inactive { revision, epoch },
            Ceiling::Active { revision, epoch, digest } => Self::Active { revision, epoch, digest },
        })
    }
}
impl EgressPolicyObservedCeilingV1 {
    fn validate(&self) -> Result<()> {
        match self {
            Self::NotApplicable | Self::Absent => Ok(()),
            Self::Inactive { revision, epoch } => {
                number(*revision, false)?;
                number(*epoch, false)
            }
            Self::Active { revision, epoch, digest: value } => {
                number(*revision, false)?;
                number(*epoch, false)?;
                require(digest(value))
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPolicyObservedSessionResourceV1 {
    NotApplicable,
    Active,
    Inactive,
    Absent,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Coverage {
    PolicyRegistryMetadata,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicyInspectionV1 {
    version: u32,
    schema_version: u32,
    scope: EgressPolicyExpectedScopeV1,
    #[serde(deserialize_with = "strict_selector")]
    selector: EgressPolicySelectorV1,
    head: EgressPolicyObservedHeadV1,
    tenant_ceiling: EgressPolicyObservedCeilingV1,
    session_resource: EgressPolicyObservedSessionResourceV1,
    observed_at: String,
    coverage: Coverage,
    historical: bool,
}
impl EgressPolicyInspectionV1 {
    /// The server supplies only metadata obtained from one checked transaction.
    pub(crate) fn observed(
        scope: EgressPolicyExpectedScopeV1,
        selector: EgressPolicySelectorV1,
        head: EgressPolicyObservedHeadV1,
        tenant_ceiling: EgressPolicyObservedCeilingV1,
        session_resource: EgressPolicyObservedSessionResourceV1,
        observed_at: String,
    ) -> Result<Self> {
        let value = Self {
            version: 1,
            schema_version: 15,
            scope,
            selector,
            head,
            tenant_ceiling,
            session_resource,
            observed_at,
            coverage: Coverage::PolicyRegistryMetadata,
            historical: true,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > RECEIPT_BYTES {
            return Err(E::InvalidResponse);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| E::InvalidResponse)?;
        value.validate()?;
        if value.canonical_bytes()? != bytes {
            return Err(E::InvalidResponse);
        }
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        let shape = (|| {
            require(self.version == 1 && self.schema_version == 15 && self.historical)?;
            self.scope.validate()?;
            self.selector.validate().map_err(|_| E::InvalidRequest)?;
            self.head.validate()?;
            self.tenant_ceiling.validate()?;
            match &self.selector {
                EgressPolicySelectorV1::Tenant => require(
                    self.tenant_ceiling == EgressPolicyObservedCeilingV1::NotApplicable
                        && self.session_resource == EgressPolicyObservedSessionResourceV1::NotApplicable,
                )?,
                EgressPolicySelectorV1::Session(_) => require(
                    self.tenant_ceiling != EgressPolicyObservedCeilingV1::NotApplicable
                        && self.session_resource != EgressPolicyObservedSessionResourceV1::NotApplicable,
                )?,
            }
            let canonical_time = chrono::DateTime::parse_from_rfc3339(&self.observed_at)
                .ok()
                .is_some_and(|time| {
                    crate::broker_registry::format_time(time.with_timezone(&chrono::Utc))
                        == self.observed_at
                });
            require(canonical_time)
        })();
        shape.map_err(|_| E::InvalidResponse)
    }
    pub fn validate_for(&self, request: &EgressPolicyInspectionRequestV1) -> Result<()> {
        request.validate()?;
        self.validate()?;
        if self.selector != *request.selector() {
            return Err(E::InvalidResponse);
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| E::InvalidResponse)?;
        if bytes.len() > RECEIPT_BYTES {
            return Err(E::InvalidResponse);
        }
        Ok(bytes)
    }
    pub fn scope(&self) -> &EgressPolicyExpectedScopeV1 {
        &self.scope
    }
    pub fn selector(&self) -> &EgressPolicySelectorV1 {
        &self.selector
    }
    pub fn head(&self) -> &EgressPolicyObservedHeadV1 {
        &self.head
    }
    pub fn tenant_ceiling(&self) -> &EgressPolicyObservedCeilingV1 {
        &self.tenant_ceiling
    }
    pub fn session_resource(&self) -> EgressPolicyObservedSessionResourceV1 {
        self.session_resource
    }
    pub fn observed_at(&self) -> &str {
        &self.observed_at
    }
    pub fn coverage(&self) -> &'static str {
        "policy_registry_metadata"
    }
    pub fn historical(&self) -> bool {
        self.historical
    }
}

macro_rules! inspection_debug {
    ($($name:ty),+ $(,)?) => {$(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("EgressPolicyInspection(<historical metadata>)")
            }
        }
    )+};
}
inspection_debug!(
    EgressPolicyInspectionRequestV1,
    EgressPolicyInspectionV1,
    EgressPolicyObservedHeadV1,
    EgressPolicyObservedCeilingV1,
    EgressPolicyObservedSessionResourceV1
);

#[cfg(test)]
mod tests;
