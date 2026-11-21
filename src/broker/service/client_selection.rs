//! An operator-selected client route. The broker and gateway still prove
//! the presented credential, tenant and operation at request time.
use super::{BrokerClient, profile::Registry};
use crate::control_fs::{Directory, PrivateFile, absolute_path, publication::PublicationFailure};
use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const RECORD_NAME: &str = "selection.json";
// Two paths of up to 4096 bytes fit even with JSON escaping.
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const REGISTRY_CHANGED: &str = "private broker registry no longer matches the published selection";
const WORKSPACE_OVERLAP: &str =
    "private broker selection must be outside configured source and export roots";
const WORKSPACE_OBSERVATION_FAILED: &str =
    "private broker selection workspace root or destination ancestry could not be verified";
const WORKSPACE_NAMES_AMBIGUOUS: &str =
    "private broker selection and an absent workspace root have ambiguous names in the same parent";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionV1 {
    version: u32,
    directory: PathBuf,
    directory_identity: (u64, u64),
    registry: PathBuf,
    registry_identity: (u64, u64),
    registry_sha256: String,
    installation_id: Uuid,
    profile: String,
}

/// An exact profile and the private registry checked by `open`.
pub struct BrokerClientSelection {
    source: PrivateFile,
    registry: Registry,
    profile: String,
}

impl BrokerClientSelection {
    /// Publish a new private selection directory below an existing private
    /// parent. This validates configuration without reading a runner token.
    pub fn publish(directory: &Path, registry: &Path, profile: &str) -> Result<()> {
        let (parent, name) = selection_parent(directory)?;
        parent.require_absent_child(&name)?;
        let registry = Registry::load(&bounded_absolute_path(registry)?)?;
        registry.profile(profile)?;
        let directory = parent.path().join(&name);
        require_outside_workspace_roots(&directory, &parent, &registry)?;
        let registry_identity = registry_identity(&registry)?;
        parent.durable_bootstrap()?;
        let stage = parent.exclusive_child(&format!(".broker-selection-{}", Uuid::new_v4()))?;
        let selection = SelectionV1 {
            version: 1,
            directory,
            directory_identity: stage.physical_identity()?,
            registry: registry.source.path().to_owned(),
            registry_identity,
            registry_sha256: registry.source.digest().to_owned(),
            installation_id: registry.data.installation_id,
            profile: profile.to_owned(),
        };
        let record = serde_json::to_vec(&selection)
            .map_err(|_| anyhow!("private broker selection cannot be encoded"))?;
        ensure!(
            record.len() as u64 <= MAX_RECORD_BYTES,
            "private broker selection exceeds its bound"
        );
        let source = stage.persistent_create(RECORD_NAME, &record)?;
        source.persistent_sync()?;
        registry
            .source
            .persistent_revalidate()
            .map_err(|_| anyhow!(REGISTRY_CHANGED))?;
        registry
            .source_directory
            .revalidate()
            .map_err(|_| anyhow!(REGISTRY_CHANGED))?;
        match parent.publish_child_exclusive(&stage, &name) {
            Ok(()) => Ok(()),
            Err(PublicationFailure::NotPublished) => Err(anyhow!(
                "private broker selection was not published; retained staging was preserved"
            )),
            Err(PublicationFailure::DurabilityUnknown) => Err(anyhow!(
                "private broker selection publication durability is unconfirmed"
            )),
        }
    }

