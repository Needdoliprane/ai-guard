//! Explicit gateway trust is loaded once and is independent of provider trust.
use crate::tls::ClientTrust;
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn load(origin: &reqwest::Url, path: Option<&Path>) -> Result<ClientTrust> {
    let Some(path) = path else {
        return Ok(ClientTrust::default());
    };
    ensure!(
        origin.scheme() == "https",
        "--gateway-ca-file requires an HTTPS gateway origin"
    );
    ClientTrust::from_ca_file(path)
        .map_err(|_| anyhow::anyhow!("gateway CA configuration is invalid or unavailable"))
}

/// Preserve every legacy/default digest byte. An explicit bundle binds its
/// exact bytes to this credential/admission without changing the wire schema.
pub(super) fn bind_digest(trust: &ClientTrust, credential_digest: String) -> String {
    let Some(trust_digest) = trust.digest() else {
        return credential_digest;
    };
    let mut digest = Sha256::new();
    digest.update(b"ai-guard/human-gateway-trust/v1\0");
    digest.update(credential_digest.as_bytes());
    digest.update(b"\0");
    digest.update(trust_digest.as_bytes());
    hex::encode(digest.finalize())
}
