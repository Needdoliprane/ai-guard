//! Process-local source authorization. Directory capabilities are retained from
//! authorization through collection and are never reconstructed from a job.
//!
//! Revalidation detects changed mappings/ancestry at the observation points. It
//! does not provide an atomic snapshot against concurrent same-UID/root changes.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::{ExportReport, Manifest, anchored_export, read_workspace_archive};
use super::{Limits, PreparedWorkspace, collect_workspace_from_root};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::Read;
use std::{
    ffi::{CString, OsStr, OsString},
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

const DIRECTORY_FLAGS: libc::c_int =
    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
const MAX_SOURCE_DEPTH: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn directory(file: &File) -> Result<Self> {
        let facts = file
            .metadata()
            .context("cannot inspect authorized source directory")?;
        ensure!(facts.is_dir(), "authorized source must be a directory");
        Ok(Self {
            device: facts.dev(),
            inode: facts.ino(),
        })
    }
}

struct RetainedDirectory {
    file: File,
    identity: Identity,
    // The filesystem root is the only entry without a name relative to a
    // retained parent. All other names are single normal path components.
    name: Option<OsString>,
}

/// An allowlisted project and the live directory capabilities that authorized
/// it. No Clone, Debug or serialization implementation can expose/rebuild them.
pub(crate) struct AuthorizedSource {
    path: PathBuf,
    chain: Vec<RetainedDirectory>,
    boundary_index: usize,
}

impl AuthorizedSource {
    pub(crate) fn open(source: &Path, allowed_roots: &[PathBuf]) -> Result<Self> {
        let path = crate::control_fs::absolute_path(source)?;
        // Select the most specific matching policy root. Normalize all policy
        // paths first so a malformed root cannot be silently ignored.
        let roots = allowed_roots
            .iter()
            .map(|root| crate::control_fs::absolute_path(root))
            .collect::<Result<Vec<_>>>()?;
        let boundary = roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .context("project is outside the authorized source roots")?;
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORY_FLAGS)
            .open("/")
            .context("cannot open source filesystem root")?;
        let identity = Identity::directory(&root)?;
        let mut chain = vec![RetainedDirectory {
            file: root,
            identity,
            name: None,
        }];
        append_directories(&mut chain, boundary.strip_prefix("/")?)?;
        let boundary_index = chain.len() - 1;
        append_directories(&mut chain, path.strip_prefix(boundary)?)?;
        let authorized = Self {
            path,
            chain,
            boundary_index,
        };
        authorized.revalidate()?;
        Ok(authorized)
    }

    /// Provenance only; preparation uses the retained descriptor, never this
    /// pathname as a new content authority.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Opaque fingerprint of the authorized source and selected ancestry.
    /// Reopening the same directory on the same mount preserves this value.
    /// The original descriptor remains the authority for collecting content.
    #[allow(
        dead_code,
        reason = "the Legacy Prepare integration will bind this source identity"
    )]
    pub(crate) fn identity_hash(&self) -> Result<String> {
        self.revalidate()?;
        let project = self
            .chain
            .last()
            .context("source authorization has no project")?;
        let birth = project
            .file
            .metadata()
            .and_then(|facts| facts.created())
            .context("authorized source creation time is unavailable")?
            .duration_since(UNIX_EPOCH)
            .context("authorized source creation time is invalid")?;
        ensure!(
            !birth.is_zero(),
            "authorized source creation time is unavailable"
        );

        let path = self.path.as_os_str().as_bytes();
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:workspace-source-identity:v1\0");
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path);
        digest.update((self.boundary_index as u64).to_be_bytes());
        digest.update((self.chain.len() as u64).to_be_bytes());
        for directory in &self.chain {
            digest.update(directory.identity.device.to_be_bytes());
            digest.update(directory.identity.inode.to_be_bytes());
        }
        digest.update(birth.as_secs().to_be_bytes());
        digest.update(birth.subsec_nanos().to_be_bytes());
        self.revalidate()?;
        Ok(hex::encode(digest.finalize()))
    }

    pub(crate) fn revalidate(&self) -> Result<()> {
        let first = self
            .chain
            .first()
            .context("source authorization has no root")?;
        let mut current = OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORY_FLAGS)
            .open("/")
            .context("cannot revalidate source filesystem root")?;
        ensure!(
            Identity::directory(&current)? == first.identity
                && Identity::directory(&first.file)? == first.identity,
            "authorized source filesystem root changed"
        );
        for (index, directory) in self.chain.iter().enumerate().skip(1) {
            let parent = &self.chain[index - 1];
            let name = directory
                .name
                .as_deref()
                .context("source directory has no component")?;
            // Recoup both the current absolute mapping and the retained parent
            // mapping. The latter includes the exact allowlist boundary FD.
            current = open_directory(&current, name)?;
            let retained_child = open_directory(&parent.file, name)?;
            let current_parent = open_directory(&directory.file, OsStr::new(".."))?;
            ensure!(
                Identity::directory(&directory.file)? == directory.identity
                    && Identity::directory(&current)? == directory.identity
                    && Identity::directory(&retained_child)? == directory.identity
                    && Identity::directory(&current_parent)? == parent.identity,
                "authorized source path or ancestry changed; import refused"
            );
        }
        ensure!(
            self.boundary_index < self.chain.len(),
            "source authorization lost its allowlist boundary"
        );
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn check_export_ancestry(&self) -> Result<()> {
        self.revalidate()
            .context("export parent path or ancestry changed; export refused")?;
        for directory in &self.chain {
            crate::control_fs::check_export_directory(&directory.file)?;
        }
        // Reopened paths are observations only. All creation/publication
        // authority remains in the retained chain, even after a failed check.
        self.revalidate()
            .context("export parent path or ancestry changed during permission checks")
    }

    pub(crate) fn prepare(
        &self,
        limits: Limits,
        excluded_roots: &[&File],
    ) -> Result<PreparedWorkspace> {
        self.revalidate()?;
        let project = self
            .chain
            .last()
            .context("source authorization has no project")?;
        let prepared =
            collect_workspace_from_root(&self.path, &project.file, limits, excluded_roots);
        // Reject a substitution even if pathname enumeration and the anchored
        // content reads happened to have compatible entry names. Revalidate on
        // error too, without accepting or publishing a partial archive.
        self.revalidate()?;
        prepared
    }
}