    /// Open the exact published selection and its checked registry.
    pub fn open(directory: &Path) -> Result<Self> {
        let (parent, name) = selection_parent(directory)?;
        let directory = Directory::open_existing(&parent.path().join(&name))
            .map_err(|_| anyhow!("published private broker selection is unavailable or unsafe"))?;
        let source = directory
            .persistent_read(RECORD_NAME, MAX_RECORD_BYTES, &[])?
            .context("published private broker selection is unavailable")?;
        let selection: SelectionV1 = serde_json::from_slice(source.bytes())
            .map_err(|_| anyhow!("invalid private broker selection"))?;
        ensure!(
            selection.version == 1
                && selection.directory.as_os_str() == directory.path().as_os_str()
                && selection.directory_identity == directory.physical_identity()?,
            "invalid or unpublished private broker selection"
        );
        let registry_path = bounded_absolute_path(&selection.registry)?;
        ensure!(
            registry_path.as_os_str() == selection.registry.as_os_str(),
            "invalid private broker selection"
        );
        let registry = Registry::load(&registry_path).map_err(|_| anyhow!(REGISTRY_CHANGED))?;
        ensure!(
            registry.data.installation_id == selection.installation_id
                && registry.source.digest() == selection.registry_sha256
                && registry_identity(&registry).map_err(|_| anyhow!(REGISTRY_CHANGED))?
                    == selection.registry_identity,
            REGISTRY_CHANGED
        );
        registry.profile(&selection.profile)?;
        require_outside_workspace_roots(directory.path(), &directory, &registry)?;
        // The rename can be visible just before the publisher synchronizes
        // the parent. Confirm durability before accepting that record here.
        source.persistent_sync()?;
        parent.sync()?;
        source.persistent_revalidate()?;
        registry
            .source
            .persistent_revalidate()
            .map_err(|_| anyhow!(REGISTRY_CHANGED))?;
        Ok(Self {
            source,
            registry,
            profile: selection.profile,
        })
    }

    /// Open the credential for this profile through the same checked registry.
    pub fn into_client(self) -> Result<BrokerClient> {
        self.source
            .persistent_revalidate()
            .map_err(|_| anyhow!("private broker selection changed"))?;
        self.registry
            .source
            .persistent_revalidate()
            .map_err(|_| anyhow!(REGISTRY_CHANGED))?;
        self.registry
            .source_directory
            .revalidate()
            .map_err(|_| anyhow!(REGISTRY_CHANGED))?;
        BrokerClient::from_registry(self.registry, self.profile, Some(self.source))
    }
}

fn bounded_absolute_path(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "broker selection paths must be absolute"
    );
    let path = absolute_path(path).map_err(|_| anyhow!("invalid private broker selection path"))?;
    ensure!(
        path.as_os_str().len() <= 4096
            && path.components().count() <= 64
            && path.to_str().is_some(),
        "broker selection paths must be bounded UTF-8 absolute paths"
    );
    Ok(path)
}

fn selection_parent(path: &Path) -> Result<(Directory, String)> {
    let path = bounded_absolute_path(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("private broker selection requires a destination name")?;
    let parent = path
        .parent()
        .context("private broker selection requires a parent directory")?;
    let parent = Directory::open_existing(parent)
        .map_err(|_| anyhow!("private broker selection parent is unavailable or unsafe"))?;
    Ok((parent, name.to_owned()))
}

fn require_outside_workspace_roots(
    destination: &Path,
    existing_ancestor: &Directory,
    registry: &Registry,
) -> Result<()> {
    let roots = registry
        .data
        .profiles
        .iter()
        .flat_map(|profile| profile.source_roots.iter().chain(&profile.export_roots));
    ensure!(
        !roots.clone().any(|root| destination.starts_with(root)),
        WORKSPACE_OVERLAP
    );

    // Keep the real private directory alive. Its revalidation checks the
    // retained ancestors on both sides of the metadata-only observations.
    let identity = existing_ancestor
        .physical_identity()
        .map_err(|_| anyhow!(WORKSPACE_OBSERVATION_FAILED))?;
    let mut ancestors = vec![identity];
    for path in existing_ancestor.path().ancestors().skip(1) {
        let facts = fs::metadata(path).map_err(|_| anyhow!(WORKSPACE_OBSERVATION_FAILED))?;
        ensure!(facts.is_dir(), WORKSPACE_OBSERVATION_FAILED);
        ancestors.push((facts.dev(), facts.ino()));
    }

    for root in roots {
        let facts = match fs::metadata(root) {
            Ok(facts) => facts,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let root_parent = observe_absent_workspace_parent(root)?;
                if destination.parent() == Some(existing_ancestor.path())
                    && root_parent == Some(identity)
                {
                    require_distinct_workspace_names(destination, root)?;
                }
                continue;
            }
            Err(_) => return Err(anyhow!(WORKSPACE_OBSERVATION_FAILED)),
        };
        ensure!(facts.is_dir(), WORKSPACE_OBSERVATION_FAILED);
        ensure!(
            !ancestors.contains(&(facts.dev(), facts.ino())),
            WORKSPACE_OVERLAP
        );
    }
    existing_ancestor
        .revalidate()
        .map_err(|_| anyhow!(WORKSPACE_OBSERVATION_FAILED))?;
    Ok(())
}

