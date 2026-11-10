//! Ed25519 signing keys of the control and their public documents.
//!
//! The private key is one PKCS#8 v2 file (mode 0600) in a private directory
//! (mode 0700) of the control account. It is protected by filesystem
//! permissions only: no passphrase, hardware module or guaranteed memory
//! erasure. It is never serialized anywhere else, and no type that holds it
//! can be printed or serialized.

use super::{
    MAX_PUBLIC_KEY_BYTES, PUBLIC_KEY_FORMAT, SIGNATURE_ALGORITHM, SIGNATURE_DOMAIN, files,
    format::{RecordSignature, decode_base64url, encode_base64url, is_sha256_hex, sha256_hex},
    report::{Code, Failure},
};
use crate::control_fs::Directory;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const KEY_FILE: &str = "signing-key.pk8";
const MAX_KEY_BYTES: u64 = 256;
const PUBLIC_KEY_BYTES: usize = 32;

/// A loaded signing key. Deliberately neither `Serialize` nor `Clone`; its
/// `Debug` output names the public fingerprint only.
pub(crate) struct SigningKey {
    pair: Ed25519KeyPair,
    key_id: String,
    public_key: [u8; PUBLIC_KEY_BYTES],
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "SigningKey {{ key_id: {}, private_key: <redacted> }}",
            self.key_id
        )
    }
}

fn signed_message(statement_bytes: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNATURE_DOMAIN.len() + statement_bytes.len());
    message.extend_from_slice(SIGNATURE_DOMAIN);
    message.extend_from_slice(statement_bytes);
    message
}

impl SigningKey {
    pub(crate) fn from_pkcs8(bytes: &[u8]) -> Result<Self, Failure> {
        let invalid = || {
            Failure::new(
                Code::KeyInvalid,
                "the signing key file is not a usable Ed25519 PKCS#8 v2 document",
            )
        };
        let pair = Ed25519KeyPair::from_pkcs8(bytes).map_err(|_| invalid())?;
        let public_key: [u8; PUBLIC_KEY_BYTES] = pair
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| invalid())?;
        Ok(Self {
            key_id: sha256_hex(&public_key),
            pair,
            public_key,
        })
    }

    pub(crate) fn key_id(&self) -> &str {
        &self.key_id
    }

    pub(crate) fn public_key(&self) -> &[u8; PUBLIC_KEY_BYTES] {
        &self.public_key
    }

    pub(crate) fn public_document(&self) -> PublicKeyDocument {
        PublicKeyDocument {
            format: PUBLIC_KEY_FORMAT.into(),
            algorithm: SIGNATURE_ALGORITHM.into(),
            key_id: self.key_id.clone(),
            public_key: encode_base64url(&self.public_key),
        }
    }

    /// Sign the exact statement bytes under the domain prefix.
    pub(crate) fn sign(&self, statement_bytes: &[u8]) -> RecordSignature {
        let signature = self.pair.sign(&signed_message(statement_bytes));
        RecordSignature {
            key_id: self.key_id.clone(),
            signature: encode_base64url(signature.as_ref()),
        }
    }
}

/// Verify one signature over the exact statement bytes with a public key that
/// the caller obtained from a trust decision, never from the signed material.
pub(crate) fn verify_signature(
    public_key: &[u8; PUBLIC_KEY_BYTES],
    statement_bytes: &[u8],
    signature: &str,
) -> bool {
    let Some(signature) = decode_base64url(signature, 64) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&signed_message(statement_bytes), &signature)
        .is_ok()
}

/// Public half of a signing key. Handing this document over approves nothing:
/// the anchor holder compares `key_id` with a fingerprint received through a
/// separate channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicKeyDocument {
    pub format: String,
    pub algorithm: String,
    /// Lowercase hexadecimal SHA-256 of the raw 32-byte public key.
    pub key_id: String,
    /// Unpadded base64url of the raw 32-byte public key.
    pub public_key: String,
}

impl PublicKeyDocument {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, Failure> {
        let invalid = |message: String| {
            Failure::new(Code::InputUnavailable, message).with("input", "public_key")
        };
        crate::json::decode_unique(bytes, 4)
            .map_err(|error| invalid(format!("public key document is not strict JSON: {error}")))?;
        let document: Self = serde_json::from_slice(bytes)
            .map_err(|error| invalid(format!("public key document is invalid: {error}")))?;
        if document.format != PUBLIC_KEY_FORMAT || document.algorithm != SIGNATURE_ALGORITHM {
            return Err(invalid(
                "public key document format or algorithm is not supported".into(),
            ));
        }
        let public_key = document.public_key_bytes()?;
        if !is_sha256_hex(&document.key_id) || document.key_id != sha256_hex(&public_key) {
            return Err(Failure::new(
                Code::KeyIdMismatch,
                "the public key document names a fingerprint that is not the SHA-256 of its key",
            )
            .with("between", vec!["public_key_document.key_id", "public_key"]));
        }
        Ok(document)
    }

