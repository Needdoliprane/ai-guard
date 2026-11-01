use super::JevError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use tokio_rustls::rustls::pki_types::CertificateDer;

#[derive(Clone, Default)]
pub struct JevTrust(Option<Arc<ExplicitTrust>>);
struct ExplicitTrust {
    certificates: Vec<CertificateDer<'static>>,
    digest: String,
}
impl JevTrust {
    /// Only certificates and whitespace are accepted. No private key, skipped
    /// PEM section, ambient OS trust or network lookup is involved.
    pub fn from_ca_pem(bytes: &[u8]) -> Result<Self, JevError> {
        let certificates =
            crate::tls::certificates(bytes).map_err(|_| JevError::InvalidConfiguration)?;
        Ok(Self(Some(Arc::new(ExplicitTrust {
            certificates,
            digest: hex::encode(Sha256::digest(bytes)),
        }))))
    }
    pub fn from_ca_file(path: &Path) -> Result<Self, JevError> {
        let bytes =
            crate::tls::read_material(path, 65_536).map_err(|_| JevError::InvalidConfiguration)?;
        Self::from_ca_pem(&bytes)
    }
    /// Apply this trust to a separately bounded diagnostic client. Explicit
    /// trust replaces built-in roots; the default preserves reqwest's WebPKI.
    pub fn apply_to_client(
        &self,
        mut builder: reqwest::ClientBuilder,
    ) -> Result<reqwest::ClientBuilder, JevError> {
        if let Some(trust) = &self.0 {
            builder = builder.tls_built_in_root_certs(false);
            for certificate in &trust.certificates {
                builder = builder.add_root_certificate(
                    reqwest::Certificate::from_der(certificate.as_ref())
                        .map_err(|_| JevError::InvalidConfiguration)?,
                );
            }
        }
        Ok(builder)
    }
    pub(super) fn is_explicit(&self) -> bool {
        self.0.is_some()
    }
    pub(super) fn configuration(&self) -> Value {
        match &self.0 {
            Some(trust) => {
                json!({"mode":"explicit_ca_bundle","ca_bundle_sha256":trust.digest,"certificate_count":trust.certificates.len()})
            }
            None => {
                json!({"mode":"embedded_webpki","ca_bundle_sha256":null,"certificate_count":null})
            }
        }
    }
}