/// Return a physical identity only when the missing root's immediate parent
/// exists. A dangling link at the root or first existing ancestor is an
/// unresolved mapping, not evidence that the path is simply absent.
fn observe_absent_workspace_parent(root: &Path) -> Result<Option<(u64, u64)>> {
    for (depth, path) in root.ancestors().enumerate() {
        let facts = match fs::symlink_metadata(path) {
            Ok(facts) => facts,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(_) => return Err(anyhow!(WORKSPACE_OBSERVATION_FAILED)),
        };
        ensure!(depth != 0, WORKSPACE_OBSERVATION_FAILED);
        let facts = if facts.is_symlink() {
            fs::metadata(path).map_err(|_| anyhow!(WORKSPACE_OBSERVATION_FAILED))?
        } else {
            facts
        };
        ensure!(facts.is_dir(), WORKSPACE_OBSERVATION_FAILED);
        return Ok((depth == 1).then_some((facts.dev(), facts.ino())));
    }
    Err(anyhow!(WORKSPACE_OBSERVATION_FAILED))
}

fn require_distinct_workspace_names(destination: &Path, root: &Path) -> Result<()> {
    let destination = destination
        .file_name()
        .context(WORKSPACE_OBSERVATION_FAILED)?
        .as_encoded_bytes();
    let root = root
        .file_name()
        .context(WORKSPACE_OBSERVATION_FAILED)?
        .as_encoded_bytes();
    // Local filesystems can collapse case or Unicode spellings. For two
    // absent siblings there is no inode with which to disprove the alias.
    ensure!(
        destination.is_ascii() && root.is_ascii(),
        WORKSPACE_NAMES_AMBIGUOUS
    );
    ensure!(!destination.eq_ignore_ascii_case(root), WORKSPACE_OVERLAP);
    Ok(())
}

