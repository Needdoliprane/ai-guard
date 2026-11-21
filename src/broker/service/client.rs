//! Explicit local client. Discovering a missing broker never starts one.
use super::{
    profile::{Registry, SubmitV1},
    protocol::{BrokerCommandV1, BrokerReplyV1, EnvelopeV1, ReplyEnvelopeV1},
};
use crate::{
    broker::{
        discovery::load_endpoint,
        local_ipc::{read_frame, write_frame},
    },
    control_fs::{Directory, PrivateFile, absolute_path},
};
use anyhow::{Result, anyhow, ensure};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Retain a single private credential identity for the duration of a CLI operation.
pub struct BrokerClient {
    root: PathBuf,
    profile: String,
    credential: PrivateFile,
    registry_source: Option<(PrivateFile, Directory)>,
    selection_source: Option<PrivateFile>,
}

impl BrokerClient {
    pub fn open(root: PathBuf, profile: String, token_file: &Path) -> Result<Self> {
        let credential = PrivateFile::open(token_file, 4096)
            .map_err(|_| anyhow!("private broker credential file is unavailable or unsafe"))?;
        Ok(Self {
            root,
            profile,
            credential,
            registry_source: None,
            selection_source: None,
        })
    }

    /// Load the named profile from an absolute private broker registry.
    pub fn open_from_config(config: &Path, profile: String) -> Result<Self> {
        Self::from_registry(Registry::load(config)?, profile, None)
    }

    pub(super) fn from_registry(
        registry: Registry,
        profile: String,
        selection_source: Option<PrivateFile>,
    ) -> Result<Self> {
        let token_file = &registry.profile(&profile)?.runner_token_file;
        let mut client = Self::open(registry.data.root.clone(), profile, token_file)?;
        client.registry_source = Some((registry.source, registry.source_directory));
        client.selection_source = selection_source;
        Ok(client)
    }

    pub async fn request(&self, command: BrokerCommandV1) -> Result<BrokerReplyV1> {
        if let Some(source) = &self.selection_source {
            source
                .persistent_revalidate()
                .map_err(|_| anyhow!("private broker selection changed"))?;
        }
        if let Some((source, directory)) = &self.registry_source {
            source
                .revalidate()
                .and_then(|()| directory.revalidate())
                .map_err(|_| anyhow!("private broker configuration changed"))?;
        }
        self.credential
            .revalidate()
            .map_err(|_| anyhow!("broker credential file changed"))?;
        let credential = std::str::from_utf8(self.credential.bytes())
            .map_err(|_| anyhow!("invalid broker credential file"))?
            .trim();
        request(&self.root, &self.profile, credential, command).await
    }
}

pub fn encode_submit(submit: &SubmitV1) -> Result<String> {
    let bytes = serde_json::to_vec(submit).map_err(|_| anyhow!("invalid broker submission"))?;
    SubmitV1::parse(&bytes)?;
    String::from_utf8(bytes).map_err(|_| anyhow!("invalid broker submission"))
}

pub async fn request(
    root: &Path,
    profile_id: &str,
    credential: &str,
    command: BrokerCommandV1,
) -> Result<BrokerReplyV1> {
    let expected_v2_operation = match &command {
        BrokerCommandV1::RecoveryStatusV2 { operation_id } => {
            ensure!(
                !operation_id.is_nil(),
                "invalid recovery-status-v2 operation identity"
            );
            Some(*operation_id)
        }
        _ => None,
    };
    if let BrokerCommandV1::RecoveryInventory { after, limit } = &command {
        ensure!(
            (1..=100).contains(limit) && after.is_none_or(|id| !id.is_nil()),
            "invalid recovery inventory bounds"
        );
    }
    ensure!(
        !credential.is_empty()
            && credential.len() <= 4096
            && credential.is_ascii()
            && !credential.chars().any(char::is_whitespace)
            && !credential.chars().any(char::is_control),
        "invalid broker credential"
    );
    ensure!(
        !profile_id.is_empty()
            && profile_id.len() <= 80
            && profile_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "invalid broker profile reference"
    );
    let root = absolute_path(root).map_err(|_| anyhow!("invalid private broker root"))?;
    let root = Directory::open_existing(&root)
        .map_err(|_| anyhow!("private broker root is unavailable"))?;
    let role = root
        .persistent_child("broker", false)
        .map_err(|_| anyhow!("private broker discovery is unsafe"))?
        .ok_or_else(|| anyhow!("broker is not running; start broker serve explicitly"))?;
    let endpoint = load_endpoint(&role)
        .map_err(|_| anyhow!("private broker discovery is unavailable or unsafe"))?
        .ok_or_else(|| anyhow!("broker is not running; start broker serve explicitly"))?;
    let endpoints = role
        .persistent_child("endpoints", false)
        .map_err(|_| anyhow!("private broker endpoint is unsafe"))?
        .ok_or_else(|| anyhow!("private broker endpoint is unavailable"))?;
    let mut stream = endpoint
        .connect_checked(&endpoints)
        .await
        .map_err(|_| anyhow!("broker is unavailable; request was not sent"))?;
    let envelope = EnvelopeV1 {
        version: 1,
        profile_id: profile_id.into(),
        credential: credential.into(),
        request: command,
    };
    write_frame(&mut stream, &envelope, 512 * 1024, Duration::from_secs(15))
        .await
        .map_err(|_| {
            anyhow!(
                "broker request outcome is unknown; inspect or retry the same operation identity"
            )
        })?;
    let answer:ReplyEnvelopeV1 = read_frame(&mut stream,512*1024,Duration::from_secs(60)).await
        .map_err(|_| anyhow!("broker reply is unavailable or invalid; inspect or retry the same operation identity"))?;
    ensure!(answer.version == 1, "incompatible broker reply version");
    if let Some(operation) = expected_v2_operation {
        match &answer.reply {
            BrokerReplyV1::RecoveryStatusV2 { status } => ensure!(
                status.version == 2
                    && status.journal.version == 1
                    && status
                        .preclaim
                        .iter()
                        .all(|entry| entry.preclaim.version == 1)
                    && status.journal.operation_id == operation,
                "incompatible or uncorrelated recovery-status-v2 reply; no V1 fallback was attempted"
            ),
            BrokerReplyV1::Error { code } => anyhow::bail!(
                "recovery-status-v2 refused ({code}); V2 compatibility is not established, including with an older broker; no V1 fallback was attempted"
            ),
            _ => anyhow::bail!(
                "incompatible broker reply to recovery-status-v2; no V1 fallback was attempted"
            ),
        }
    }
    Ok(answer.reply)
}
