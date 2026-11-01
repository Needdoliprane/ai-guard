//! Explicit local TLS material provisioning. Secret packets never implement
//! Debug and never enter command arguments, environment, receipts or errors.
use crate::control_fs::{Directory, PrivateFile, absolute_path, publication::PublicationFailure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    os::fd::{AsRawFd, FromRawFd},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_rustls::rustls::{
    self,
    pki_types::{PrivateKeyDer, ServerName, UnixTime, pem::PemObject},
    server::ParsedCertificate,
};
use uuid::Uuid;

pub mod cli;
pub use cli::{ModelTlsArgs, execute};
#[cfg(test)]
mod tests;
const MAX_WIRE: usize = 512 * 1024;
const MAX_CERT: usize = 65_536;
const MAX_KEY: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    InvalidInput,
    Storage,
    Runtime,
    TimedOut,
    Interrupted,
    PublicationUnknown,
    LocalProcessUnknown,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid TLS material, server name or packet",
            Self::Storage => "private TLS storage validation or synchronization failed",
            Self::Runtime => "TLS preparer runtime operation failed",
            Self::TimedOut => "TLS provisioning orchestration deadline expired",
            Self::Interrupted => "TLS provisioning interrupted",
            Self::PublicationUnknown => "TLS configuration publication/durability is unknown; do not activate the retained destination",
            Self::LocalProcessUnknown => "local TLS runtime transport termination is unconfirmed",
        })
    }
}
impl std::error::Error for Failure {}
type Result<T> = std::result::Result<T, Failure>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PacketV1 {
    version: u32,
    generation: String,
    server_name: String,
    ca_pem: String,
    cert_pem: String,
    key_pem: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AckV1 {
    pub version: u32,
    pub generation: String,
    pub server_name: String,
    pub ca_certificates_sha256: Vec<String>,
    pub server_certificates_sha256: Vec<String>,
}
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 253
        || !name.is_ascii()
        || name == "localhost"
        || [
            "model",
            "gateway",
            "dashboard",
            "ingress",
            "local-ai-ingress",
        ]
        .contains(&name)
        || name.ends_with(".localhost")
        || name.parse::<std::net::IpAddr>().is_ok()
        || name.split('.').any(|part| {
            part.is_empty()
                || part.len() > 63
                || part.starts_with('-')
                || part.ends_with('-')
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(Failure::InvalidInput);
    }
    Ok(())
}
fn generation(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| Failure::InvalidInput)?;
    if id.is_nil() || id.to_string() != value {
        return Err(Failure::InvalidInput);
    }
    Ok(id)
}
pub(crate) fn decode_packet(bytes: &[u8]) -> Result<PacketV1> {
    if bytes.is_empty() || bytes.len() > MAX_WIRE {
        return Err(Failure::InvalidInput);
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let packet = PacketV1::deserialize(&mut decoder).map_err(|_| Failure::InvalidInput)?;
    decoder.end().map_err(|_| Failure::InvalidInput)?;
    Ok(packet)
}
pub(crate) fn validate_packet(packet: &PacketV1) -> Result<AckV1> {
    if packet.version != 1
        || packet.ca_pem.len() > MAX_CERT
        || packet.cert_pem.len() > MAX_CERT
        || packet.key_pem.len() > MAX_KEY
    {
        return Err(Failure::InvalidInput);
    }
    generation(&packet.generation)?;
    validate_name(&packet.server_name)?;
    // This first pass enforces the shared strict PEM grammar for BOTH chain
    // and key. The key is decoded again only after that framing is accepted.
    crate::tls::ServerIdentity::from_pem(packet.cert_pem.as_bytes(), packet.key_pem.as_bytes())
        .map_err(|_| Failure::InvalidInput)?;
    let ca =
        crate::tls::certificates(packet.ca_pem.as_bytes()).map_err(|_| Failure::InvalidInput)?;
    let chain =
        crate::tls::certificates(packet.cert_pem.as_bytes()).map_err(|_| Failure::InvalidInput)?;
    let leaf = chain.first().ok_or(Failure::InvalidInput)?;
    let parsed = ParsedCertificate::try_from(leaf).map_err(|_| Failure::InvalidInput)?;
    let provider = rustls::crypto::ring::default_provider();
    let key = PrivateKeyDer::from_pem_slice(packet.key_pem.as_bytes())
        .map_err(|_| Failure::InvalidInput)?;
    rustls::sign::CertifiedKey::from_der(chain.clone(), key, &provider)
        .map_err(|_| Failure::InvalidInput)?
        .keys_match()
        .map_err(|_| Failure::InvalidInput)?;
    let mut roots = rustls::RootCertStore::empty();
    for certificate in &ca {
        roots
            .add(certificate.clone())
            .map_err(|_| Failure::InvalidInput)?;
    }
    rustls::client::verify_server_cert_signed_by_trust_anchor(
        &parsed,
        &roots,
        &chain[1..],
        UnixTime::now(),
        provider.signature_verification_algorithms.all,
    )
    .map_err(|_| Failure::InvalidInput)?;
    rustls::client::verify_server_name(
        &parsed,
        &ServerName::try_from(packet.server_name.as_str()).map_err(|_| Failure::InvalidInput)?,
    )
    .map_err(|_| Failure::InvalidInput)?;
    let leaf_spki = parsed.subject_public_key_info();
    for certificate in ca.iter().chain(chain.iter().skip(1)) {
        if ParsedCertificate::try_from(certificate)
            .map_err(|_| Failure::InvalidInput)?
            .subject_public_key_info()
            == leaf_spki
        {
            return Err(Failure::InvalidInput);
        }
    }
    let fingerprints = |certificates: &[rustls::pki_types::CertificateDer<'_>]| {
        certificates
            .iter()
            .map(|certificate| hex::encode(Sha256::digest(certificate.as_ref())))
            .collect()
    };
    Ok(AckV1 {
        version: 1,
        generation: packet.generation.clone(),
        server_name: packet.server_name.clone(),
        ca_certificates_sha256: fingerprints(&ca),
        server_certificates_sha256: fingerprints(&chain),
    })
}

pub(crate) struct Sources {
    parents: Vec<Directory>,
    files: Vec<PrivateFile>,
    pub packet: PacketV1,
    pub expected: AckV1,
}
impl Sources {
    pub fn open(ca: &Path, cert: &Path, key: &Path, name: String) -> Result<Self> {
        validate_name(&name)?;
        let mut parents = Vec::new();
        let mut files = Vec::new();
        for (path, bound) in [(ca, MAX_CERT), (cert, MAX_CERT), (key, MAX_KEY)] {
            let path = absolute_path(path).map_err(|_| Failure::Storage)?;
            parents.push(
                Directory::open_existing(path.parent().ok_or(Failure::Storage)?)
                    .map_err(|_| Failure::Storage)?,
            );
            files.push(PrivateFile::open(&path, bound as u64).map_err(|_| Failure::Storage)?);
        }
        let text = |index: usize| {
            String::from_utf8(files[index].bytes().to_vec()).map_err(|_| Failure::InvalidInput)
        };
        let packet = PacketV1 {
            version: 1,
            generation: Uuid::new_v4().to_string(),
            server_name: name,
            ca_pem: text(0)?,
            cert_pem: text(1)?,
            key_pem: text(2)?,
        };
        let expected = validate_packet(&packet)?;
        let sources = Self {
            parents,
            files,
            packet,
            expected,
        };
        sources.revalidate()?;
        Ok(sources)
    }
    pub fn revalidate(&self) -> Result<()> {
        for parent in &self.parents {
            parent.revalidate().map_err(|_| Failure::Storage)?;
        }
        for file in &self.files {
            file.revalidate().map_err(|_| Failure::Storage)?;
        }
        Ok(())
    }
    pub fn wire(&self) -> Result<Vec<u8>> {
        self.revalidate()?;
        let bytes = serde_json::to_vec(&self.packet).map_err(|_| Failure::InvalidInput)?;
        if bytes.len() > MAX_WIRE {
            return Err(Failure::InvalidInput);
        }
        Ok(bytes)
    }
}

fn empty(directory: &Directory) -> Result<()> {
    let inventory = directory
        .persistent_inventory(1)
        .map_err(|_| Failure::Storage)?;
    if inventory.visited != 0
        || !inventory.entries.is_empty()
        || inventory.rejected != 0
        || inventory.incomplete
    {
        return Err(Failure::Storage);
    }
    Ok(())
}
pub(crate) fn import_into(packet: PacketV1, ca: &Directory, identity: &Directory) -> Result<AckV1> {
    use std::os::unix::fs::MetadataExt;
    let expected = validate_packet(&packet)?;
    empty(ca)?;
    empty(identity)?;
    let a = ca
        .try_clone_file()
        .map_err(|_| Failure::Storage)?
        .metadata()
        .map_err(|_| Failure::Storage)?;
    let b = identity
        .try_clone_file()
        .map_err(|_| Failure::Storage)?
        .metadata()
        .map_err(|_| Failure::Storage)?;
    if (a.dev(), a.ino()) == (b.dev(), b.ino()) {
        return Err(Failure::Storage);
    }
    for (directory, name, bytes) in [
        (ca, "root-ca.pem", packet.ca_pem.as_bytes()),
        (identity, "server-chain.pem", packet.cert_pem.as_bytes()),
        (identity, "server.key", packet.key_pem.as_bytes()),
    ] {
        directory
            .persistent_create(name, bytes)
            .and_then(|file| file.persistent_sync())
            .map_err(|_| Failure::Storage)?;
    }
    ca.sync().map_err(|_| Failure::Storage)?;
    identity.sync().map_err(|_| Failure::Storage)?;
    let read = |directory: &Directory, name: &str, bound: usize| {
        directory
            .read_private_file(name, bound as u64)
            .map_err(|_| Failure::Storage)
            .and_then(|file| String::from_utf8(file.bytes().to_vec()).map_err(|_| Failure::Storage))
    };
    let observed = PacketV1 {
        version: 1,
        generation: packet.generation,
        server_name: packet.server_name,
        ca_pem: read(ca, "root-ca.pem", MAX_CERT)?,
        cert_pem: read(identity, "server-chain.pem", MAX_CERT)?,
        key_pem: read(identity, "server.key", MAX_KEY)?,
    };
    if validate_packet(&observed)? != expected {
        return Err(Failure::Storage);
    }
    // Exercise the actual consumer readers too, not merely an equivalent
    // private-file read. Unsupported projected filesystems still fail closed.
    crate::tls::ServerIdentity::from_files(
        &identity.path().join("server-chain.pem"),
        &identity.path().join("server.key"),
    )
    .map_err(|_| Failure::Storage)?;
    crate::jev::JevTrust::from_ca_file(&ca.path().join("root-ca.pem"))
        .map_err(|_| Failure::Storage)?;
    ca.revalidate()
        .and_then(|_| identity.revalidate())
        .map_err(|_| Failure::Storage)?;
    Ok(expected)
}

// stdin/stdout are pipes owned by the hidden importer. AsyncFd prevents a
// cancelled Tokio stdin read from retaining a blocking pool thread forever.
fn pipe(fd: i32) -> Result<tokio::io::unix::AsyncFd<std::fs::File>> {
    // SAFETY: duplicate the explicitly owned internal stdio descriptor. No
    // other thread reads/writes these pipes in this hidden command.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(Failure::InvalidInput);
    }
    // SAFETY: successful F_DUPFD transfers one newly owned descriptor.
    let file = unsafe { std::fs::File::from_raw_fd(duplicate) };
    // SAFETY: F_GETFL/F_SETFL operate on that live owned pipe descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(Failure::InvalidInput);
    }
    // SAFETY: File owns the valid duplicated FD, with O_NONBLOCK established
    // above. AsyncFd owns that File until deregistration; neither consumer
    // replaces, closes or extracts it. receive_stdin/write_ack only borrow it
    // for nonblocking IO and retain AsyncFd through each operation. Both call
    // this helper inside the active Tokio runtime.
    unsafe { tokio::io::unix::AsyncFd::register(file) }.map_err(|_| Failure::InvalidInput)
}
async fn receive_stdin() -> Result<Vec<u8>> {
    let pipe = pipe(0)?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let mut ready = pipe.readable().await.map_err(|_| Failure::InvalidInput)?;
        match ready.try_io(|handle| {
            use std::io::Read;
            let mut file = handle.get_ref();
            file.read(&mut buffer)
        }) {
            Err(_) => continue,
            Ok(Err(_)) => return Err(Failure::InvalidInput),
            Ok(Ok(0)) => return Ok(bytes),
            Ok(Ok(count)) => {
                if count > MAX_WIRE - bytes.len() {
                    return Err(Failure::InvalidInput);
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
        }
    }
}
async fn write_ack(bytes: &[u8]) -> Result<()> {
    let pipe = pipe(1)?;
    let mut offset = 0;
    while offset < bytes.len() {
        let mut ready = pipe.writable().await.map_err(|_| Failure::Storage)?;
        match ready.try_io(|handle| {
            use std::io::Write;
            let mut file = handle.get_ref();
            file.write(&bytes[offset..])
        }) {
            Err(_) => continue,
            Ok(Ok(count)) if count > 0 => offset += count,
            _ => return Err(Failure::Storage),
        }
    }
    Ok(())
}
pub async fn internal_import() -> Result<()> {
    let bytes = tokio::time::timeout(Duration::from_secs(15), receive_stdin())
        .await
        .map_err(|_| Failure::TimedOut)??;
    let packet = decode_packet(&bytes)?;
    validate_packet(&packet)?;
    let ca = Directory::open_existing(Path::new("/data")).map_err(|_| Failure::Storage)?;
    let identity =
        Directory::open_existing(Path::new("/run/secrets")).map_err(|_| Failure::Storage)?;
    let ack = import_into(packet, &ca, &identity)?;
    let bytes = serde_json::to_vec(&ack).map_err(|_| Failure::Storage)?;
    if bytes.len() > 4096 {
        return Err(Failure::Storage);
    }
    tokio::time::timeout(Duration::from_secs(15), write_ack(&bytes))
        .await
        .map_err(|_| Failure::TimedOut)??;
    Ok(())
}

pub(crate) fn decode_ack(bytes: &[u8], expected: &AckV1) -> Result<()> {
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err(Failure::Runtime);
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let observed = AckV1::deserialize(&mut decoder).map_err(|_| Failure::Runtime)?;
    decoder.end().map_err(|_| Failure::Runtime)?;
    if &observed != expected {
        return Err(Failure::Runtime);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VolumeSelection {
    pub ca: String,
    pub identity: String,
    pub network: String,
}
impl VolumeSelection {
    pub fn new(generation: &str) -> Self {
        Self {
            ca: format!("ai-guard-model-tls-{generation}-ca"),
            identity: format!("ai-guard-model-tls-{generation}-identity"),
            network: format!("ai-guard-judge-{generation}"),
        }
    }
}
#[derive(Serialize)]
struct ManifestV1<'a> {
    version: u32,
    ack: &'a AckV1,
    image: &'a str,
    volumes: &'a VolumeSelection,
    runtime: &'a crate::runtime::model_tls::RuntimeProvenance,
}
pub(crate) struct OutputPlan {
    output: PathBuf,
    parent: Directory,
    staged: Directory,
    name: String,
}
impl OutputPlan {
    pub fn prepare(output: &Path) -> Result<Self> {
        let output = absolute_path(output).map_err(|_| Failure::Storage)?;
        let parent = Directory::open_existing(output.parent().ok_or(Failure::Storage)?)
            .map_err(|_| Failure::Storage)?;
        let name = output
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.chars().any(char::is_control))
            .ok_or(Failure::Storage)?
            .to_owned();
        parent
            .require_absent_child(&name)
            .map_err(|_| Failure::Storage)?;
        let staged = parent
            .exclusive_child(&format!(".model-tls-staging-{}", Uuid::new_v4()))
            .map_err(|_| Failure::Storage)?;
        Ok(Self {
            output,
            parent,
            staged,
            name,
        })
    }
}
pub(crate) fn publish(
    plan: &OutputPlan,
    ack: &AckV1,
    image: &str,
    volumes: &VolumeSelection,
    runtime: &crate::runtime::model_tls::RuntimeProvenance,
) -> Result<PathBuf> {
    let OutputPlan {
        parent,
        staged,
        name,
        ..
    } = plan;
    parent.revalidate().map_err(|_| Failure::Storage)?;
    parent
        .require_absent_child(name)
        .map_err(|_| Failure::Storage)?;
    let manifest = serde_json::to_vec_pretty(&ManifestV1 {
        version: 1,
        ack,
        image,
        volumes,
        runtime,
    })
    .map_err(|_| Failure::Storage)?;
    let env = format!(
        "AI_GUARD_LOCAL_AI_TLS_CA_VOLUME={}\nAI_GUARD_LOCAL_AI_TLS_IDENTITY_VOLUME={}\nAI_GUARD_LOCAL_AI_TLS_SERVER_NAME={}\nAI_GUARD_LOCAL_AI_TLS_NETWORK={}\nAI_GUARD_CONTROL_IMAGE={}\n",
        volumes.ca, volumes.identity, ack.server_name, volumes.network, image
    );
    for (name, bytes) in [
        ("manifest.json", manifest.as_slice()),
        ("volumes.env", env.as_bytes()),
    ] {
        staged
            .persistent_create(name, bytes)
            .and_then(|file| file.persistent_sync())
            .map_err(|_| Failure::Storage)?;
        #[cfg(test)]
        if name == "manifest.json" {
            tests::after_manifest()?;
        }
    }
    staged.sync().map_err(|_| Failure::Storage)?;
    parent
        .publish_child_exclusive(staged, name)
        .map_err(|failure| match failure {
            PublicationFailure::NotPublished => Failure::Storage,
            PublicationFailure::DurabilityUnknown => Failure::PublicationUnknown,
        })?;
    Ok(plan.output.clone())
}
