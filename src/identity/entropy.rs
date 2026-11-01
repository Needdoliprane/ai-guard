use std::io::Read;

pub(crate) fn random_bytes() -> anyhow::Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}
pub(crate) fn local_credential_format(token: &str) -> bool {
    token.strip_prefix("aigh1_").is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Returns a fresh secret to trusted provisioning code. It must be published
/// only into the retained private credential file, never stdout or an audit.
pub fn generate_local_credential() -> anyhow::Result<String> {
    Ok(format!("aigh1_{}", hex::encode(random_bytes()?)))
}