    pub(crate) fn public_key_bytes(&self) -> Result<[u8; PUBLIC_KEY_BYTES], Failure> {
        decode_base64url(&self.public_key, PUBLIC_KEY_BYTES)
            .and_then(|bytes| <[u8; PUBLIC_KEY_BYTES]>::try_from(bytes).ok())
            .ok_or_else(|| {
                Failure::new(
                    Code::InputUnavailable,
                    "public key is not canonical unpadded base64url of 32 bytes",
                )
                .with("input", "public_key")
            })
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, Failure> {
        let mut bytes = serde_json::to_vec(self).map_err(|_| {
            Failure::new(Code::IoFailure, "public key document cannot be serialized")
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// Result of `key init` and `key show`. It carries public material only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyOutcome {
    pub key_id: String,
    pub algorithm: String,
    pub public_key: String,
    pub key_dir: String,
    /// Path of the public key document written by this call, if any.
    pub public_key_file: Option<String>,
}

fn unavailable(error: &anyhow::Error) -> Failure {
    Failure::new(
        Code::KeyUnavailable,
        "the signing key directory or file does not satisfy the private storage policy",
    )
    .cause(error)
}

pub(crate) fn load_signing_key(key_dir: &Path) -> Result<SigningKey, Failure> {
    let path = files::absolute(key_dir)?;
    let directory = Directory::open_existing(&path).map_err(|error| unavailable(&error))?;
    let file = directory
        .persistent_read(KEY_FILE, MAX_KEY_BYTES, &[])
        .map_err(|error| unavailable(&error))?
        .ok_or_else(|| {
            Failure::new(
                Code::KeyUnavailable,
                "the key directory holds no signing-key.pk8",
            )
            .with("key_dir", path.display().to_string())
        })?;
    SigningKey::from_pkcs8(file.bytes())
}

fn outcome(key: &SigningKey, key_dir: &Path, public_key_file: Option<String>) -> KeyOutcome {
    let document = key.public_document();
    KeyOutcome {
        key_id: document.key_id,
        algorithm: document.algorithm,
        public_key: document.public_key,
        key_dir: key_dir.display().to_string(),
        public_key_file,
    }
}

/// Create a signing key in a new private directory. An existing path is never
/// reused: after an interrupted initialization, choose a new directory.
pub fn init_key(key_dir: &Path) -> Result<KeyOutcome, Failure> {
    let path = files::absolute(key_dir)?;
    let exists = || {
        Failure::new(Code::KeyDirectoryExists, "the key directory already exists")
            .with("key_dir", path.display().to_string())
    };
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return Err(exists()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(unavailable(&error.into())),
    }
    let directory = Directory::open_or_create(&path).map_err(|error| unavailable(&error))?;
    directory.durable_bootstrap().map_err(|error| {
        files::io_failure("the new key directory could not be made durable", &error)
    })?;
    let random = ring::rand::SystemRandom::new();
    let document = Ed25519KeyPair::generate_pkcs8(&random).map_err(|_| {
        Failure::new(
            Code::IoFailure,
            "the system random generator is unavailable",
        )
    })?;
    let key = SigningKey::from_pkcs8(document.as_ref())?;
    let file = directory
        .persistent_create(KEY_FILE, document.as_ref())
        .map_err(|error| {
            if files::already_exists(&error) {
                exists()
            } else {
                files::io_failure("the signing key file could not be created", &error)
            }
        })?;
    file.persistent_sync()
        .and_then(|()| directory.sync())
        .map_err(|error| files::io_failure("the signing key could not be made durable", &error))?;
    drop(file);
    let reloaded = load_signing_key(&path)?;
    if reloaded.key_id() != key.key_id() {
        return Err(Failure::new(
            Code::IoFailure,
            "the signing key read back from disk is not the generated key",
        ));
    }
    Ok(outcome(&reloaded, &path, None))
}

/// Show the public half of an existing key and optionally write its public
/// document to a new file.
pub fn show_key(key_dir: &Path, out: Option<&Path>) -> Result<KeyOutcome, Failure> {
    let path = files::absolute(key_dir)?;
    let key = load_signing_key(&path)?;
    let written = match out {
        Some(out) => Some(
            files::write_new(
                out,
                &key.public_document().to_bytes()?,
                MAX_PUBLIC_KEY_BYTES,
            )?
            .display()
            .to_string(),
        ),
        None => None,
    };
    Ok(outcome(&key, &path, written))
}

/// Read a public key document handed over by the operator.
pub(crate) fn read_public_key(path: &Path) -> Result<PublicKeyDocument, Failure> {
    PublicKeyDocument::parse(&files::read_input(
        path,
        MAX_PUBLIC_KEY_BYTES,
        Code::InputUnavailable,
        "public_key",
    )?)
}
