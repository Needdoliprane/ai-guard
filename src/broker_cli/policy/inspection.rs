//! Explicit checked observation, retained only as untrusted historical metadata.
pub(super) mod storage;
#[cfg(test)]
mod tests;

use agentsandbox::{
    control::egress_policy::{EgressPolicyInspectionRequestV1, EgressPolicySelectorV1},
    control_fs::{PrivateFile, absolute_path},
    runner::AuditClient,
};
use anyhow::{Result, anyhow, ensure};
use std::{io::Write, path::PathBuf};

pub(super) async fn execute(
    gateway: String,
    token_path: PathBuf,
    tenant: bool,
    session: Option<String>,
    output_dir: PathBuf,
) -> Result<()> {
    let selector = match (tenant, session) {
        (true, None) => EgressPolicySelectorV1::Tenant,
        (false, Some(raw)) => {
            let id = uuid::Uuid::parse_str(&raw)
                .map_err(|_| anyhow!("egress_policy_invalid_selector"))?;
            ensure!(
                !id.is_nil() && id.to_string() == raw,
                "egress_policy_invalid_selector"
            );
            EgressPolicySelectorV1::Session(id.to_string())
        }
        _ => return Err(anyhow!("egress_policy_invalid_selector")),
    };
    let request = EgressPolicyInspectionRequestV1::new(selector)
        .map_err(|_| anyhow!("egress_policy_invalid_selector"))?;
    let gateway = super::storage::origin(&gateway)?;
    let token_path =
        absolute_path(&token_path).map_err(|_| anyhow!("egress_policy_unsafe_token"))?;
    let token =
        PrivateFile::open(&token_path, 4096).map_err(|_| anyhow!("egress_policy_unsafe_token"))?;
    let credential = std::str::from_utf8(token.bytes())
        .map_err(|_| anyhow!("egress_policy_invalid_token"))?
        .trim();
    let revalidate_token = || {
        token
            .revalidate()
            .map_err(|_| anyhow!("egress_policy_token_changed"))
    };
    revalidate_token()?;
    let client = AuditClient::new(&gateway, credential)
        .map_err(|_| anyhow!("egress_policy_invalid_origin_or_token"))?;
    revalidate_token()?;
    // Only this command asks for a fresh observation. Preparation and mutation
    // never inspect implicitly or repair a retained request.
    let result = client.egress_inspect_policy(&request).await;
    revalidate_token()?;
    let observation = result.map_err(|error| anyhow!(error.code()))?;
    observation
        .validate_for(&request)
        .map_err(|_| anyhow!("egress_policy_invalid_response"))?;
    let canonical = observation
        .canonical_bytes()
        .map_err(|_| anyhow!("egress_policy_invalid_response"))?;
    ensure!(canonical.len() <= 4096, "egress_policy_invalid_response");
    ensure!(
        !credential.is_empty()
            && !canonical
                .windows(credential.len())
                .any(|part| part == credential.as_bytes()),
        "egress_policy_invalid_response"
    );
    let report = storage::Report::prepare(
        &gateway,
        &request,
        &observation,
        &output_dir,
        &revalidate_token,
    )?;
    revalidate_token()?;
    report.revalidate()?;
    emit_to(&report, Some(credential), &mut std::io::stdout().lock())
}

fn output(report: &storage::Report) -> Result<Vec<u8>> {
    report.revalidate()?;
    let value = serde_json::json!({
        "version": 1,
        "kind": "policy_inspection",
        "historical": true,
        "coverage": "policy_registry_metadata",
        "observed_at": report.observation.observed_at(),
    });
    let mut bytes =
        serde_json::to_vec(&value).map_err(|_| anyhow!("egress_policy_output_unresolved"))?;
    ensure!(bytes.len() < 4096, "egress_policy_output_unresolved");
    bytes.push(b'\n');
    Ok(bytes)
}

fn emit_to(
    report: &storage::Report,
    credential: Option<&str>,
    writer: &mut impl Write,
) -> Result<()> {
    let bytes = output(report)?;
    if let Some(secret) = credential.filter(|secret| !secret.is_empty()) {
        ensure!(
            !bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()),
            "egress_policy_output_unresolved"
        );
    }
    writer
        .write_all(&bytes)
        .map_err(|_| anyhow!("egress_policy_output_unresolved"))
}
