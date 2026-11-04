use super::{Result, WorkloadCliError as E};
use crate::{
    control_fs::{PrivateFile, absolute_path},
    workload::{WorkloadBindingIssueV1, WorkloadBindingRevocationV1, WorkloadSubjectV1},
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    net::IpAddr,
    os::fd::{AsFd, AsRawFd},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(super) const MAX_INPUT: u64 = 16_384;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Issue,
    Rotate,
    Revoke,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Rotate => "rotate",
            Self::Revoke => "revoke",
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Manifest {
    Issue {
        version: u32,
        expected_revision: u64,
        subject: WorkloadSubjectV1,
        expires_at: String,
    },
    Rotate {
        version: u32,
        credential_id: String,
        expected_revision: u64,
        subject: WorkloadSubjectV1,
        expires_at: String,
    },
    Revoke {
        version: u32,
        credential_id: String,
        expected_revision: u64,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Request {
    Issue(WorkloadBindingIssueV1),
    Revoke(WorkloadBindingRevocationV1),
}

impl Request {
    pub fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|_| E::UnsafeState)
    }
    pub fn digest(&self) -> Result<String> {
        Ok(hex::encode(Sha256::digest(self.bytes()?)))
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Issue(v) => &v.credential_id,
            Self::Revoke(v) => &v.credential_id,
        }
    }
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Issue(v) => &v.operation_id,
            Self::Revoke(v) => &v.operation_id,
        }
    }
    pub fn revision(&self) -> u64 {
        match self {
            Self::Issue(v) => v.expected_revision,
            Self::Revoke(v) => v.expected_revision,
        }
    }
    pub fn validate(&self, action: Action) -> Result<()> {
        match (self, action) {
            (Self::Issue(v), Action::Issue) if v.expected_revision == 0 => v.validate(),
            (Self::Issue(v), Action::Rotate) if v.expected_revision > 0 => v.validate(),
            (Self::Revoke(v), Action::Revoke) => v.validate(),
            _ => return Err(E::UnsafeState),
        }
        .map_err(|_| E::UnsafeState)
    }
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = if path == Path::new("-") {
            read_stdin()?
        } else {
            let source = PrivateFile::open(
                &absolute_path(path).map_err(|_| E::InvalidInput)?,
                MAX_INPUT,
            )
            .map_err(|_| E::InvalidInput)?;
            source.revalidate().map_err(|_| E::InvalidInput)?;
            source.bytes().to_vec()
        };
        serde_json::from_slice(&bytes).map_err(|_| E::InvalidInput)
    }

    pub fn prepare(self) -> Result<(Action, Request, Option<String>)> {
        let (action, request, secret) = match self {
            Self::Revoke {
                version,
                credential_id,
                expected_revision,
            } => (
                Action::Revoke,
                Request::Revoke(WorkloadBindingRevocationV1 {
                    version,
                    operation_id: Uuid::new_v4().to_string(),
                    credential_id,
                    expected_revision,
                }),
                None,
            ),
            value => {
                let (action, version, credential_id, expected_revision, subject, expires_at) =
                    match value {
                        Self::Issue {
                            version,
                            expected_revision,
                            subject,
                            expires_at,
                        } => (
                            Action::Issue,
                            version,
                            Uuid::new_v4().to_string(),
                            expected_revision,
                            subject,
                            expires_at,
                        ),
                        Self::Rotate {
                            version,
                            credential_id,
                            expected_revision,
                            subject,
                            expires_at,
                        } => (
                            Action::Rotate,
                            version,
                            credential_id,
                            expected_revision,
                            subject,
                            expires_at,
                        ),
                        _ => return Err(E::InvalidInput),
                    };
                let expiry = chrono::DateTime::parse_from_rfc3339(&expires_at)
                    .map_err(|_| E::InvalidInput)?;
                if expiry.timestamp_millis() <= chrono::Utc::now().timestamp_millis() {
                    return Err(E::InvalidInput);
                }
                let mut entropy = [0u8; 32];
                SystemRandom::new()
                    .fill(&mut entropy)
                    .map_err(|_| E::Unavailable)?;
                let secret = format!("aigw1_{}", hex::encode(entropy));
                let request = Request::Issue(WorkloadBindingIssueV1 {
                    version,
                    operation_id: Uuid::new_v4().to_string(),
                    credential_id,
                    expected_revision,
                    subject,
                    sha256_digest: hex::encode(Sha256::digest(secret.as_bytes())),
                    expires_at,
                });
                (action, request, Some(secret))
            }
        };
        request.validate(action).map_err(|_| E::InvalidInput)?;
        Ok((action, request, secret))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlWire {
    version: u32,
    gateway: String,
    expected_tenant_id: String,
    control_namespace_uuid: String,
    runner_token_file: PathBuf,
    #[serde(default)]
    ca_bundle_file: Option<PathBuf>,
}

pub(super) struct Control {
    pub source: PrivateFile,
    pub gateway: reqwest::Url,
    pub expected_tenant_id: String,
    pub control_namespace_uuid: String,
    pub runner_token_file: PathBuf,
    pub ca_bundle_file: Option<PathBuf>,
}

impl Control {
    pub fn load(path: &Path) -> Result<Self> {
        let source = PrivateFile::open(
            &absolute_path(path).map_err(|_| E::InvalidControl)?,
            MAX_INPUT,
        )
        .map_err(|_| E::InvalidControl)?;
        let value: ControlWire =
            serde_json::from_slice(source.bytes()).map_err(|_| E::InvalidControl)?;
        if value.version != 1
            || !value.runner_token_file.is_absolute()
            || value
                .ca_bundle_file
                .as_ref()
                .is_some_and(|p| !p.is_absolute())
        {
            return Err(E::InvalidControl);
        }
        crate::tenancy::validate_tenant_id(&value.expected_tenant_id)
            .map_err(|_| E::InvalidControl)?;
        canonical_uuid(&value.control_namespace_uuid).map_err(|_| E::InvalidControl)?;
        let gateway = origin(&value.gateway)?;
        if value.ca_bundle_file.is_some() && gateway.scheme() != "https" {
            return Err(E::InvalidControl);
        }
        source.revalidate().map_err(|_| E::InvalidControl)?;
        Ok(Self {
            source,
            gateway,
            expected_tenant_id: value.expected_tenant_id,
            control_namespace_uuid: value.control_namespace_uuid,
            runner_token_file: absolute_path(&value.runner_token_file)
                .map_err(|_| E::InvalidControl)?,
            ca_bundle_file: value
                .ca_bundle_file
                .as_deref()
                .map(absolute_path)
                .transpose()
                .map_err(|_| E::InvalidControl)?,
        })
    }
    pub fn revalidate(&self) -> Result<()> {
        self.source.revalidate().map_err(|_| E::InvalidControl)
    }
}

pub(super) fn canonical_uuid(value: &str) -> Result<()> {
    let id = Uuid::parse_str(value).map_err(|_| E::UnsafeState)?;
    if id.is_nil() || id.to_string() != value {
        return Err(E::UnsafeState);
    }
    Ok(())
}

pub(super) fn origin(value: &str) -> Result<reqwest::Url> {
    if value.len() > 2048 {
        return Err(E::InvalidControl);
    }
    let origin = reqwest::Url::parse(value).map_err(|_| E::InvalidControl)?;
    let local = origin.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !(origin.scheme() == "https" || origin.scheme() == "http" && local)
    {
        return Err(E::InvalidControl);
    }
    Ok(origin)
}

fn read_stdin() -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let stdin = std::io::stdin();
    let _guard = stdin.lock();
    let owned = stdin
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| E::InvalidInput)?;
    let mut input = std::fs::File::from(owned);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(E::InputDeadline);
        }
        let timeout =
            i32::try_from(remaining.as_millis().saturating_add(1)).map_err(|_| E::InputDeadline)?;
        let mut descriptor = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: initialized pollfd, retained owned FD, bounded monotonic timeout.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(E::InvalidInput);
        }
        if Instant::now() >= deadline {
            return Err(E::InputDeadline);
        }
        if ready == 0 {
            continue;
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(E::InvalidInput);
        }
        if descriptor.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            continue;
        }
        let limit = chunk.len().min(MAX_INPUT as usize + 1 - bytes.len());
        let count = match input.read(&mut chunk[..limit]) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(_) => return Err(E::InvalidInput),
        };
        if Instant::now() >= deadline {
            return Err(E::InputDeadline);
        }
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() as u64 > MAX_INPUT {
            return Err(E::InvalidInput);
        }
    }
}
