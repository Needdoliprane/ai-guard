//! Authenticated tenant boundaries shared by the gateway and durable storage.

pub const LOCAL_TENANT: &str = "local";

pub fn validate_tenant_id(tenant_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !tenant_id.is_empty()
            && tenant_id.len() <= 128
            && tenant_id
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') }),
        "tenant identity must contain 1 to 128 ASCII letters, digits, hyphens or underscores"
    );
    Ok(())
}
