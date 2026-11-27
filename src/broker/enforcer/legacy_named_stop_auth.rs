//! Private mutual-authentication codec for a single named STOP packet.
use super::{
    Authentication, Challenge, EnforcerError, EnforcerResult, deserialize_legacy_optional_uuid,
    hex_hash,
};
use crate::broker::journal::canonical;
use hmac::{Hmac, Mac};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Error as _, MapAccess, Visitor, value::MapAccessDeserializer},
};
use sha2::{Digest, Sha256};
use std::fmt;
use uuid::Uuid;

pub(super) const HANDSHAKE_FRAME: u32 = 1024;
const VERSION: u32 = 1;
const ROLE: &str = "stops_only";
const CLIENT_DOMAIN: &[u8] = b"ai-guard:legacy-stops-only-client-auth:v1\0";
const GUARDIAN_DOMAIN: &[u8] = b"ai-guard:legacy-stops-only-guardian-auth:v1\0";
const ACK_DOMAIN: &[u8] = b"ai-guard:legacy-stops-only-ack:v1\0";
const REQUEST_DOMAIN: &[u8] = b"ai-guard:legacy-stops-only-request:v1\0";

/// Decode one first frame. Each branch requires its entire closed shape.
/// Once selected, its proof is verified only for that branch.
pub(super) enum FirstAuthentication {
    Legacy(Authentication),
    StopsOnly(StopsOnlyAuthenticationV1),
}