/// Export-only namespace capability. Keep this through publication and its
/// durability barrier. Other UIDs must not be able to replace its entries;
/// same-UID/root and storage-administrator mutation remains outside the policy.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) struct ExportAncestry {
    anchor: AuthorizedSource,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl ExportAncestry {
    /// Legacy path entry point: authorize a normalized, no-symlink chain once,
    /// then retain it rather than resolving a publication authority again.
    pub(super) fn open(parent: &Path) -> Result<Self> {
        let parent =
            crate::control_fs::absolute_path(parent).context("invalid export parent path")?;
        let anchor = AuthorizedSource::open(&parent, std::slice::from_ref(&parent))
            .context("cannot retain export parent ancestry")?;
        let retained = Self { anchor };
        retained.revalidate()?;
        Ok(retained)
    }

    fn from_retained(anchor: &AuthorizedSource) -> Result<Self> {
        anchor.check_export_ancestry()?;
        let chain = anchor
            .chain
            .iter()
            .map(|directory| {
                Ok(RetainedDirectory {
                    file: directory
                        .file
                        .try_clone()
                        .context("cannot clone retained export ancestry descriptor")?,
                    identity: directory.identity,
                    name: directory.name.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let retained = Self {
            anchor: AuthorizedSource {
                path: anchor.path.clone(),
                chain,
                boundary_index: anchor.boundary_index,
            },
        };
        retained.revalidate()?;
        Ok(retained)
    }

    pub(super) fn revalidate(&self) -> Result<()> {
        self.anchor.check_export_ancestry()
    }

    pub(super) fn try_clone_parent(&self) -> Result<File> {
        self.revalidate()?;
        self.anchor
            .chain
            .last()
            .context("export ancestry has no parent")?
            .file
            .try_clone()
            .context("cannot clone retained export parent descriptor")
    }

    /// Provenance for the caller, never a replacement creation authority.
    pub(super) fn path(&self) -> &Path {
        self.anchor.path()
    }
}

/// A retained export parent under policy roots checked for lexical and observed
/// physical overlap with every source policy root. Each policy root must remain
/// identifiable; an individual project underneath it need not still exist.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) struct AuthorizedExportParent {
    anchor: AuthorizedSource,
    source_roots: Vec<AuthorizedSource>,
    export_roots: Vec<AuthorizedSource>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl AuthorizedExportParent {
    pub(crate) fn identity(&self) -> Result<(u64, u64)> {
        self.revalidate()?;
        let parent = self
            .anchor
            .chain
            .last()
            .context("export authorization has no parent")?;
        let identity = Identity::directory(&parent.file)?;
        Ok((identity.device, identity.inode))
    }
    pub(crate) fn open(
        parent: &Path,
        allowed_export_roots: &[PathBuf],
        forbidden_source_roots: &[PathBuf],
    ) -> Result<Self> {
        let export_roots = allowed_export_roots
            .iter()
            .map(|root| crate::control_fs::absolute_path(root))
            .collect::<Result<Vec<_>>>()?;
        let source_roots = forbidden_source_roots
            .iter()
            .map(|root| crate::control_fs::absolute_path(root))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            export_roots.iter().all(|export| source_roots
                .iter()
                .all(|source| !export.starts_with(source) && !source.starts_with(export))),
            "export and source policy roots must be disjoint"
        );
        let anchor = AuthorizedSource::open(parent, &export_roots)
            .context("cannot authorize export parent")?;
        let source_roots = source_roots
            .iter()
            .map(|root| {
                AuthorizedSource::open(root, std::slice::from_ref(root)).context(
                    "configured source policy root is unavailable or unsafe; export disjointness cannot be verified",
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let export_roots = export_roots
            .iter()
            .map(|root| {
                AuthorizedSource::open(root, std::slice::from_ref(root)).context(
                    "configured export policy root is unavailable or unsafe; export disjointness cannot be verified",
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let authorized = Self {
            anchor,
            source_roots,
            export_roots,
        };
        authorized.revalidate()?;
        Ok(authorized)
    }

    pub(crate) fn path(&self) -> &Path {
        self.anchor.path()
    }

    pub(crate) fn revalidate(&self) -> Result<()> {
        for root in &self.source_roots {
            root.revalidate().context(
                "configured source policy root changed or became unavailable; export disjointness cannot be verified",
            )?;
        }
        for root in &self.export_roots {
            root.revalidate().context(
                "configured export policy root changed or became unavailable; export disjointness cannot be verified",
            )?;
        }
        self.anchor
            .check_export_ancestry()
            .context("export parent namespace is no longer safe; export refused")?;
        for source in &self.source_roots {
            for export in &self.export_roots {
                require_physically_disjoint(source, export)?;
            }
            // Check the requested parent as well as all configured roots: an
            // alias deeper underneath an export root may lead back to a source.
            require_physically_disjoint(source, &self.anchor)?;
        }
        Ok(())
    }

    pub(super) fn retained_ancestry(&self) -> Result<ExportAncestry> {
        self.revalidate()?;
        ExportAncestry::from_retained(&self.anchor)
    }

    // Closed authorized workspace extraction; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn extract<R: Read>(
        &self,
        reader: R,
        name: &OsStr,
        limits: Limits,
    ) -> Result<Manifest> {
        Ok(self.extract_with_report(reader, name, limits)?.manifest)
    }

    pub(crate) fn extract_with_report<R: Read>(
        &self,
        reader: R,
        name: &OsStr,
        limits: Limits,
    ) -> Result<ExportReport> {
        self.revalidate()?;
        let parent = self
            .anchor
            .chain
            .last()
            .context("export authorization has no parent")?;
        anchored_export::require_new_destination(&parent.file, name)?;
        // Clone the authorized capability, never reopen its pathname to choose
        // a new creation authority after policy validation.
        let mut staging = anchored_export::Staging::new_with_ancestry(self.retained_ancestry()?)?;
        let report = read_workspace_archive(reader, limits, Some(&staging))?;
        self.revalidate()?;
        staging
            .publish(name, self.path())
            .context("cannot publish authorized export")?;
        Ok(report)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn require_physically_disjoint(left: &AuthorizedSource, right: &AuthorizedSource) -> Result<()> {
    let left_root = left
        .chain
        .last()
        .context("source policy root has no retained identity")?;
    let right_root = right
        .chain
        .last()
        .context("export policy root has no retained identity")?;
    // Common ancestors such as / are expected. Overlap means that one whole
    // policy root is an ancestor of the other, including an identity reached
    // under a differently cased name or an accessible mount alias. A mount can
    // hide underlying ancestors; this inspection does not prove disjointness
    // against every privileged mount topology or concurrent administrator.
    ensure!(
        !right
            .chain
            .iter()
            .any(|directory| directory.identity == left_root.identity)
            && !left
                .chain
                .iter()
                .any(|directory| directory.identity == right_root.identity),
        "export and source policy roots physically overlap; export refused"
    );
    Ok(())
}

fn append_directories(chain: &mut Vec<RetainedDirectory>, path: &Path) -> Result<()> {
    for component in path.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("authorized source path has an invalid component");
        };
        ensure!(
            chain.len() <= MAX_SOURCE_DEPTH,
            "authorized source ancestry exceeds the supported bound"
        );
        let parent = chain.last().context("source authorization has no parent")?;
        let file = open_directory(&parent.file, name)?;
        let identity = Identity::directory(&file)?;
        chain.push(RetainedDirectory {
            file,
            identity,
            name: Some(name.to_owned()),
        });
    }
    Ok(())
}

fn open_directory(parent: &File, name: &OsStr) -> Result<File> {
    let name = CString::new(name.as_bytes()).context("source component contains NUL")?;
    // SAFETY: parent owns a live directory FD; name is a single normalized
    // component (or the fixed internal parent lookup), and flags deny symlinks.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | DIRECTORY_FLAGS,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("authorized source directory changed or contains a symlink; import refused");
    }
    // SAFETY: successful openat returned a new uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod authorized_workspace_tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize().expect("fixture path exists")
    }

    fn write_project(root: &Path) -> PathBuf {
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.txt"), b"synthetic project content\n").unwrap();
        project
    }

    #[test]
    fn authorized_source_identity_distinguishes_copies_and_path_replacement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let original_path = write_project(&root.join("original"));
        let copy_path = root.join("copy");
        fs::create_dir(&copy_path)?;
        fs::copy(
            original_path.join("README.txt"),
            copy_path.join("README.txt"),
        )?;
        let allowed = std::slice::from_ref(&root);

        let original_hash = AuthorizedSource::open(&original_path, allowed)?.identity_hash()?;
        let original = AuthorizedSource::open(&original_path, allowed)?;
        assert_eq!(original.identity_hash()?, original_hash);
        let copied = AuthorizedSource::open(&copy_path, allowed)?;
        let original_manifest = original.prepare(Limits::default(), &[])?.manifest;
        assert_eq!(
            copied.prepare(Limits::default(), &[])?.manifest,
            original_manifest
        );
        assert_ne!(copied.identity_hash()?, original_hash);

        let retired_path = root.join("retired");
        fs::rename(&original_path, &retired_path)?;
        let retired = AuthorizedSource::open(&retired_path, allowed)?;
        assert_ne!(retired.identity_hash()?, original_hash);

        fs::rename(&copy_path, &original_path)?;
        assert!(original.identity_hash().is_err());
        let replacement = AuthorizedSource::open(&original_path, allowed)?;
        assert_eq!(
            replacement.prepare(Limits::default(), &[])?.manifest,
            original_manifest
        );
        assert_ne!(replacement.identity_hash()?, original_hash);
        Ok(())
    }

    struct ExportParentFixture {
        _temp: tempfile::TempDir,
        sources: PathBuf,
        exports: PathBuf,
        ancestor: PathBuf,
        parent: PathBuf,
    }

    fn export_parent_fixture() -> ExportParentFixture {
        let temp = tempfile::tempdir().unwrap();
        let base = canonical(temp.path());
        let sources = base.join("sources");
        // This ancestor is above the allowlisted export root: validating only
        // the root and its descendants would leave this namespace mutable.
        let ancestor = base.join("ancestor");
        let exports = ancestor.join("exports");
        let parent = exports.join("parent");
        for path in [&sources, &ancestor, &exports, &parent] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        ExportParentFixture {
            _temp: temp,
            sources,
            exports,
            ancestor,
            parent,
        }
    }

    fn authorize_export_fixture(fixture: &ExportParentFixture) -> Result<AuthorizedExportParent> {
        AuthorizedExportParent::open(
            &fixture.parent,
            std::slice::from_ref(&fixture.exports),
            std::slice::from_ref(&fixture.sources),
        )
    }

    fn assert_export_namespace_unchanged(
        fixture: &ExportParentFixture,
        parent_mode: u32,
        ancestor_mode: u32,
    ) {
        assert_eq!(
            fs::metadata(&fixture.parent).unwrap().permissions().mode() & 0o7777,
            parent_mode,
            "authorization must not repair the export parent's permissions"
        );
        assert_eq!(
            fs::metadata(&fixture.ancestor)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            ancestor_mode,
            "authorization must not repair an export ancestor's permissions"
        );
        assert_eq!(
            fs::read_dir(&fixture.parent).unwrap().count(),
            0,
            "authorization must not create a staging or destination entry"
        );
        assert_eq!(fs::read_dir(&fixture.ancestor).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&fixture.exports).unwrap().count(), 1);
    }

    #[test]
    fn authorized_export_rejects_world_writable_parent_without_mutation() {
        let fixture = export_parent_fixture();
        fs::set_permissions(&fixture.parent, fs::Permissions::from_mode(0o777)).unwrap();

        let result = authorize_export_fixture(&fixture);

        assert_export_namespace_unchanged(&fixture, 0o777, 0o755);
        assert!(
            result.is_err(),
            "an allowlisted nonsticky parent writable by another UID must be refused"
        );
    }

    fn reject_mutable_export_ancestor(mode: u32) {
        let fixture = export_parent_fixture();
        fs::set_permissions(&fixture.parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&fixture.ancestor, fs::Permissions::from_mode(mode)).unwrap();

        let result = authorize_export_fixture(&fixture);

        assert_export_namespace_unchanged(&fixture, 0o700, mode);
        assert!(
            result.is_err(),
            "a private export parent does not make its mutable ancestor trustworthy"
        );
    }

    #[test]
    fn authorized_export_rejects_group_writable_ancestor_without_mutation() {
        reject_mutable_export_ancestor(0o775);
    }

    #[test]
    fn authorized_export_rejects_world_writable_ancestor_without_mutation() {
        reject_mutable_export_ancestor(0o777);
    }

    #[test]
    fn authorized_export_accepts_private_or_nonwritable_parent_without_mutation() {
        for mode in [0o700, 0o755] {
            let fixture = export_parent_fixture();
            fs::set_permissions(&fixture.parent, fs::Permissions::from_mode(mode)).unwrap();

            let export = authorize_export_fixture(&fixture)
                .expect("a safely owned parent without group/other writes is allowed");
            export.revalidate().unwrap();

            assert_export_namespace_unchanged(&fixture, mode, 0o755);
        }
    }

    #[test]
    fn authorized_export_revalidate_rejects_new_parent_write_permissions() {
        let fixture = export_parent_fixture();
        let export = authorize_export_fixture(&fixture).unwrap();
        fs::set_permissions(&fixture.parent, fs::Permissions::from_mode(0o777)).unwrap();

        let result = export.revalidate();

        assert_export_namespace_unchanged(&fixture, 0o777, 0o755);
        assert!(
            result.is_err(),
            "a retained inode must lose export authority when its parent permissions become unsafe"
        );
    }

    #[test]
    fn authorized_export_revalidate_rejects_new_ancestor_write_permissions() {
        let fixture = export_parent_fixture();
        let export = authorize_export_fixture(&fixture).unwrap();
        fs::set_permissions(&fixture.ancestor, fs::Permissions::from_mode(0o775)).unwrap();

        let result = export.revalidate();

        assert_export_namespace_unchanged(&fixture, 0o755, 0o775);
        assert!(
            result.is_err(),
            "an unchanged ancestry mapping must still reject newly mutable ancestors"
        );
    }

    #[test]
    fn authorized_export_does_not_require_trusted_source_permissions() {
        let fixture = export_parent_fixture();
        fs::set_permissions(&fixture.sources, fs::Permissions::from_mode(0o777)).unwrap();

        let export = authorize_export_fixture(&fixture)
            .expect("untrusted source content does not grant mutation of the export namespace");
        export.revalidate().unwrap();

        assert_export_namespace_unchanged(&fixture, 0o755, 0o755);
        assert_eq!(
            fs::metadata(&fixture.sources).unwrap().permissions().mode() & 0o7777,
            0o777,
            "export policy must not repair or impose private-control modes on source roots"
        );
    }

    // Mirror the existing workspace archive fixture writer so unsafe paths
    // reach the product parser instead of being normalized by tar::Builder.
    fn raw_archive(path: &str, kind: u8, contents: &[u8]) -> Vec<u8> {
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append(&header, contents).unwrap();
        archive.into_inner().unwrap()
    }

    #[test]
    fn authorized_source_requires_allowlisted_root_and_rejects_relocated_symlink_mapping() {
        let temp = tempfile::tempdir().unwrap();
        let base = canonical(temp.path());
        let allowed = base.join("allowed");
        let project = write_project(&allowed);
        let outside = base.join("outside");
        let outside_project = write_project(&outside);
        fs::write(
            outside_project.join("outside-only-canary.txt"),
            b"SYNTHETIC_OUTSIDE_CANARY",
        )
        .unwrap();

        assert!(AuthorizedSource::open(&outside_project, std::slice::from_ref(&allowed)).is_err());

        let grant = AuthorizedSource::open(&project, std::slice::from_ref(&allowed)).unwrap();
        assert_eq!(grant.path(), project.as_path());
        let moved = base.join("allowed-moved");
        fs::rename(&allowed, &moved).unwrap();
        symlink(&outside, &allowed).unwrap();

        assert!(grant.revalidate().is_err());
        assert!(grant.prepare(Limits::default(), &[]).is_err());
        assert!(AuthorizedSource::open(&project, std::slice::from_ref(&allowed)).is_err());

        fs::remove_file(&allowed).unwrap();
        fs::rename(&moved, &allowed).unwrap();
        let restored = grant
            .prepare(Limits::default(), &[])
            .expect("restoring the original directory mapping restores its authority");
        assert!(restored.manifest.contains_key("README.txt"));
        assert!(!restored.manifest.contains_key("outside-only-canary.txt"));
        let mut archive = tar::Archive::new(fs::File::open(restored.archive.path()).unwrap());
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            assert!(!path.ends_with("outside-only-canary.txt"));
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert!(
                !bytes
                    .windows(b"SYNTHETIC_OUTSIDE_CANARY".len())
                    .any(|part| part == b"SYNTHETIC_OUTSIDE_CANARY")
            );
        }
        assert_eq!(
            fs::read(outside_project.join("outside-only-canary.txt")).unwrap(),
            b"SYNTHETIC_OUTSIDE_CANARY"
        );
    }

    #[test]
    fn authorized_export_restores_archive_after_source_removal_and_never_overwrites_destination() {
        let temp = tempfile::tempdir().unwrap();
        let base = canonical(temp.path());
        let sources = base.join("sources");
        let exports = base.join("exports");
        fs::create_dir(&sources).unwrap();
        fs::create_dir(&exports).unwrap();
        let project = write_project(&sources);
        fs::write(project.join("nested.txt"), b"synthetic nested content").unwrap();

        let source = AuthorizedSource::open(&project, std::slice::from_ref(&sources)).unwrap();
        let prepared = source.prepare(Limits::default(), &[]).unwrap();
        assert!(prepared.manifest.contains_key("README.txt"));
        assert!(prepared.manifest.contains_key("nested.txt"));

        let export = AuthorizedExportParent::open(
            &exports,
            std::slice::from_ref(&exports),
            std::slice::from_ref(&sources),
        )
        .unwrap();
        let authorized_identity = export.identity().unwrap();
        fs::remove_dir_all(&project).unwrap();
        let restored_manifest = export
            .extract(
                fs::File::open(prepared.archive.path()).unwrap(),
                OsStr::new("restored"),
                Limits::default(),
            )
            .expect("a missing original project does not invalidate a live export parent");
        let restored = exports.join("restored");
        assert_eq!(restored_manifest, prepared.manifest);
        assert_eq!(
            fs::read(restored.join("README.txt")).unwrap(),
            b"synthetic project content\n"
        );
        assert_eq!(
            fs::read(restored.join("nested.txt")).unwrap(),
            b"synthetic nested content"
        );
        assert_eq!(export.identity().unwrap(), authorized_identity);

        fs::write(
            restored.join("README.txt"),
            b"preserve existing destination",
        )
        .unwrap();
        assert!(
            export
                .extract(
                    fs::File::open(prepared.archive.path()).unwrap(),
                    OsStr::new("restored"),
                    Limits::default(),
                )
                .is_err()
        );
        assert_eq!(
            fs::read(restored.join("README.txt")).unwrap(),
            b"preserve existing destination"
        );
    }

    #[test]
    fn authorized_export_rejects_symlink_parent_and_missing_source_root_for_disjointness() {
        let temp = tempfile::tempdir().unwrap();
        let base = canonical(temp.path());
        let exports = base.join("exports");
        let sources = base.join("sources");
        fs::create_dir(&exports).unwrap();
        fs::create_dir(&sources).unwrap();
        let real_parent = exports.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        symlink(&real_parent, exports.join("symlink-parent")).unwrap();

        assert!(
            AuthorizedExportParent::open(
                &exports.join("symlink-parent"),
                std::slice::from_ref(&exports),
                std::slice::from_ref(&sources),
            )
            .is_err()
        );

        let missing_sources = base.join("configured-source-root-missing");
        let error = match AuthorizedExportParent::open(
            &exports,
            std::slice::from_ref(&exports),
            std::slice::from_ref(&missing_sources),
        ) {
            Ok(_) => {
                panic!("unavailable source policy root must prevent disjointness authorization")
            }
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("export disjointness cannot be verified"));
    }

    #[test]
    fn authorized_export_rejects_traversal_and_symlink_archives_with_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let base = canonical(temp.path());
        let sources = base.join("sources");
        let exports = base.join("exports");
        fs::create_dir(&sources).unwrap();
        fs::create_dir(&exports).unwrap();
        let export = AuthorizedExportParent::open(
            &exports,
            std::slice::from_ref(&exports),
            std::slice::from_ref(&sources),
        )
        .unwrap();
        let outside = base.join("escape.txt");
        fs::write(&outside, b"preserve outside fixture").unwrap();

        for (name, bytes) in [
            (
                "traversal",
                raw_archive("../escape.txt", b'0', b"overwrite attempt"),
            ),
            ("symlink", raw_archive("link", b'2', b"")),
        ] {
            assert!(
                export
                    .extract(
                        std::io::Cursor::new(bytes),
                        OsStr::new(name),
                        Limits::default(),
                    )
                    .is_err()
            );
            assert!(!exports.join(name).exists());
            assert_eq!(fs::read(&outside).unwrap(), b"preserve outside fixture");
            assert_eq!(fs::read_dir(&exports).unwrap().count(), 0);
        }
    }
}
