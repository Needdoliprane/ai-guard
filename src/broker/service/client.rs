//! Explicit local client. Discovering a missing broker never starts one.
use super::{
    profile::SubmitV1,
    protocol::{BrokerCommandV1, BrokerReplyV1, EnvelopeV1, ReplyEnvelopeV1},
};
use crate::{
    broker::{
        discovery::load_endpoint,
        local_ipc::{read_frame, write_frame},
    },
    control_fs::{Directory, absolute_path},
};
use anyhow::{Result, anyhow, ensure};
use std::{path::Path, time::Duration};

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
    Ok(answer.reply)
}
