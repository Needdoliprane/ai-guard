//! Pure private-envelope checks shared by offline preparation and the retained
//! runtime registry. Configuration is not a bearer authentication or receipt.
use super::{
    CredentialIndex, CredentialRole, EnvelopeEntryV1, EnvelopeRoleV1, EnvelopeV1,
    MAX_ENVELOPE_BYTES, MAX_ENVELOPE_ENTRIES, ReviewCredentialFailure, credential_digest,
    valid_credential,
};
use crate::api::auth::ControlCredentialInput;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechnicalCredentialRoleV1 {
    Runner,
    Viewer,
}
impl TechnicalCredentialRoleV1 {
    pub(super) fn internal(self) -> CredentialRole {
        match self {
            Self::Runner => CredentialRole::Runner,
            Self::Viewer => CredentialRole::Viewer,
        }
    }
}

/// Intentionally neither Debug nor serializable: contains a private token.
pub struct TechnicalCredentialInput<'a> {
    pub tenant_id: &'a str,
    pub role: TechnicalCredentialRoleV1,
    pub credential_id: Uuid,
    pub generation: u64,
    pub token: &'a str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TechnicalCredentialIdentityV1 {
    pub tenant_id: String,
    pub role: TechnicalCredentialRoleV1,
    pub credential_id: Uuid,
    pub generation: u64,
}

/// Resolved independent expectations only. No Store, owner, source or receipt.
pub struct EnvelopeExpectationsV1 {
    pub(super) installation_id: Uuid,
    pub(super) closure_id: Uuid,
    pub(super) closure_revision: u64,
    pub(super) public_digest: String,
    pub(super) tenant_order: Vec<String>,
    pub(super) identities: BTreeMap<(String, TechnicalCredentialRoleV1), (Uuid, u64)>,
    pub(super) index: CredentialIndex,
}

impl EnvelopeExpectationsV1 {
    pub fn from_configured_inputs(
        installation_id: Uuid,
        closure_id: Uuid,
        closure_revision: u64,
        public_digest: String,
        tenant_order: &[String],
        inputs: &[TechnicalCredentialInput<'_>],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !installation_id.is_nil()
                && !closure_id.is_nil()
                && closure_revision > 0
                && public_digest.len() == 64
                && public_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "human_credential_configuration_invalid"
        );
        let index = CredentialIndex::from_configured_inputs(
            tenant_order,
            inputs.iter().map(|input| ControlCredentialInput {
                tenant_id: input.tenant_id,
                role: input.role.internal(),
                token: input.token,
            }),
        )?;
        let mut identities = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for input in inputs {
            anyhow::ensure!(
                !input.credential_id.is_nil()
                    && input.generation > 0
                    && ids.insert(input.credential_id)
                    && identities
                        .insert(
                            (input.tenant_id.to_owned(), input.role),
                            (input.credential_id, input.generation)
                        )
                        .is_none(),
                "human_credential_configuration_invalid"
            );
        }
        Ok(Self {
            installation_id,
            closure_id,
            closure_revision,
            public_digest,
            tenant_order: tenant_order.to_vec(),
            identities,
            index,
        })
    }
}

pub(super) struct ValidatedEnvelopeConfiguration {
    pub(super) runners: HashMap<[u8; 32], TechnicalCredentialIdentityV1>,
}

