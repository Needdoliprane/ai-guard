use agentsandbox::runner::AuditClient;
use std::{error::Error, fs, os::unix::fs::MetadataExt};

type ProbeResult<T> = Result<T, Box<dyn Error>>;

async fn verify(base: &str, pairs: &[(String, String)]) -> ProbeResult<()> {
    let directory = fs::metadata("/run/secrets")?;
    if directory.uid() != 1000 || directory.mode() & 0o777 != 0o700 {
        return Err("credential directory must be owned by UID1000 with mode0700".into());
    }
    let mut namespaces = Vec::new();
    for (tenant, path) in pairs {
        let key_file = fs::metadata(path)?;
        if !key_file.is_file() || key_file.mode() & 0o777 != 0o444 || key_file.len() > 1024 {
            return Err("unexpected projected credential file metadata".into());
        }
        let token = fs::read_to_string(path)?;
        let client = AuditClient::new(base, token.trim())?;
        let value = client.get_json("api/v1/control/protocol").await?;
        if value.get("version").and_then(|v| v.as_u64()) != Some(1)
            || value.get("schema_version").and_then(|v| v.as_u64()) != Some(3)
            || value.get("tenant_id").and_then(|v| v.as_str()) != Some(tenant.as_str())
            || value.get("role").and_then(|v| v.as_str()) != Some("runner")
            || value.get("active").and_then(|v| v.as_bool()) != Some(true)
        {
            return Err("authenticated control protocol does not match the expected tenant".into());
        }
        let namespace = value
            .get("control_namespace_uuid")
            .and_then(|v| v.as_str())
            .ok_or("control namespace missing")?;
        if namespace.len() != 36
            || namespace == "00000000-0000-0000-0000-000000000000"
            || namespaces.iter().any(|old| old == namespace)
        {
            return Err("control namespaces must be present, non-nil and distinct".into());
        }
        namespaces.push(namespace.to_owned());
        println!("http_protocol_confirmed tenant={tenant} role=runner schema=3 active=true namespace={namespace}");
    }
    let mut backups = 0;
    for entry in fs::read_dir("/data/backups")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("tenant-") || !name.ends_with("-v2.sqlite") {
            continue;
        }
        let facts = entry.metadata()?;
        if !facts.is_file()
            || facts.uid() != 1000
            || facts.mode() & 0o777 != 0o600
            || facts.nlink() != 1
            || facts.len() == 0
        {
            return Err("backup metadata is not private and complete".into());
        }
        backups += 1;
    }
    if backups != pairs.len() {
        return Err("expected one backup per tenant".into());
    }
    println!("backup_metadata_confirmed count={backups} owner=1000 mode=0600");
    Ok(())
}

fn main() -> ProbeResult<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 5 {
        return Err("usage: probe BASE TENANT_A RUNNER_FILE_A TENANT_B RUNNER_FILE_B".into());
    }
    let pairs = [
        (arguments[1].clone(), arguments[2].clone()),
        (arguments[3].clone(), arguments[4].clone()),
    ];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(verify(&arguments[0], &pairs))
}
