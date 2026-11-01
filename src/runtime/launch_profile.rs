//! Immutable Docker CLI start configuration. This is data and a retained file
//! capability, never a start permission or a completion/fencing witness.
use super::{Runtime, RuntimeKind};
use crate::control_fs::{Directory, PrivateFile};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::{Component, Path},
};
use uuid::Uuid;

pub(crate) const DIRECTORY: &str = "docker-launch-v1";
pub(crate) const FILE: &str = "config.json";
const CONTENT: &[u8] = b"{}\n";

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn endpoint(value: &str) -> bool {
    value.strip_prefix("unix://").is_some_and(|path| {
        path.starts_with('/')
            && path.len() > 1
            && path.len() <= 4096
            && !path.chars().any(char::is_control)
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
    })
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DockerLaunchProfileBindingV1 {
    pub(crate) version: u32,
    pub(crate) kind: String,
    pub(crate) journal_scope_hash: String,
    pub(crate) parent_operation_id: Uuid,
    pub(crate) intent_hash: String,
    pub(crate) directory_ref: String,
    pub(crate) file_name: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) runtime_endpoint: String,
    pub(crate) invocation_fingerprint: String,
}
impl DockerLaunchProfileBindingV1 {
    pub(crate) fn new(
        scope: &str,
        parent: Uuid,
        intent: &str,
        target: &str,
        invocation: &str,
    ) -> Result<Self> {
        let value = Self {
            version: 1,
            kind: "docker_minimal".into(),
            journal_scope_hash: scope.into(),
            parent_operation_id: parent,
            intent_hash: intent.into(),
            directory_ref: DIRECTORY.into(),
            file_name: FILE.into(),
            sha256: hex::encode(Sha256::digest(CONTENT)),
            bytes: 3,
            runtime_endpoint: target.into(),
            invocation_fingerprint: invocation.into(),
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.kind == "docker_minimal"
                && self.directory_ref == DIRECTORY
                && self.file_name == FILE
                && self.bytes == 3
                && self.sha256 == hex::encode(Sha256::digest(CONTENT))
                && hash(&self.journal_scope_hash)
                && hash(&self.intent_hash)
                && hash(&self.invocation_fingerprint)
                && !self.parent_operation_id.is_nil()
                && endpoint(&self.runtime_endpoint),
            "invalid Docker start profile binding"
        );
        Ok(())
    }
}
impl fmt::Debug for DockerLaunchProfileBindingV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DockerLaunchProfileBindingV1(<private>)")
    }
}

pub(crate) struct LaunchProfileSelection {
    endpoint: String,
    invocation: String,
}
impl LaunchProfileSelection {
    pub(crate) fn from_runtime(runtime: &Runtime, target: &str) -> Result<Self> {
        ensure!(
            runtime.kind == RuntimeKind::Docker
                && runtime.invocation().policy_version == 2
                && endpoint(target),
            "minimal Docker start profile requires an approved Docker invocation and Unix endpoint"
        );
        Ok(Self {
            endpoint: target.into(),
            invocation: runtime.invocation().fingerprint(),
        })
    }
    pub(crate) fn binding(
        &self,
        scope: &str,
        parent: Uuid,
        intent: &str,
    ) -> Result<DockerLaunchProfileBindingV1> {
        DockerLaunchProfileBindingV1::new(scope, parent, intent, &self.endpoint, &self.invocation)
    }
}