fn registry_identity(registry: &Registry) -> Result<(u64, u64)> {
    let source = &registry.source;
    let name = source
        .path()
        .file_name()
        .context("invalid private broker registry name")?;
    source.persistent_revalidate()?;
    let identity = registry
        .source_directory
        .private_entry(name)?
        .context("private broker registry is absent")?;
    source.persistent_revalidate()?;
    Ok(identity.physical_identity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_fs::publication::FAIL_AFTER_RENAME;
    use serde_json::json;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const PROFILE: &str = "selected";

    fn fixture() -> Result<(tempfile::TempDir, Directory, PrivateFile)> {
        fixture_with_selected_source(Path::new("selected-source"))
    }

    fn fixture_with_selected_source(
        selected_source: &Path,
    ) -> Result<(tempfile::TempDir, Directory, PrivateFile)> {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let directory = Directory::open_existing(&temporary.path().canonicalize()?)?;
        let root = directory.path();
        let profile = |id: &str, tenant: &str| {
            let source_root = if id == PROFILE {
                root.join(selected_source)
            } else {
                root.join(format!("{id}-source"))
            };
            json!({
                "id": id,
                "gateway": "http://127.0.0.1:9",
                "tenant_id": tenant,
                "control_namespace_uuid": Uuid::new_v4(),
                "config_dir": root.join("config"),
                "runner_token_file": root.join(format!("{id}-runner")),
                "runtime": "docker",
                "runtime_program": root.join("engine-not-opened"),
                "runtime_endpoint": "unix:///not-opened/engine.sock",
                "image": "worker:synthetic",
                "policy_profile": "strict",
                "network": "none",
                "allow_rootful_runtime": false,
                "max_timeout_seconds": 20,
                "allow_capture_output": false,
                "managed_clients": false,
                "source_roots": [source_root],
                "export_roots": [root.join(format!("{id}-export"))]
            })
        };
        let bytes = serde_json::to_vec(&json!({
            "version": 1,
            "installation_id": Uuid::new_v4(),
            "root": root.join("broker-state"),
            "max_enforcer_slots": 8,
            "profiles": [profile("other", "tenant-b"), profile(PROFILE, "tenant-a")]
        }))?;
        let registry = directory.persistent_create("registry.json", &bytes)?;
        Ok((temporary, directory, registry))
    }

    #[test]
    fn published_selector_pins_the_explicit_profile_without_opening_credentials() -> Result<()> {
        let (_temporary, parent, registry) = fixture()?;
        let destination = parent.path().join("selection for CLI");
        assert!(BrokerClientSelection::publish(&destination, registry.path(), "unknown").is_err());
        assert!(!destination.exists());
        assert!(
            BrokerClientSelection::publish(Path::new("relative"), registry.path(), PROFILE)
                .is_err()
        );
        BrokerClientSelection::publish(&destination, registry.path(), PROFILE)?;

        let published = Directory::open_existing(&destination)?;
        let entries = published.persistent_entries(2)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, RECORD_NAME);
        let source = published
            .persistent_read(RECORD_NAME, MAX_RECORD_BYTES, &[])?
            .unwrap();
        let record: SelectionV1 = serde_json::from_slice(source.bytes())?;
        assert_eq!(record.registry_sha256, registry.digest());
        assert_eq!(record.registry, registry.path());
        assert_eq!(record.profile, PROFILE);
        let selected = BrokerClientSelection::open(&destination)?;
        assert_eq!(selected.profile, PROFILE);
        assert!(
            selected.into_client().is_err(),
            "the selected runner file is absent"
        );
        parent.persistent_create("selected-runner", b"synthetic-runner-fixture")?;
        BrokerClientSelection::open(&destination)?.into_client()?;
        assert!(BrokerClientSelection::publish(&destination, registry.path(), "other").is_err());
        source.persistent_revalidate()?;
        Ok(())
    }

    #[test]
    fn selector_refuses_registry_replacement_with_identical_bytes() -> Result<()> {
        let (_temporary, parent, registry) = fixture()?;
        let destination = parent.path().join("selection");
        BrokerClientSelection::publish(&destination, registry.path(), PROFILE)?;
        let opened = BrokerClientSelection::open(&destination)?;
        parent.persistent_create("selected-runner", b"synthetic-runner-fixture")?;
        fs::rename(
            registry.path(),
            parent.path().join("retained-registry.json"),
        )?;
        parent.persistent_create("registry.json", registry.bytes())?;
        let refusal = BrokerClientSelection::open(&destination)
            .err()
            .context("registry replacement was accepted")?;
        assert_eq!(refusal.to_string(), REGISTRY_CHANGED);
        assert!(opened.into_client().is_err());
        Ok(())
    }

    #[test]
    fn selector_refuses_source_and_export_destinations_before_staging_and_on_read() -> Result<()> {
        let (_temporary, parent, registry) = fixture()?;
        let valid_path = parent.path().join("selected-source-sibling");
        BrokerClientSelection::publish(&valid_path, registry.path(), PROFILE)?;
        let valid = Directory::open_existing(&valid_path)?
            .persistent_read(RECORD_NAME, MAX_RECORD_BYTES, &[])?
            .context("published test selection missing")?;

        for name in ["selected-source", "other-export"] {
            let root = parent.exclusive_child(name)?;
            let destination = root.path().join("selection");
            let refusal = BrokerClientSelection::publish(&destination, registry.path(), PROFILE)
                .err()
                .context("workspace destination was published")?;
            assert_eq!(refusal.to_string(), WORKSPACE_OVERLAP);
            assert!(root.persistent_entries(4)?.is_empty());

            // Exercise the reader with the exact published binding rewritten
            // solely to match the fixture's disallowed destination and inode.
            let invalid = root.exclusive_child("selection")?;
            let mut record: SelectionV1 = serde_json::from_slice(valid.bytes())?;
            record.directory = destination.clone();
            record.directory_identity = invalid.physical_identity()?;
            invalid.persistent_create(RECORD_NAME, &serde_json::to_vec(&record)?)?;
            let refusal = BrokerClientSelection::open(&destination)
                .err()
                .context("workspace selection was opened")?;
            assert_eq!(refusal.to_string(), WORKSPACE_OVERLAP);
        }
        Ok(())
    }

    #[test]
    fn selector_refuses_a_workspace_alias_to_an_existing_destination_ancestor() -> Result<()> {
        let (_temporary, parent, registry) = fixture()?;
        let live_root = parent.exclusive_child("same-workspace")?;
        let nested = live_root.exclusive_child("nested")?;
        let selection = nested.path().join("selection");
        // An absent configured root is permitted. Once it points at the
        // existing workspace, the same selection must be refused at read.
        BrokerClientSelection::publish(&selection, registry.path(), PROFILE)?;
        symlink(live_root.path(), parent.path().join("selected-source"))?;

        let next = nested.path().join("new-selection");
        let refusal = BrokerClientSelection::publish(&next, registry.path(), PROFILE)
            .err()
            .context("workspace alias allowed private publication")?;
        assert_eq!(refusal.to_string(), WORKSPACE_OVERLAP);
        assert_eq!(nested.persistent_entries(4)?.len(), 1);
        let refusal = BrokerClientSelection::open(&selection)
            .err()
            .context("workspace alias allowed private selection lookup")?;
        assert_eq!(refusal.to_string(), WORKSPACE_OVERLAP);
        Ok(())
    }

    #[test]
    fn selector_refuses_ambiguous_absent_workspace_siblings_before_staging() -> Result<()> {
        let (_temporary, parent, registry) =
            fixture_with_selected_source(Path::new("parent-alias/selected-source"))?;
        symlink(parent.path(), parent.path().join("parent-alias"))?;
        let original_entries = fs::read_dir(parent.path())?.count();
        for name in ["selected-source", "SELECTED-SOURCE"] {
            let refusal =
                BrokerClientSelection::publish(&parent.path().join(name), registry.path(), PROFILE)
                    .err()
                    .context("an absent workspace alias allowed private publication")?;
            assert_eq!(refusal.to_string(), WORKSPACE_OVERLAP);
        }
        let refusal = BrokerClientSelection::publish(
            &parent.path().join("sélection"),
            registry.path(),
            PROFILE,
        )
        .err()
        .context("an unverified Unicode sibling allowed private publication")?;
        assert_eq!(refusal.to_string(), WORKSPACE_NAMES_AMBIGUOUS);
        assert_eq!(fs::read_dir(parent.path())?.count(), original_entries);

        let independent = parent.path().join("distinct-selection");
        BrokerClientSelection::publish(&independent, registry.path(), PROFILE)?;
        BrokerClientSelection::open(&independent)?;
        Ok(())
    }

    #[test]
    fn selector_refuses_a_dangling_workspace_path_or_ancestor() -> Result<()> {
        for root in ["unresolved", "unresolved/workspace"] {
            let (_temporary, parent, registry) = fixture_with_selected_source(Path::new(root))?;
            let destination = parent.path().join("selection");
            symlink(&destination, parent.path().join("unresolved"))?;
            let original_entries = fs::read_dir(parent.path())?.count();
            let refusal = BrokerClientSelection::publish(&destination, registry.path(), PROFILE)
                .err()
                .context("an unresolved workspace alias allowed private publication")?;
            assert_eq!(refusal.to_string(), WORKSPACE_OBSERVATION_FAILED);
            assert_eq!(fs::read_dir(parent.path())?.count(), original_entries);
        }
        Ok(())
    }

    #[test]
    fn selection_can_be_confirmed_after_interrupted_publication_without_replacement() -> Result<()>
    {
        let (_temporary, parent, registry) = fixture()?;
        let destination = parent.path().join("selection");
        FAIL_AFTER_RENAME.with(|failure| failure.set(true));
        let result = BrokerClientSelection::publish(&destination, registry.path(), PROFILE);
        // A pre-rename failure must not leave the thread-local hook armed.
        FAIL_AFTER_RENAME.with(|failure| failure.set(false));
        let refusal = result
            .err()
            .context("publication failure was not reported")?;
        assert_eq!(
            refusal.to_string(),
            "private broker selection publication durability is unconfirmed"
        );
        let selection = BrokerClientSelection::open(&destination)?;
        assert_eq!(selection.profile, PROFILE);
        assert!(BrokerClientSelection::publish(&destination, registry.path(), PROFILE).is_err());
        selection.source.persistent_revalidate()?;
        Ok(())
    }
}