impl<'de> Deserialize<'de> for FirstAuthentication {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct FirstFrame;
        impl<'de> Visitor<'de> for FirstFrame {
            type Value = FirstAuthentication;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a closed private authentication object")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                #[derive(Deserialize)]
                #[serde(untagged)]
                enum Wire {
                    Legacy(Authentication),
                    StopsOnly(StopsOnlyAuthenticationV1),
                }
                Ok(match Wire::deserialize(MapAccessDeserializer::new(map))? {
                    Wire::Legacy(authentication) => FirstAuthentication::Legacy(authentication),
                    Wire::StopsOnly(authentication) => {
                        FirstAuthentication::StopsOnly(authentication)
                    }
                })
            }
        }
        decoder.deserialize_map(FirstFrame)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthenticationKindV1 {
    AuthenticateLegacyStopsOnly,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StopsOnlyAuthenticationV1 {
    kind: AuthenticationKindV1,
    #[serde(deserialize_with = "deserialize_version")]
    version: u32,
    #[serde(deserialize_with = "deserialize_nonce")]
    client_nonce: Uuid,
    #[serde(deserialize_with = "deserialize_hash")]
    root_binding: String,
    #[serde(deserialize_with = "deserialize_hash")]
    request_hash: String,
    #[serde(deserialize_with = "deserialize_hash")]
    client_proof: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrantKindV1 {
    GrantedLegacyStopsOnly,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StopsOnlyGrantV1 {
    kind: GrantKindV1,
    #[serde(deserialize_with = "deserialize_version")]
    version: u32,
    #[serde(deserialize_with = "deserialize_hash")]
    server_proof: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AckKindV1 {
    LegacyStopsOnlyAck,
}

/// Acknowledgement of the STOP intent latch or its refusal.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum StopsOnlyResultV1 {
    IntentLatched {},
    Refused { code: EnforcerError },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StopsOnlyAckV1 {
    kind: AckKindV1,
    #[serde(deserialize_with = "deserialize_version")]
    version: u32,
    result: StopsOnlyResultV1,
    #[serde(deserialize_with = "deserialize_hash")]
    proof: String,
}

/// A connection transcript with no key or runtime authority. The shared key
/// authenticates the selected role on the connection. A holder of that key
/// can still use the separate historical authentication.
pub(super) struct StopsOnlySessionV1 {
    epoch: Uuid,
    server_nonce: Uuid,
    client_nonce: Uuid,
    root_binding: String,
    request_hash: String,
}

impl StopsOnlyAuthenticationV1 {
    /// Prepare one new connection against the checked endpoint's binding.
    pub(super) fn new(
        key: &[u8; 32],
        challenge: &Challenge,
        root_binding: &str,
        request_hash: &str,
    ) -> EnforcerResult<(Self, StopsOnlySessionV1)> {
        let session =
            StopsOnlySessionV1::new(challenge, Uuid::new_v4(), root_binding, request_hash)?;
        let authentication = Self {
            kind: AuthenticationKindV1::AuthenticateLegacyStopsOnly,
            version: VERSION,
            client_nonce: session.client_nonce,
            root_binding: session.root_binding.clone(),
            request_hash: session.request_hash.clone(),
            client_proof: sign(key, CLIENT_DOMAIN, &session.fields())?,
        };
        Ok((authentication, session))
    }

    /// Verify the request against the guardian's own checked endpoint binding.
    pub(super) fn authenticate(
        &self,
        key: &[u8; 32],
        challenge: &Challenge,
        expected_root_binding: &str,
    ) -> EnforcerResult<StopsOnlySessionV1> {
        if self.root_binding != expected_root_binding {
            return Err(EnforcerError::Unauthorized);
        }
        let session = StopsOnlySessionV1::new(
            challenge,
            self.client_nonce,
            expected_root_binding,
            &self.request_hash,
        )?;
        verify(key, CLIENT_DOMAIN, &session.fields(), &self.client_proof)?;
        Ok(session)
    }
}

impl StopsOnlySessionV1 {
    fn new(
        challenge: &Challenge,
        client_nonce: Uuid,
        root_binding: &str,
        request_hash: &str,
    ) -> EnforcerResult<Self> {
        if challenge.epoch.is_nil()
            || challenge.nonce.is_nil()
            || client_nonce.is_nil()
            || !hex_hash(root_binding)
            || !hex_hash(request_hash)
        {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(Self {
            epoch: challenge.epoch,
            server_nonce: challenge.nonce,
            client_nonce,
            root_binding: root_binding.into(),
            request_hash: request_hash.into(),
        })
    }

    fn fields(&self) -> impl Serialize + '_ {
        (
            VERSION,
            ROLE,
            self.epoch,
            self.server_nonce,
            self.client_nonce,
            self.root_binding.as_str(),
            self.request_hash.as_str(),
        )
    }

    pub(super) fn grant(&self, key: &[u8; 32]) -> EnforcerResult<StopsOnlyGrantV1> {
        Ok(StopsOnlyGrantV1 {
            kind: GrantKindV1::GrantedLegacyStopsOnly,
            version: VERSION,
            server_proof: sign(key, GUARDIAN_DOMAIN, &self.fields())?,
        })
    }

    pub(super) fn verify_grant(
        &self,
        key: &[u8; 32],
        grant: &StopsOnlyGrantV1,
    ) -> EnforcerResult<()> {
        verify(key, GUARDIAN_DOMAIN, &self.fields(), &grant.server_proof)
    }

    pub(super) fn require_request(&self, facts: &impl Serialize) -> EnforcerResult<()> {
        if request_hash(facts)? != self.request_hash {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(())
    }

    pub(super) fn acknowledge(
        &self,
        key: &[u8; 32],
        result: StopsOnlyResultV1,
    ) -> EnforcerResult<StopsOnlyAckV1> {
        Ok(StopsOnlyAckV1 {
            kind: AckKindV1::LegacyStopsOnlyAck,
            version: VERSION,
            result,
            proof: sign(key, ACK_DOMAIN, &(self.fields(), result))?,
        })
    }

    pub(super) fn verify_acknowledgement(
        &self,
        key: &[u8; 32],
        ack: &StopsOnlyAckV1,
    ) -> EnforcerResult<StopsOnlyResultV1> {
        verify(key, ACK_DOMAIN, &(self.fields(), ack.result), &ack.proof)?;
        Ok(ack.result)
    }
}

/// Hash both the borrowed journal packet and its matching decoded form.
pub(super) fn request_hash(facts: &impl Serialize) -> EnforcerResult<String> {
    let bytes = canonical(facts).map_err(|_| EnforcerError::InvalidInput)?;
    let mut digest = Sha256::new();
    digest.update(REQUEST_DOMAIN);
    digest.update(bytes);
    Ok(hex::encode(digest.finalize()))
}

fn auth_mac(key: &[u8; 32], domain: &[u8], value: &impl Serialize) -> EnforcerResult<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| EnforcerError::InvalidInput)?;
    mac.update(domain);
    mac.update(&canonical(value).map_err(|_| EnforcerError::InvalidInput)?);
    Ok(mac)
}

fn sign(key: &[u8; 32], domain: &[u8], value: &impl Serialize) -> EnforcerResult<String> {
    Ok(hex::encode(
        auth_mac(key, domain, value)?.finalize().into_bytes(),
    ))
}

fn verify(
    key: &[u8; 32],
    domain: &[u8],
    value: &impl Serialize,
    proof: &str,
) -> EnforcerResult<()> {
    if !hex_hash(proof) {
        return Err(EnforcerError::Unauthorized);
    }
    let mut bytes = [0; 32];
    hex::decode_to_slice(proof, &mut bytes).map_err(|_| EnforcerError::Unauthorized)?;
    auth_mac(key, domain, value)?
        .verify_slice(&bytes)
        .map_err(|_| EnforcerError::Unauthorized)
}

fn deserialize_version<'de, D: Deserializer<'de>>(decoder: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(decoder)?;
    if version != VERSION {
        return Err(D::Error::custom("invalid private named STOP auth version"));
    }
    Ok(version)
}

fn deserialize_nonce<'de, D: Deserializer<'de>>(decoder: D) -> Result<Uuid, D::Error> {
    let invalid = || D::Error::custom("invalid private named STOP auth nonce");
    deserialize_legacy_optional_uuid(decoder)
        .map_err(|_| invalid())?
        .ok_or_else(invalid)
}

fn deserialize_hash<'de, D: Deserializer<'de>>(decoder: D) -> Result<String, D::Error> {
    let hash = String::deserialize(decoder)?;
    if !hex_hash(&hash) {
        return Err(D::Error::custom("invalid private named STOP auth hash"));
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge() -> Challenge {
        Challenge {
            epoch: Uuid::from_u128(1),
            nonce: Uuid::from_u128(2),
        }
    }

    #[test]
    fn first_authentication_preserves_legacy_and_closes_stops_only() -> anyhow::Result<()> {
        let challenge = challenge();
        assert_eq!(
            serde_json::to_string(&challenge)?,
            r#"{"epoch":"00000000-0000-0000-0000-000000000001","nonce":"00000000-0000-0000-0000-000000000002"}"#
        );
        let legacy = Authentication {
            proof: "f".repeat(64),
        };
        let legacy_json = serde_json::to_string(&legacy)?;
        assert_eq!(legacy_json, format!(r#"{{"proof":"{}"}}"#, legacy.proof));
        assert!(matches!(
            serde_json::from_str(&legacy_json)?,
            FirstAuthentication::Legacy(decoded) if decoded.proof == legacy.proof
        ));
        assert!(
            serde_json::from_str::<FirstAuthentication>(r#"{"proof":"a","proof":"b"}"#).is_err()
        );
        assert!(serde_json::from_str::<FirstAuthentication>(r#"["only_a_proof"]"#).is_err());

        let (auth, _) = StopsOnlyAuthenticationV1::new(
            &[0x5a; 32],
            &challenge,
            &"a".repeat(64),
            &"b".repeat(64),
        )?;
        let frame = serde_json::to_vec(&auth)?;
        assert!(frame.len() <= HANDSHAKE_FRAME as usize);
        assert!(matches!(
            serde_json::from_slice(&frame)?,
            FirstAuthentication::StopsOnly(_)
        ));
        let mut mixed = serde_json::to_value(auth)?;
        mixed["proof"] = serde_json::Value::String(legacy.proof);
        assert!(serde_json::from_value::<FirstAuthentication>(mixed).is_err());
        Ok(())
    }

    #[test]
    fn stops_only_proofs_bind_roles_epoch_filesystem_and_packet() -> anyhow::Result<()> {
        let key = [0x5a; 32];
        let challenge = challenge();
        let root = "a".repeat(64);
        let facts = serde_json::json!({"version": 1, "operation_id": Uuid::from_u128(3)});
        let (auth, client) =
            StopsOnlyAuthenticationV1::new(&key, &challenge, &root, &request_hash(&facts)?)?;
        let server = auth.authenticate(&key, &challenge, &root)?;
        let mut grant = server.grant(&key)?;
        client.verify_grant(&key, &grant)?;
        server.require_request(&facts)?;
        let mut ack = server.acknowledge(&key, StopsOnlyResultV1::IntentLatched {})?;
        assert!(matches!(
            client.verify_acknowledgement(&key, &ack)?,
            StopsOnlyResultV1::IntentLatched {}
        ));

        grant.server_proof = auth.client_proof.clone();
        assert!(client.verify_grant(&key, &grant).is_err());
        ack.result = StopsOnlyResultV1::Refused {
            code: EnforcerError::NotReady,
        };
        assert!(client.verify_acknowledgement(&key, &ack).is_err());
        let other_epoch = Challenge {
            epoch: Uuid::from_u128(4),
            nonce: challenge.nonce,
        };
        assert!(auth.authenticate(&key, &other_epoch, &root).is_err());
        assert!(
            auth.authenticate(&key, &challenge, &"c".repeat(64))
                .is_err()
        );
        let changed = serde_json::json!({"version": 1, "operation_id": Uuid::from_u128(5)});
        assert!(server.require_request(&changed).is_err());
        Ok(())
    }
}
