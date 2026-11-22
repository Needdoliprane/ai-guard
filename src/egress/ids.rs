//! Closed identifiers carried by the broker-owned egress channel.
//!
//! These are data, never identity proofs: the broker assigns them from its own
//! retained context and compares every echoed value against that context.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Exact channel contract named in the worker handshake and launch profile.
/// It is never selected from a historical relay token or frame.
pub const CHANNEL_CONTRACT: &str = "broker_proxy_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdError;
impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("egress identifier is not canonical")
    }
}
impl std::error::Error for IdError {}

/// Strict, nonnil lower-case hyphenated UUID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdV1(String);
impl IdV1 {
    pub fn new(id: uuid::Uuid) -> Result<Self, IdError> {
        Self::parse(&id.to_string())
    }
    pub fn random() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    pub fn parse(raw: &str) -> Result<Self, IdError> {
        let id = uuid::Uuid::parse_str(raw).map_err(|_| IdError)?;
        if id.is_nil() || id.to_string() != raw {
            return Err(IdError);
        }
        Ok(Self(raw.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for IdV1 {
    type Error = IdError;
    fn try_from(raw: String) -> Result<Self, IdError> {
        Self::parse(&raw)
    }
}
impl From<IdV1> for String {
    fn from(id: IdV1) -> Self {
        id.0
    }
}

/// Lower-case hexadecimal SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DigestV1(String);
impl DigestV1 {
    pub fn parse(raw: &str) -> Result<Self, IdError> {
        if raw.len() != 64
            || !raw
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(IdError);
        }
        Ok(Self(raw.into()))
    }
    pub fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for DigestV1 {
    type Error = IdError;
    fn try_from(raw: String) -> Result<Self, IdError> {
        Self::parse(&raw)
    }
}
impl From<DigestV1> for String {
    fn from(digest: DigestV1) -> Self {
        digest.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_ids_accept_only_canonical_nonnil_uuids_and_lowercase_digests() {
        let raw = "00000000-0000-4000-8000-000000000001";
        assert_eq!(IdV1::parse(raw).unwrap().as_str(), raw);
        for bad in [
            "",
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-4000-8000-00000000000A",
            "{00000000-0000-4000-8000-000000000001}",
            "00000000000040008000000000000001",
        ] {
            assert_eq!(IdV1::parse(bad), Err(IdError), "{bad}");
        }
        assert!(serde_json::from_str::<IdV1>("\"00000000-0000-0000-0000-000000000000\"").is_err());
        let digest = DigestV1::of(b"abc");
        assert_eq!(
            digest.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(DigestV1::parse(&digest.as_str().to_uppercase()).is_err());
        assert!(DigestV1::parse(&digest.as_str()[..63]).is_err());
    }
}