#[derive(Clone)]
pub(crate) struct RetainedDockerLaunchProfile {
    binding: DockerLaunchProfileBindingV1,
    directory: Directory,
    file: PrivateFile,
}
impl fmt::Debug for RetainedDockerLaunchProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetainedDockerLaunchProfile(<private>)")
    }
}
impl RetainedDockerLaunchProfile {
    pub(crate) fn create(job: &Directory, binding: &DockerLaunchProfileBindingV1) -> Result<Self> {
        binding.validate()?;
        ensure!(
            job.persistent_child(DIRECTORY, false)?.is_none(),
            "Docker start profile already exists; reconciliation required"
        );
        let directory = job
            .persistent_child(DIRECTORY, true)?
            .ok_or_else(|| anyhow::anyhow!("Docker start profile directory unavailable"))?;
        let file = directory.persistent_create(FILE, CONTENT)?;
        file.persistent_sync()?;
        directory.sync()?;
        job.sync()?;
        let value = Self {
            binding: binding.clone(),
            directory,
            file,
        };
        value.revalidate()?;
        Ok(value)
    }
    pub(crate) fn load(
        job: &Directory,
        binding: &DockerLaunchProfileBindingV1,
        exclusions: &[crate::control_fs::PrivateEntryIdentity],
    ) -> Result<Self> {
        binding.validate()?;
        let directory = job
            .persistent_child(DIRECTORY, false)?
            .ok_or_else(|| anyhow::anyhow!("Docker start profile missing"))?;
        Self::closed(&directory)?;
        let file = directory
            .persistent_read(FILE, 3, exclusions)?
            .ok_or_else(|| anyhow::anyhow!("Docker start profile missing"))?;
        let value = Self {
            binding: binding.clone(),
            directory,
            file,
        };
        value.revalidate()?;
        value.file.persistent_sync()?;
        value.directory.sync()?;
        Ok(value)
    }
    fn closed(directory: &Directory) -> Result<()> {
        let entries = directory.persistent_entries(2)?;
        ensure!(
            entries.len() == 1
                && entries[0].name == FILE
                && !entries[0].directory
                && entries[0].bytes == 3,
            "Docker start profile directory is incomplete or has unexpected entries"
        );
        Ok(())
    }
    pub(crate) fn revalidate(&self) -> Result<()> {
        self.binding.validate()?;
        self.directory.revalidate()?;
        self.file.persistent_revalidate()?;
        Self::closed(&self.directory)?;
        ensure!(
            self.file.bytes() == CONTENT && self.file.digest() == self.binding.sha256,
            "Docker start profile changed"
        );
        Ok(())
    }
    pub(crate) fn binding(&self) -> &DockerLaunchProfileBindingV1 {
        &self.binding
    }
    pub(crate) fn directory_path(&self) -> &Path {
        self.directory.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    fn binding() -> DockerLaunchProfileBindingV1 {
        DockerLaunchProfileBindingV1::new(
            &"1".repeat(64),
            Uuid::from_u128(1),
            &"2".repeat(64),
            "unix:///not-opened/docker.sock",
            &"3".repeat(64),
        )
        .unwrap()
    }
    fn root() -> (tempfile::TempDir, Directory) {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
        (temp, root)
    }

    #[test]
    fn minimal_profile_is_exact_private_closed_and_retained() {
        let (_temp, root) = root();
        let binding = binding();
        let profile = RetainedDockerLaunchProfile::create(&root, &binding).unwrap();
        let path = profile.directory_path().join(FILE);
        assert_eq!(fs::read(&path).unwrap(), b"{}\n");
        assert_eq!(
            fs::metadata(profile.directory_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert!(RetainedDockerLaunchProfile::create(&root, &binding).is_err());
        let loaded = RetainedDockerLaunchProfile::load(&root, &binding, &[]).unwrap();
        assert_eq!(loaded.binding(), &binding);
        fs::rename(&path, root.path().join("saved-original")).unwrap();
        let replacement = profile.directory.persistent_create(FILE, CONTENT).unwrap();
        replacement.persistent_sync().unwrap();
        assert!(
            profile.revalidate().is_err(),
            "same bytes under a substituted name are not the retained capability"
        );
    }

    #[test]
    fn minimal_profile_refuses_changed_bytes_modes_links_parents_and_extras() {
        for variant in 0..7 {
            let (_temp, root) = root();
            let binding = binding();
            let profile = RetainedDockerLaunchProfile::create(&root, &binding).unwrap();
            let path = profile.directory_path().join(FILE);
            match variant {
                0 => fs::write(&path, b"[]\n").unwrap(),
                1 => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
                2 => {
                    fs::rename(&path, root.path().join("saved")).unwrap();
                    symlink(root.path().join("saved"), &path).unwrap();
                }
                3 => {
                    fs::rename(&path, root.path().join("saved")).unwrap();
                    fs::hard_link(root.path().join("saved"), &path).unwrap();
                }
                4 => {
                    fs::rename(profile.directory_path(), root.path().join("saved-parent")).unwrap();
                    root.persistent_child(DIRECTORY, true).unwrap();
                }
                5 => {
                    profile
                        .directory
                        .persistent_create("unexpected", b"synthetic-only")
                        .unwrap();
                }
                _ => {
                    profile
                        .directory
                        .persistent_child("unexpected", true)
                        .unwrap();
                }
            }
            assert!(profile.revalidate().is_err());
            assert!(RetainedDockerLaunchProfile::load(&root, &binding, &[]).is_err());
        }
    }

    #[test]
    fn minimal_profile_binding_is_strict_and_scoped() {
        let good = binding();
        let value = serde_json::to_value(&good).unwrap();
        for (field, bad) in [
            ("version", serde_json::json!(2)),
            ("directory_ref", serde_json::json!("../escape")),
            ("file_name", serde_json::json!("elsewhere.json")),
            ("runtime_endpoint", serde_json::json!("tcp://127.0.0.1:1")),
            ("bytes", serde_json::json!(4)),
            ("sha256", serde_json::json!("0".repeat(64))),
        ] {
            let mut changed = value.clone();
            changed[field] = bad;
            let changed: DockerLaunchProfileBindingV1 = serde_json::from_value(changed).unwrap();
            assert!(changed.validate().is_err());
        }
        let mut unknown = value;
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<DockerLaunchProfileBindingV1>(unknown).is_err());
        let mut different = good.clone();
        different.journal_scope_hash = "4".repeat(64);
        assert_ne!(good, different);
    }
}