pub fn prepare_envelope_configuration(
    expected: &EnvelopeExpectationsV1,
    inputs: &[TechnicalCredentialInput<'_>],
) -> anyhow::Result<Vec<u8>> {
    let mut entries: Vec<_> = inputs
        .iter()
        .map(|input| EnvelopeEntryV1 {
            tenant_id: input.tenant_id.to_owned(),
            role: match input.role {
                TechnicalCredentialRoleV1::Runner => EnvelopeRoleV1::Runner,
                TechnicalCredentialRoleV1::Viewer => EnvelopeRoleV1::Viewer,
            },
            credential_id: input.credential_id,
            generation: input.generation,
            token: input.token.to_owned(),
        })
        .collect();
    entries.sort_by(|a, b| {
        (&a.tenant_id, a.role.role().as_str()).cmp(&(&b.tenant_id, b.role.role().as_str()))
    });
    let bytes = serde_json::to_vec(&EnvelopeV1 {
        version: 1,
        installation_id: expected.installation_id,
        closure_id: expected.closure_id,
        closure_revision: expected.closure_revision,
        control_config_public_digest: expected.public_digest.clone(),
        entries,
    })
    .map_err(|_| ReviewCredentialFailure::InvalidConfiguration)?;
    validate_envelope_configuration(&bytes, expected)?;
    Ok(bytes)
}

/// Exposes only success/failure. The configuration result remains private.
pub fn validate_technical_credentials_configuration(
    bytes: &[u8],
    expected: &EnvelopeExpectationsV1,
) -> anyhow::Result<()> {
    validate_envelope_configuration(bytes, expected).map(|_| ())
}

pub(super) fn validate_envelope_configuration(
    bytes: &[u8],
    expected: &EnvelopeExpectationsV1,
) -> anyhow::Result<ValidatedEnvelopeConfiguration> {
    let invalid = || ReviewCredentialFailure::InvalidConfiguration;
    anyhow::ensure!(
        !bytes.is_empty()
            && bytes.len() <= MAX_ENVELOPE_BYTES
            && !expected.tenant_order.is_empty()
            && expected.tenant_order.len() <= MAX_ENVELOPE_ENTRIES / 2,
        "review_credential_configuration_invalid"
    );
    // The bounded visitor rejects duplicate keys and collection overflow before
    // insertion, including ignored policy fields in other offline documents.
    let envelope: EnvelopeV1 = crate::human_authority::migration::operator::decode_closed(
        bytes,
        MAX_ENVELOPE_BYTES,
        MAX_ENVELOPE_ENTRIES,
    )
    .map_err(|_| invalid())?;
    validate_parsed_envelope(envelope, expected)
}

pub(super) fn validate_runtime_configuration(
    bytes: &[u8],
    index: &CredentialIndex,
    tenants: &[String],
    closure: &crate::human_authority::HumanAuthorityClosureV1,
) -> anyhow::Result<ValidatedEnvelopeConfiguration> {
    anyhow::ensure!(
        !bytes.is_empty()
            && bytes.len() <= MAX_ENVELOPE_BYTES
            && !tenants.is_empty()
            && tenants.len() <= 256,
        "review_credential_configuration_invalid"
    );
    let envelope: EnvelopeV1 = crate::human_authority::migration::operator::decode_closed(
        bytes,
        MAX_ENVELOPE_BYTES,
        MAX_ENVELOPE_ENTRIES,
    )?;
    let identities = envelope
        .entries
        .iter()
        .map(|e| {
            (
                (
                    e.tenant_id.clone(),
                    match e.role {
                        EnvelopeRoleV1::Runner => TechnicalCredentialRoleV1::Runner,
                        EnvelopeRoleV1::Viewer => TechnicalCredentialRoleV1::Viewer,
                    },
                ),
                (e.credential_id, e.generation),
            )
        })
        .collect();
    let expected = EnvelopeExpectationsV1 {
        installation_id: closure.installation_id,
        closure_id: closure.closure_id,
        closure_revision: closure.closure_revision,
        public_digest: closure.configuration.public_config_digest.clone(),
        tenant_order: tenants.to_vec(),
        identities,
        index: index.clone(),
    };
    validate_parsed_envelope(envelope, &expected)
}

fn validate_parsed_envelope(
    envelope: EnvelopeV1,
    expected: &EnvelopeExpectationsV1,
) -> anyhow::Result<ValidatedEnvelopeConfiguration> {
    let invalid = || ReviewCredentialFailure::InvalidConfiguration;
    if envelope.version != 1
        || envelope.installation_id != expected.installation_id
        || envelope.closure_id != expected.closure_id
        || envelope.closure_revision != expected.closure_revision
        || envelope.control_config_public_digest != expected.public_digest
        || envelope.entries.len() != expected.index.entries.len()
        || envelope.entries.len() != expected.tenant_order.len() * 2
        || envelope.entries.len() > MAX_ENVELOPE_ENTRIES
    {
        return Err(invalid().into());
    }
    let tenants: HashMap<_, _> = expected
        .tenant_order
        .iter()
        .enumerate()
        .map(|(i, t)| (t.as_str(), i))
        .collect();
    let mut ids = HashSet::new();
    let mut digests = HashSet::new();
    let mut tuples = HashSet::new();
    let mut runners = HashMap::new();
    for entry in envelope.entries {
        let tenant_index = *tenants.get(entry.tenant_id.as_str()).ok_or_else(invalid)?;
        let role = entry.role.role();
        let technical_role = match role {
            CredentialRole::Runner => TechnicalCredentialRoleV1::Runner,
            CredentialRole::Viewer => TechnicalCredentialRoleV1::Viewer,
        };
        let digest = credential_digest(&entry.token);
        let binding = expected.index.lookup_digest(&digest).ok_or_else(invalid)?;
        if !valid_credential(&entry.token)
            || entry.credential_id.is_nil()
            || entry.generation == 0
            || binding.tenant_index != tenant_index
            || binding.role != role
            || !ids.insert(entry.credential_id)
            || !digests.insert(digest)
            || !tuples.insert((tenant_index, role.as_str()))
            || expected
                .identities
                .get(&(entry.tenant_id.clone(), technical_role))
                != Some(&(entry.credential_id, entry.generation))
        {
            return Err(invalid().into());
        }
        if role == CredentialRole::Runner {
            runners.insert(
                digest,
                TechnicalCredentialIdentityV1 {
                    tenant_id: entry.tenant_id,
                    role: technical_role,
                    credential_id: entry.credential_id,
                    generation: entry.generation,
                },
            );
        }
    }
    if digests.len() != expected.index.entries.len() || runners.len() != expected.tenant_order.len()
    {
        return Err(invalid().into());
    }
    Ok(ValidatedEnvelopeConfiguration { runners })
}

#[cfg(test)]
mod tests;
