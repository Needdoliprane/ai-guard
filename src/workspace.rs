use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use walkdir::WalkDir;

#[cfg(unix)]
mod authorized;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use authorized::AuthorizedExportParent;
#[cfg(unix)]
pub(crate) use authorized::{AuthorizedSource, PreparedNamedWorkspace};
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod export_durability_tests;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_bytes: 128 * 1024 * 1024,
            max_file_bytes: 32 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprint {
    pub sha256: String,
    pub bytes: u64,
    /// Mode of the represented regular file, not a promise to execute it.
    /// Absent in historical manifests whose modes were not recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}
pub type Manifest = BTreeMap<String, Fingerprint>;

/// Observed publication result. Omission counts cover explicit archive entries
/// (files and directories); implicit parent directories are not archive entries.
pub struct ExportReport {
    pub manifest: Manifest,
    pub omitted_entries: u64,
}

/// Preserve the recorded coverage when checking a historical import. Extra
/// observed paths and all content facts remain present, so equality/hash checks
/// still reject them. This projection must not replace a fresh scan's facts.
pub(crate) fn manifest_at_recorded_coverage(recorded: &Manifest, observed: &Manifest) -> Manifest {
    observed
        .iter()
        .map(|(path, fingerprint)| {
            let mut fingerprint = fingerprint.clone();
            if recorded.get(path).is_some_and(|known| known.mode.is_none()) {
                fingerprint.mode = None;
            }
            (path.clone(), fingerprint)
        })
        .collect()
}

/// Publication happened, but its durable result could not be confirmed. The
/// destination is preserved and must be reconciled before another export.
#[derive(Debug)]
pub(crate) struct ExportPublicationUnknown;

impl std::fmt::Display for ExportPublicationUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "export publication outcome is unknown; destination preserved; reconcile before retry",
        )
    }
}
impl std::error::Error for ExportPublicationUnknown {}

// Bound both explicit and implicit directory depth before creating an entry.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const MAX_EXPORT_DEPTH: usize = 64;

pub struct PreparedWorkspace {
    pub archive: tempfile::NamedTempFile,
    pub manifest: Manifest,
    pub excluded: Vec<String>,
    pub excluded_count: usize,
    pub source: PathBuf,
    pub bytes: u64,
}

impl Limits {
    pub fn max_archive_bytes(self) -> usize {
        self.max_bytes
            .saturating_add((self.max_files as u64).saturating_mul(2048))
            .saturating_add(1024 * 1024)
            .min(usize::MAX as u64) as usize
    }
}

fn excluded_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        ".git",
        ".ssh",
        ".aws",
        ".azure",
        ".gnupg",
        ".kube",
        ".agentsandbox",
        "secrets",
        "mounted-secrets",
        "credential-snapshots",
        "credentials",
        "node_modules",
        "target",
        ".venv",
        "venv",
        ".npmrc",
        ".pypirc",
        ".netrc",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        ".ds_store",
    ]
    .contains(&name.as_str())
        || name == ".env"
        || name.starts_with(".env.")
        || name.starts_with("credentials.")
        || [".pem", ".key", ".p12", ".pfx", ".keystore"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn export_policy_name(name: &str) -> String {
    let mut folded = String::with_capacity(name.len());
    for character in name.chars().flat_map(char::to_lowercase) {
        match character {
            // Unicode folds with an ASCII result that lowercase alone misses.
            '\u{17f}' => folded.push('s'),
            '\u{df}' => folded.push_str("ss"),
            '\u{fb00}' => folded.push_str("ff"),
            '\u{fb01}' => folded.push_str("fi"),
            '\u{fb02}' => folded.push_str("fl"),
            '\u{fb03}' => folded.push_str("ffi"),
            '\u{fb04}' => folded.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => folded.push_str("st"),
            // HFS+ ignores these characters in path comparisons. Only use this
            // projection to match reserved names; ordinary Unicode is retained.
            '\u{200c}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{206a}'..='\u{206f}'
            | '\u{feff}' => {}
            character => folded.push(character),
        }
    }
    folded
}

// Share the export reserved-name projection at this narrow private boundary.
// Agent rules/commands and ordinary directories named audit remain source.
fn excluded_private_audit_path(path: &Path) -> bool {
    let mut claude = false;
    for component in path.components() {
        let Component::Normal(name) = component else {
            claude = false;
            continue;
        };
        let Some(name) = name.to_str() else {
            claude = false;
            continue;
        };
        let name = export_policy_name(name);
        if claude && name == "audit" {
            return true;
        }
        claude = name == ".claude";
    }
    false
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn excluded_export_path(path: &Path) -> bool {
    if excluded_private_audit_path(path) {
        return true;
    }
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let Some(name) = name.to_str() else {
            // validate_relative_path rejects this before the export policy.
            return true;
        };
        let name = export_policy_name(name);
        if excluded_name(&name)
            || [
                ".husky",
                ".envrc",
                ".idea",
                ".vscode",
                ".devcontainer",
                ".direnv",
                ".pre-commit-config.yaml",
                ".gitattributes",
                ".gitmodules",
                ".mise.toml",
                ".tool-versions",
            ]
            .contains(&name.as_str())
        {
            return true;
        }
    }
    false
}

/// Recognize Git directory shapes from validated archive metadata without
/// invoking Git or reading an exported configuration. Checking after the last
/// entry handles arbitrary tar ordering and implicit parent directories. This
/// conservative structural policy also rejects linked-worktree gitdirs whose
/// `commondir` would otherwise redirect discovery outside their directory.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn refuse_export_git_directories(
    entries: &BTreeMap<PathBuf, bool>,
    directories: &BTreeSet<PathBuf>,
) -> Result<()> {
    #[derive(Default)]
    struct Shape {
        head: bool,
        objects: bool,
        refs: bool,
        commondir: bool,
    }
    let parent_key = |path: &Path| {
        path.parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(export_policy_name(&name.to_string_lossy())),
                _ => None,
            })
            .collect::<PathBuf>()
    };
    let mut shapes = BTreeMap::<PathBuf, Shape>::new();
    for path in directories {
        if excluded_export_path(path) {
            continue;
        }
        let name = export_policy_name(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
        );
        match name.as_str() {
            "objects" => shapes.entry(parent_key(path)).or_default().objects = true,
            "refs" => shapes.entry(parent_key(path)).or_default().refs = true,
            _ => {}
        }
    }
    for (path, is_directory) in entries {
        if *is_directory || excluded_export_path(path) {
            continue;
        }
        let name = export_policy_name(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
        );
        match name.as_str() {
            "head" => shapes.entry(parent_key(path)).or_default().head = true,
            "commondir" => shapes.entry(parent_key(path)).or_default().commondir = true,
            _ => {}
        }
    }
    if shapes
        .values()
        .any(|shape| shape.head && (shape.commondir || (shape.objects && shape.refs)))
    {
        bail!("export contains a worker-authored Git directory; nothing published");
    }
    Ok(())
}

/// Read a project without following a symlink in any path component. The open
/// directory descriptors also keep a concurrent rename from escaping the root.
#[cfg(unix)]
fn open_beneath(root: &File, path: &Path) -> Result<File> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let mut directory = root.try_clone()?;
    let components: Vec<_> = path.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("invalid source path");
        };
        let name = CString::new(name.as_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if index + 1 < components.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: directory owns a live descriptor; name is a NUL-terminated
        // component. A successful descriptor is immediately owned by File.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("source changed or contains a symlink; import refused");
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

#[cfg(not(unix))]
fn open_beneath(_root: &File, _path: &Path) -> Result<File> {
    bail!("safe workspace import currently requires Linux, macOS, or WSL")
}

pub fn prepare_workspace(source: &Path, limits: Limits) -> Result<PreparedWorkspace> {
    prepare_workspace_excluding(source, limits, &[])
}

/// Exclude retained control directories by filesystem identity, including when
/// their pathname uses a case alias or their name is not a reserved directory.
pub(crate) fn prepare_workspace_excluding(
    source: &Path,
    limits: Limits,
    excluded_roots: &[&File],
) -> Result<PreparedWorkspace> {
    if fs::symlink_metadata(source)?.file_type().is_symlink() {
        bail!("the project root must not be a symlink");
    }
    let source = source
        .canonicalize()
        .context("project directory does not exist")?;
    if !source.is_dir() {
        bail!("project must be a directory");
    }
    #[cfg(unix)]
    let root = open_beneath(&File::open("/")?, source.strip_prefix("/")?)?;
    #[cfg(not(unix))]
    let root = File::open(&source)?;
    collect_workspace_from_root(&source, &root, limits, excluded_roots)
}

/// Collect using the descriptor selected by the caller, without reopening the
/// source pathname. Enumeration names are untrusted; every content read stays
/// beneath this retained descriptor. AuthorizedSource also revalidates the
/// pathname mapping and its directory ancestry before accepting this archive.
fn collect_workspace_from_root(
    source: &Path,
    root: &File,
    limits: Limits,
    excluded_roots: &[&File],
) -> Result<PreparedWorkspace> {
    if !root.metadata()?.is_dir() {
        bail!("project must be a directory");
    }
    if excluded_private_audit_path(source) {
        bail!("project root is within a private Claude audit archive; choose a separate project");
    }
    #[cfg(unix)]
    let excluded_identities = {
        use std::os::unix::fs::MetadataExt;
        let mut identities = BTreeSet::new();
        for file in excluded_roots {
            let facts = file.metadata()?;
            if !facts.is_dir() {
                bail!("excluded control root must be a directory");
            }
            identities.insert((facts.dev(), facts.ino()));
        }
        identities
    };
    #[cfg(not(unix))]
    if !excluded_roots.is_empty() {
        bail!("descriptor-relative workspace exclusions require Unix");
    }
    #[cfg(unix)]
    if !excluded_identities.is_empty() {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::MetadataExt,
        };
        // A project inside the control root also contains control state; only
        // looking for an excluded child during traversal would miss this case.
        let mut ancestor = root.try_clone()?;
        let mut reached_root = false;
        for _ in 0..4096 {
            let facts = ancestor.metadata()?;
            let identity = (facts.dev(), facts.ino());
            if excluded_identities.contains(&identity) {
                bail!("project root is within a control directory; choose a separate project");
            }
            // This fixed parent lookup is for identity inspection only. Never
            // allow caller-supplied parent components in open_beneath.
            // SAFETY: ancestor owns a directory FD and the C name is constant.
            let fd = unsafe {
                libc::openat(
                    ancestor.as_raw_fd(),
                    c"..".as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("project ancestry cannot be inspected for control exclusion");
            }
            // SAFETY: successful openat returned a new owned directory FD.
            let parent = unsafe { File::from_raw_fd(fd) };
            let parent_facts = parent.metadata()?;
            if (parent_facts.dev(), parent_facts.ino()) == identity {
                reached_root = true;
                break;
            }
            ancestor = parent;
        }
        if !reached_root {
            bail!("project ancestry exceeds the supported control-exclusion bound");
        }
    }
    let mut archive = tempfile::NamedTempFile::new()?;
    let mut builder = tar::Builder::new(archive.as_file_mut());
    let mut manifest = Manifest::new();
    let mut excluded = Vec::new();
    let mut excluded_count = 0;
    let mut bytes = 0_u64;
    let mut entries = 0_usize;
    let mut walker = WalkDir::new(source)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.context("cannot enumerate project safely")?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = validate_relative_path(entry.path().strip_prefix(source)?)?;
        let name = entry
            .file_name()
            .to_str()
            .context("project filename is not UTF-8")?;
        if excluded_name(name) || excluded_private_audit_path(&relative) {
            excluded_count += 1;
            if excluded.len() < 256 {
                excluded.push(relative.to_string_lossy().into_owned());
            }
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if entry.file_type().is_symlink() {
            bail!("source symlink refused: {}", relative.display());
        }
        if !entry.file_type().is_file() && !entry.file_type().is_dir() {
            bail!("special source file refused: {}", relative.display());
        }
        let mut file = open_beneath(root, &relative)?;
        let metadata = file.metadata()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.is_dir() && excluded_identities.contains(&(metadata.dev(), metadata.ino()))
            {
                excluded_count += 1;
                if excluded.len() < 256 {
                    excluded.push(relative.to_string_lossy().into_owned());
                }
                walker.skip_current_dir();
                continue;
            }
        }
        entries += 1;
        if entries > limits.max_files {
            bail!("workspace exceeds the {} entry limit", limits.max_files);
        }
        let mut header = tar::Header::new_gnu();
        header.set_uid(1000);
        header.set_gid(1000);
        header.set_mtime(0);
        if metadata.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            builder.append_data(&mut header, &relative, std::io::empty())?;
        } else if metadata.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() > 1 {
                    bail!("source hard link refused: {}", relative.display());
                }
            }
            if metadata.len() > limits.max_file_bytes {
                bail!("file exceeds size limit: {}", relative.display());
            }
            let mut contents = Vec::with_capacity(metadata.len() as usize);
            (&mut file)
                .take(limits.max_file_bytes + 1)
                .read_to_end(&mut contents)?;
            if contents.len() as u64 > limits.max_file_bytes {
                bail!("file grew beyond the size limit: {}", relative.display());
            }
            bytes = bytes
                .checked_add(contents.len() as u64)
                .context("workspace size overflow")?;
            if bytes > limits.max_bytes {
                bail!("workspace exceeds the total byte limit");
            }
            let mut mode = 0o644;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 != 0 {
                    mode = 0o755;
                }
            }
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(contents.len() as u64);
            header.set_mode(mode);
            builder.append_data(&mut header, &relative, contents.as_slice())?;
            manifest.insert(
                relative.to_string_lossy().into_owned(),
                Fingerprint {
                    sha256: hex::encode(Sha256::digest(&contents)),
                    bytes: contents.len() as u64,
                    mode: Some(mode),
                },
            );
        } else {
            bail!("special source file refused: {}", relative.display());
        }
    }
    builder.finish()?;
    drop(builder);
    archive.as_file_mut().flush()?;
    Ok(PreparedWorkspace {
        archive,
        manifest,
        excluded,
        excluded_count,
        source: source.to_owned(),
        bytes,
    })
}

pub fn validate_relative_path(path: &Path) -> Result<PathBuf> {
    let text = path.to_str().context("archive path must be UTF-8")?;
    if text.contains('\\') || text.contains(':') || text.contains('\0') || text.len() > 4096 {
        bail!("unsafe archive path");
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            _ => bail!("archive path escapes its destination"),
        }
    }
    Ok(result)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn extract_workspace<R: Read>(
    reader: R,
    destination: &Path,
    original: &Path,
    limits: Limits,
) -> Result<Manifest> {
    Ok(extract_workspace_with_report(reader, destination, original, limits)?.manifest)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn extract_workspace_with_report<R: Read>(
    reader: R,
    destination: &Path,
    original: &Path,
    limits: Limits,
) -> Result<ExportReport> {
    let original = original
        .canonicalize()
        .context("original project must still be identifiable")?;
    let name = destination
        .file_name()
        .context("export needs a new directory name")?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .context("export parent must already exist")?;
    let destination = parent.join(name);
    if destination.starts_with(&original) {
        bail!("export into the original project is forbidden; choose a separate directory");
    }
    if fs::symlink_metadata(&destination).is_ok() {
        bail!("export destination already exists; nothing overwritten");
    }
    // Keep the validated directory open. Every creation, cleanup and rename is
    // relative to these descriptors, never to the now-mutable parent pathname.
    let mut staging = anchored_export::Staging::new(&parent)?;
    let report = read_workspace_archive(reader, limits, Some(&staging))?;
    staging
        .publish(name, &parent)
        .context("cannot publish verified export")?;
    Ok(report)
}

/// Inspect a bounded runtime archive without writing its contents on the host.
/// Recovery uses the immutable workspace volume, so this operation must not
/// depend on the continued existence of the original project directory.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn scan_workspace_archive<R: Read>(reader: R, limits: Limits) -> Result<Manifest> {
    Ok(read_workspace_archive(reader, limits, None)?.manifest)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_workspace_archive<R: Read>(
    reader: R,
    limits: Limits,
    staging: Option<&anchored_export::Staging>,
) -> Result<ExportReport> {
    let mut archive = tar::Archive::new(reader.take(limits.max_archive_bytes() as u64));
    let mut seen = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut manifest = Manifest::new();
    let mut total = 0_u64;
    let mut entries = 0_usize;
    let mut omitted_entries = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry.context("invalid workspace archive")?;
        let path = validate_relative_path(&entry.path()?)?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            bail!("archive links, sparse files, and special files are forbidden");
        }
        if path.as_os_str().is_empty() {
            if kind.is_dir() {
                continue;
            }
            bail!("empty archive filename");
        }
        entries += 1;
        if entries > limits.max_files {
            bail!("export exceeds the entry limit");
        }
        if path.components().count() > MAX_EXPORT_DEPTH {
            bail!("export exceeds the path depth limit");
        }
        let directory = if kind.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or_else(|| Path::new(""))
        };
        // A single tar header can imply many parents. Preflight the complete
        // entry before its first mkdir, for scanner/extractor parity.
        for parent in directory
            .ancestors()
            .filter(|path| !path.as_os_str().is_empty())
        {
            if !directories.contains(parent) && directories.len() >= limits.max_files {
                bail!("export exceeds the directory limit including implicit parents");
            }
            directories.insert(parent.to_path_buf());
        }
        if seen.insert(path.clone(), kind.is_dir()).is_some() {
            bail!("duplicate archive path: {}", path.display());
        }
        // Extraction also rejects these conflicts through filesystem creation.
        // Validate explicitly so read-only scans enforce the same archive shape.
        if path
            .ancestors()
            .skip(1)
            .any(|parent| seen.get(parent) == Some(&false))
            || (!kind.is_dir()
                && seen
                    .range((
                        std::ops::Bound::Excluded(path.clone()),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
                    .is_some_and(|(next, _)| next.starts_with(&path)))
        {
            bail!("archive file conflicts with a directory path");
        }
        // Filtering must not bypass archive validation or quota accounting.
        // Read-only scans retain every observed file, including these names.
        let omitted = staging.is_some() && excluded_export_path(&path);
        if omitted {
            omitted_entries = omitted_entries
                .checked_add(1)
                .context("export omitted-entry count overflow")?;
        }
        if kind.is_dir() {
            if let Some(staging) = staging.filter(|_| !omitted) {
                staging.create_directory(&path)?;
            }
            continue;
        }
        let size = entry.size();
        if size > limits.max_file_bytes {
            bail!("exported file exceeds the byte limit");
        }
        total = total.checked_add(size).context("export size overflow")?;
        if total > limits.max_bytes {
            bail!("export exceeds the total byte limit");
        }
        let mut file = staging
            .filter(|_| !omitted)
            .map(|staging| staging.create_file(&path))
            .transpose()?;
        // Validate the untrusted header, but never restore its executable bits.
        let observed_mode = entry.header().mode()? & 0o7777;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 16 * 1024];
        let mut written = 0_u64;
        loop {
            let len = entry.read(&mut buffer)?;
            if len == 0 {
                break;
            }
            written += len as u64;
            if written > size {
                bail!("archive entry exceeds its declared size");
            }
            hasher.update(&buffer[..len]);
            if let Some(file) = &mut file {
                file.write_all(&buffer[..len])?;
            }
        }
        if written != size {
            bail!("truncated archive entry");
        }
        if let Some(file) = &mut file {
            use std::os::unix::fs::PermissionsExt;
            file.flush()?;
            #[cfg(test)]
            export_durability_tests::record(
                export_durability_tests::Stage::FileFlushed,
                &path,
                file,
            );
            file.set_permissions(fs::Permissions::from_mode(0o644))?;
            #[cfg(test)]
            export_durability_tests::record(
                export_durability_tests::Stage::FileModeApplied,
                &path,
                file,
            );
            file.sync_all()
                .context("cannot synchronize exported file data and final mode")?;
            #[cfg(test)]
            export_durability_tests::after_barrier(
                export_durability_tests::Stage::FileSynced,
                &path,
                file,
            )?;
        }
        if !omitted {
            manifest.insert(
                path.to_string_lossy().into_owned(),
                Fingerprint {
                    sha256: hex::encode(hasher.finalize()),
                    bytes: size,
                    mode: Some(if staging.is_some() {
                        0o644
                    } else {
                        observed_mode
                    }),
                },
            );
        }
    }
    if staging.is_some() {
        refuse_export_git_directories(&seen, &directories)?;
        // No worker-controlled names or contents reach the diagnostic. Failure
        // to report the policy aborts before either export caller publishes.
        writeln!(
            std::io::stderr().lock(),
            "Export contents remain untrusted: skipped {omitted_entries} policy-blocked archive entries; executable bits are removed. Review the content before using it."
        )
        .context("cannot report export safety policy")?;
    }
    Ok(ExportReport {
        manifest,
        omitted_entries,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn extract_workspace<R: Read>(
    _reader: R,
    _destination: &Path,
    _original: &Path,
    _limits: Limits,
) -> Result<Manifest> {
    bail!("descriptor-anchored export currently requires Linux, macOS, or WSL")
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn extract_workspace_with_report<R: Read>(
    _reader: R,
    _destination: &Path,
    _original: &Path,
    _limits: Limits,
) -> Result<ExportReport> {
    bail!("descriptor-anchored export currently requires Linux, macOS, or WSL")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod anchored_export {
    use super::authorized::ExportAncestry;
    use super::*;
    use std::{
        cell::RefCell,
        ffi::{CStr, CString, OsStr},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct DirectoryIdentity {
        device: u64,
        inode: u64,
    }
    impl DirectoryIdentity {
        fn of(file: &File) -> Result<Self> {
            let metadata = file.metadata()?;
            if !metadata.is_dir() {
                bail!("export directory changed type");
            }
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
    }

    pub(super) struct Staging {
        ancestry: ExportAncestry,
        parent: File,
        directory: File,
        directories: RefCell<BTreeMap<PathBuf, DirectoryIdentity>>,
        name: CString,
        published: bool,
    }

    fn same_directory(left: &File, right: &File) -> Result<bool> {
        let left = left.metadata()?;
        let right = right.metadata()?;
        Ok(left.is_dir()
            && right.is_dir()
            && left.dev() == right.dev()
            && left.ino() == right.ino())
    }
    fn open_child_directory(parent: &File, name: &CStr) -> Result<File> {
        // SAFETY: parent owns its descriptor and name is a live C string.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("export directory changed or is a symlink");
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn target_name(target: &OsStr) -> Result<CString> {
        let mut components = Path::new(target).components();
        if !matches!(components.next(), Some(Component::Normal(name)) if name == target)
            || components.next().is_some()
        {
            bail!("export destination needs a single normal directory name");
        }
        CString::new(target.as_bytes()).context("export destination name contains NUL")
    }

    /// Check absence relative to the authorized parent, refusing every entry
    /// type and every inspection failure other than an actual missing entry.
    pub(super) fn require_new_destination(parent: &File, target: &OsStr) -> Result<()> {
        let target = target_name(target)?;
        let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: parent owns a live FD, target is one C component, and facts
        // is writable; AT_SYMLINK_NOFOLLOW also rejects dangling symlinks.
        let result = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                target.as_ptr(),
                facts.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            bail!("export destination already exists; nothing overwritten");
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOENT) {
            return Err(error).context("cannot safely inspect export destination");
        }
        Ok(())
    }

    impl Staging {
        pub(super) fn new(parent: &Path) -> Result<Self> {
            Self::new_with_ancestry(ExportAncestry::open(parent)?)
        }

        pub(super) fn new_with_ancestry(ancestry: ExportAncestry) -> Result<Self> {
            // Keep the checked ancestry, not only its last FD, until publication
            // and its parent barrier have both been confirmed.
            let parent = ancestry.try_clone_parent()?;
            let name = CString::new(format!(".agentsandbox-export-{}", uuid::Uuid::new_v4()))?;
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("cannot create private export staging");
            }
            // If opening fails, there is no retained staging identity: leave
            // the entry rather than unlinking a potentially replaced name.
            let directory = open_child_directory(&parent, &name)?;
            Ok(Self {
                ancestry,
                parent,
                directory,
                directories: RefCell::new(BTreeMap::new()),
                name,
                published: false,
            })
        }

        fn create_directory_beneath(&self, path: &Path) -> Result<File> {
            let mut directory = self.directory.try_clone()?;
            let mut relative = PathBuf::new();
            for component in path.components() {
                let Component::Normal(part) = component else {
                    bail!("invalid relative export directory");
                };
                relative.push(part);
                let expected = self.directories.borrow().get(&relative).copied();
                let name = CString::new(part.as_bytes())?;
                if expected.is_none()
                    && unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) } != 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("cannot create an exclusive export directory");
                }
                directory = open_child_directory(&directory, &name)?;
                let identity = DirectoryIdentity::of(&directory)?;
                if expected.is_some_and(|expected| expected != identity) {
                    bail!("export directory identity changed");
                }
                if expected.is_none() {
                    self.directories
                        .borrow_mut()
                        .insert(relative.clone(), identity);
                    #[cfg(test)]
                    export_durability_tests::record(
                        export_durability_tests::Stage::DirectoryCreated,
                        &relative,
                        &directory,
                    );
                }
            }
            Ok(directory)
        }

        fn reopen_directory(&self, path: &Path) -> Result<File> {
            let mut directory = self.directory.try_clone()?;
            let mut relative = PathBuf::new();
            for component in path.components() {
                let Component::Normal(part) = component else {
                    bail!("invalid tracked export directory");
                };
                relative.push(part);
                let expected = self
                    .directories
                    .borrow()
                    .get(&relative)
                    .copied()
                    .context("untracked export directory refused")?;
                let name = CString::new(part.as_bytes())?;
                directory = open_child_directory(&directory, &name)?;
                if DirectoryIdentity::of(&directory)? != expected {
                    bail!("tracked export directory identity changed");
                }
            }
            Ok(directory)
        }

        fn synchronize_directories(&self) -> Result<()> {
            // Tracked paths are bounded by the archive preflight. Reversing
            // their prefix order visits children before parents, without
            // enumerating the filesystem or retaining one FD per directory.
            let paths: Vec<_> = self.directories.borrow().keys().rev().cloned().collect();
            for path in paths {
                let directory = self.reopen_directory(&path)?;
                directory
                    .sync_all()
                    .context("cannot synchronize export directory")?;
                #[cfg(test)]
                export_durability_tests::after_barrier(
                    export_durability_tests::Stage::DirectorySynced,
                    &path,
                    &directory,
                )?;
            }
            self.directory
                .sync_all()
                .context("cannot synchronize export staging")?;
            #[cfg(test)]
            export_durability_tests::after_barrier(
                export_durability_tests::Stage::StagingSynced,
                Path::new(""),
                &self.directory,
            )?;
            Ok(())
        }

        pub(super) fn create_directory(&self, path: &Path) -> Result<()> {
            self.create_directory_beneath(path)?;
            Ok(())
        }
        pub(super) fn create_file(&self, path: &Path) -> Result<File> {
            let directory =
                self.create_directory_beneath(path.parent().unwrap_or_else(|| Path::new("")))?;
            let name = CString::new(
                path.file_name()
                    .context("empty export filename")?
                    .as_bytes(),
            )?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("cannot create an exclusive exported file");
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        pub(super) fn publish(&mut self, target: &OsStr, parent_path: &Path) -> Result<()> {
            let target = target_name(target)?;
            if parent_path != self.ancestry.path() {
                bail!("export parent provenance changed");
            }
            self.ancestry.revalidate()?;
            let current_staging = open_child_directory(&self.parent, &self.name)?;
            if !same_directory(&self.directory, &current_staging)? {
                bail!("export staging identity changed");
            }
            self.synchronize_directories()?;
            self.ancestry.revalidate()?;
            let current_staging = open_child_directory(&self.parent, &self.name)?;
            if !same_directory(&self.directory, &current_staging)? {
                bail!("export staging identity changed before publication");
            }
            // The identity check detects relocation for the caller; the dirfd
            // rename remains safe even if the pathname changes after that check.
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                    self.parent.as_raw_fd(),
                    target.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                    self.parent.as_raw_fd(),
                    target.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("atomic publication refused to replace the destination");
            }
            // Ownership changes at the successful kernel rename, before any
            // fallible barrier or test hook. Drop must never clear this tree.
            self.published = true;
            let confirm = || -> Result<()> {
                #[cfg(test)]
                export_durability_tests::after_barrier(
                    export_durability_tests::Stage::RenamedExclusive,
                    Path::new(OsStr::from_bytes(target.as_bytes())),
                    &self.directory,
                )?;
                self.parent
                    .sync_all()
                    .context("cannot synchronize export publication parent")?;
                #[cfg(test)]
                export_durability_tests::after_barrier(
                    export_durability_tests::Stage::ParentSynced,
                    Path::new(""),
                    &self.parent,
                )?;
                self.ancestry.revalidate()?;
                let destination = open_child_directory(&self.parent, &target)?;
                if !same_directory(&self.directory, &destination)? {
                    bail!("published export identity changed");
                }
                Ok(())
            };
            confirm().context(ExportPublicationUnknown)
        }
    }
    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    fn clear_directory(directory: &File) -> Result<()> {
        let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
        if duplicate < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            unsafe {
                libc::close(duplicate);
            }
            return Err(std::io::Error::last_os_error().into());
        }
        let stream = DirectoryStream(stream);
        loop {
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_owned();
            if name.as_bytes() == b"." || name.as_bytes() == b".." {
                continue;
            }
            if let Ok(child) = open_child_directory(directory, &name) {
                clear_directory(&child)?;
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            } else if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
    impl Drop for Staging {
        fn drop(&mut self) {
            if self.published {
                return;
            }
            // Never let a stale path-based TempDir destructor follow a replaced
            // parent into the original project. Cleanup uses the held fds too.
            // If ancestry rights/mappings became unsafe, preserve the staging
            // rather than unlink names in a namespace we can no longer trust.
            if self.ancestry.revalidate().is_err() {
                return;
            }
            if open_child_directory(&self.parent, &self.name)
                .ok()
                .and_then(|current| same_directory(&self.directory, &current).ok())
                != Some(true)
            {
                return;
            }
            if clear_directory(&self.directory).is_err()
                || self.ancestry.revalidate().is_err()
                || open_child_directory(&self.parent, &self.name)
                    .ok()
                    .and_then(|current| same_directory(&self.directory, &current).ok())
                    != Some(true)
            {
                return;
            }
            unsafe {
                libc::unlinkat(
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                    libc::AT_REMOVEDIR,
                );
            }
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    fn archive(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, kind, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(tar::EntryType::new(*kind));
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn assert_scan_and_export_reject(bytes: &[u8], limits: Limits) {
        assert!(scan_workspace_archive(bytes, limits).is_err());
        let original = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        assert!(extract_workspace(bytes, &destination, original.path(), limits).is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    #[test]
    fn scan_rejects_directory_file_collisions_with_export_parity() {
        for entries in [
            vec![("a", b'0', b"x".as_slice()), ("a/b", b'0', b"y".as_slice())],
            vec![("a/b", b'5', b"".as_slice()), ("a", b'0', b"x".as_slice())],
            vec![("a/b", b'0', b"y".as_slice()), ("a", b'0', b"x".as_slice())],
        ] {
            assert_scan_and_export_reject(&archive(&entries), Limits::default());
        }
    }

    #[test]
    fn scan_and_export_record_their_modes_without_writing_the_original() {
        // Directory headers may legitimately follow their contents.
        let bytes = archive(&[("nested/data", b'0', b"safe"), ("nested", b'5', b"")]);
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        fs::create_dir(&original).unwrap();
        let destination = parent.path().join("export");
        let exported =
            extract_workspace(bytes.as_slice(), &destination, &original, Limits::default())
                .unwrap();
        fs::rename(&original, parent.path().join("moved")).unwrap();
        let before = fs::read_dir(parent.path()).unwrap().count();
        let scanned = scan_workspace_archive(bytes.as_slice(), Limits::default()).unwrap();
        assert_eq!(
            scanned["nested/data"].sha256,
            exported["nested/data"].sha256
        );
        assert_eq!(scanned["nested/data"].bytes, exported["nested/data"].bytes);
        assert_eq!(scanned["nested/data"].mode, Some(0o755));
        assert_eq!(exported["nested/data"].mode, Some(0o644));
        assert_eq!(
            scanned["nested/data"].sha256,
            hex::encode(Sha256::digest(b"safe"))
        );
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), before);
        // Public export retains the independent original/destination guard.
        assert!(
            extract_workspace(
                bytes.as_slice(),
                &parent.path().join("other"),
                &original,
                Limits::default()
            )
            .is_err()
        );
        assert!(!parent.path().join("other").exists());
        assert!(
            extract_workspace(
                bytes.as_slice(),
                &parent.path().join("moved/inside"),
                &parent.path().join("moved"),
                Limits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn scan_and_export_share_unsafe_archive_and_limit_refusals() {
        for path in ["../escape", "/outside", "C:\\secret", "a\\..\\secret"] {
            assert_scan_and_export_reject(&archive(&[(path, b'0', b"x")]), Limits::default());
        }
        for kind in *b"12346S" {
            assert_scan_and_export_reject(&archive(&[("special", kind, b"")]), Limits::default());
        }
        assert_scan_and_export_reject(
            &archive(&[("a", b'0', b"x"), ("a", b'0', b"y")]),
            Limits::default(),
        );
        let bytes = archive(&[("a", b'0', b"xy"), ("b", b'0', b"z")]);
        for limits in [
            Limits {
                max_files: 1,
                ..Limits::default()
            },
            Limits {
                max_file_bytes: 1,
                ..Limits::default()
            },
            Limits {
                max_bytes: 2,
                ..Limits::default()
            },
        ] {
            assert_scan_and_export_reject(&bytes, limits);
        }
        let truncated = archive(&[("a", b'0', b"data")]);
        assert_scan_and_export_reject(&truncated[..513], Limits::default());
    }

    #[test]
    fn retained_control_directory_is_excluded_from_manifest_and_archive() {
        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        fs::create_dir(&project).unwrap();
        let control = project.join("control");
        fs::create_dir_all(control.join("credential-snapshots")).unwrap();
        fs::write(
            control.join("credential-snapshots/prepared"),
            b"PRIVATE_FIXTURE=not-for-import",
        )
        .unwrap();
        fs::write(control.join("authority"), b"private control evidence").unwrap();
        fs::write(project.join("business.txt"), b"business").unwrap();
        fs::create_dir(project.join("control-code")).unwrap();
        fs::write(project.join("control-code/business.txt"), b"source").unwrap();
        let guard = File::open(&control).unwrap();
        let prepared = prepare_workspace_excluding(
            &project,
            Limits {
                max_files: 3,
                ..Limits::default()
            },
            &[&guard],
        )
        .unwrap();
        assert_eq!(
            prepared
                .manifest
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["business.txt", "control-code/business.txt"]
        );
        assert!(prepared.excluded.iter().any(|path| path == "control"));
        let mut archive = tar::Archive::new(File::open(prepared.archive.path()).unwrap());
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            assert!(!entry.path().unwrap().starts_with("control"));
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            assert!(
                !data
                    .windows(b"PRIVATE_FIXTURE".len())
                    .any(|part| part == b"PRIVATE_FIXTURE")
            );
        }
        assert!(prepare_workspace_excluding(&control, Limits::default(), &[&guard]).is_err());
        assert!(
            prepare_workspace_excluding(
                &control.join("credential-snapshots"),
                Limits::default(),
                &[&guard]
            )
            .is_err()
        );
        let file = File::open(project.join("business.txt")).unwrap();
        assert!(prepare_workspace_excluding(&project, Limits::default(), &[&file]).is_err());
    }

    #[test]
    fn private_claude_audit_is_absent_from_import_but_agent_rules_are_preserved() {
        let project = tempfile::tempdir().unwrap();
        let project_path = project.path().canonicalize().unwrap();
        let private = [
            ".claude/audit/report.md",
            "nested/.CLAUDE/AUDIT/report.md",
            "aliases/.clau\u{200d}de/au\u{200d}dit/report.md",
        ];
        let kept = [
            "src/main.rs",
            "AGENTS.md",
            "CLAUDE.md",
            ".claude/rules/project.md",
            ".claude/commands/check.md",
            ".claude/audit-notes/README.md",
            "docs/audit/report.md",
        ];
        const CANARY: &[u8] = b"PRIVATE_AUDIT_SYNTHETIC_CANARY";
        for path in private {
            let path = project_path.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, CANARY).unwrap();
        }
        for path in kept {
            let path = project_path.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"ordinary source or agent rule").unwrap();
        }
        let prepared = prepare_workspace(&project_path, Limits::default()).unwrap();
        for path in private {
            assert!(
                !prepared.manifest.contains_key(path),
                "private import: {path}"
            );
            assert_eq!(fs::read(project_path.join(path)).unwrap(), CANARY);
        }
        for path in kept {
            assert!(
                prepared.manifest.contains_key(path),
                "useful source omitted: {path}"
            );
        }
        assert_eq!(prepared.manifest.len(), kept.len());
        assert_eq!(prepared.excluded_count, 3);
        let mut tar = tar::Archive::new(prepared.archive.reopen().unwrap());
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            assert!(!data.windows(CANARY.len()).any(|part| part == CANARY));
        }
        let authorized =
            AuthorizedSource::open(&project_path, std::slice::from_ref(&project_path)).unwrap();
        let anchored = authorized.prepare(Limits::default(), &[]).unwrap();
        assert_eq!(anchored.manifest, prepared.manifest);
        for path in private {
            let root = project_path.join(path).parent().unwrap().to_path_buf();
            let error = prepare_workspace(&root, Limits::default())
                .err()
                .expect("a private archive root cannot become a project");
            assert!(
                error.to_string().contains("private Claude audit archive"),
                "{error:#}"
            );
            let authorized =
                AuthorizedSource::open(&root, std::slice::from_ref(&project_path)).unwrap();
            let error = authorized
                .prepare(Limits::default(), &[])
                .err()
                .expect("the anchored collector must also refuse a private archive root");
            assert!(
                error.to_string().contains("private Claude audit archive"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn private_claude_audit_is_omitted_from_export_with_implicit_and_late_parents() {
        const CANARY: &[u8] = b"PRIVATE_AUDIT_SYNTHETIC_CANARY";
        let private = [
            ".claude/audit/report.md",
            "nested/.CLAUDE/AUDIT/report.md",
            "aliases/.clau\u{200d}de/au\u{200d}dit/report.md",
        ];
        let kept = [
            "src/main.rs",
            "AGENTS.md",
            "CLAUDE.md",
            ".claude/rules/project.md",
            ".claude/commands/check.md",
            ".claude/audit-notes/README.md",
            "docs/audit/report.md",
        ];
        let mut entries = Vec::new();
        for path in private {
            entries.push((path, b'0', CANARY));
        }
        for path in kept {
            entries.push((path, b'0', b"ordinary source or agent rule".as_slice()));
        }
        // Implicit parent paths must be filtered too; a late directory header
        // must not recreate the private directory after its files were omitted.
        entries.push((".claude/audit", b'5', b"".as_slice()));
        let bytes = archive(&entries);
        let observed = scan_workspace_archive(bytes.as_slice(), Limits::default()).unwrap();
        for path in private {
            assert!(
                observed.contains_key(path),
                "strict scan preserves observed archive facts"
            );
        }
        let original = tempfile::tempdir().unwrap();
        fs::write(original.path().join("original.txt"), b"unchanged original").unwrap();
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let report = extract_workspace_with_report(
            bytes.as_slice(),
            &destination,
            original.path(),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(report.omitted_entries, 4);
        assert_eq!(report.manifest.len(), kept.len());
        for path in private {
            assert!(
                !report.manifest.contains_key(path),
                "private export: {path}"
            );
            assert!(!destination.join(path).exists());
            assert!(!destination.join(path).parent().unwrap().exists());
        }
        for path in kept {
            assert!(
                report.manifest.contains_key(path),
                "useful source omitted: {path}"
            );
            assert_eq!(
                fs::read(destination.join(path)).unwrap(),
                b"ordinary source or agent rule"
            );
        }
        assert_eq!(
            fs::read(original.path().join("original.txt")).unwrap(),
            b"unchanged original"
        );
        assert_eq!(fs::read_dir(original.path()).unwrap().count(), 1);
    }

    #[test]
    fn control_exclusion_tracks_the_retained_directory_not_its_former_name() {
        let project = tempfile::tempdir().unwrap();
        let control = project.path().join("control");
        fs::create_dir(&control).unwrap();
        fs::write(control.join("private"), b"original").unwrap();
        let guard = File::open(&control).unwrap();
        fs::rename(&control, project.path().join("relocated")).unwrap();
        fs::create_dir(&control).unwrap();
        fs::write(control.join("ordinary-successor"), b"ordinary").unwrap();
        let prepared =
            prepare_workspace_excluding(project.path(), Limits::default(), &[&guard]).unwrap();
        assert!(!prepared.manifest.contains_key("relocated/private"));
        assert!(prepared.manifest.contains_key("control/ordinary-successor"));
        assert!(prepared.excluded.iter().any(|path| path == "relocated"));
    }

    #[test]
    fn external_control_root_does_not_change_the_project_manifest() {
        let project = tempfile::tempdir().unwrap();
        let control = tempfile::tempdir().unwrap();
        fs::write(project.path().join("data"), b"business").unwrap();
        let guard = File::open(control.path()).unwrap();
        let ordinary = prepare_workspace(project.path(), Limits::default()).unwrap();
        let excluded =
            prepare_workspace_excluding(project.path(), Limits::default(), &[&guard]).unwrap();
        assert_eq!(ordinary.manifest, excluded.manifest);
    }

    #[test]
    fn orphaned_credential_snapshots_are_excluded_with_another_control_root() {
        let project = tempfile::tempdir().unwrap();
        let current_control = tempfile::tempdir().unwrap();
        let old = project.path().join("previous-control/credential-snapshots");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("residual"), b"PRIVATE_FIXTURE=old-snapshot").unwrap();
        fs::write(project.path().join("business"), b"source").unwrap();
        let guard = File::open(current_control.path()).unwrap();
        let prepared =
            prepare_workspace_excluding(project.path(), Limits::default(), &[&guard]).unwrap();
        assert_eq!(prepared.manifest.len(), 1);
        assert!(prepared.manifest.contains_key("business"));
        assert!(
            prepared
                .excluded
                .iter()
                .any(|path| path == "previous-control/credential-snapshots")
        );
        let mut archive = tar::Archive::new(File::open(prepared.archive.path()).unwrap());
        assert!(archive.entries().unwrap().all(|entry| {
            !entry
                .unwrap()
                .path()
                .unwrap()
                .starts_with("previous-control/credential-snapshots")
        }));
    }

    #[test]
    fn publication_never_replaces_a_destination_created_after_validation() {
        let parent = tempfile::tempdir().unwrap();
        // Match extract_workspace: macOS's temporary directory may be below
        // /var, a symlink that the descriptor-anchored traversal must reject.
        let parent_path = parent.path().canonicalize().unwrap();
        let destination = parent_path.join("result");
        let mut staging = anchored_export::Staging::new(&parent_path).unwrap();
        staging
            .create_file(Path::new("data"))
            .unwrap()
            .write_all(b"safe")
            .unwrap();
        fs::create_dir(&destination).unwrap();
        assert!(
            staging
                .publish(std::ffi::OsStr::new("result"), &parent_path)
                .is_err()
        );
        drop(staging);
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
    }
}

#[derive(Debug, Serialize)]
pub struct FileChange {
    pub path: String,
    pub change: &'static str,
    pub before: Option<Fingerprint>,
    pub after: Option<Fingerprint>,
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod export_policy_regression_tests {
    use super::*;
    use std::{ffi::OsStr, os::unix::fs::PermissionsExt};

    fn scan_file(path: &str, mode: u32, data: &[u8]) -> Manifest {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        builder.append_data(&mut header, path, data).unwrap();
        let bytes = builder.into_inner().unwrap();
        scan_workspace_archive(bytes.as_slice(), Limits::default()).unwrap()
    }

    #[test]
    fn historical_manifest_serialization_preserves_absent_mode_and_hash() {
        const LEGACY: &str = concat!(
            "{\"tool\":{\"sha256\":\"",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "\",\"bytes\":4}}"
        );
        let recorded: Manifest = serde_json::from_str(LEGACY).unwrap();
        assert_eq!(recorded["tool"].mode, None);
        assert_eq!(serde_json::to_string(&recorded).unwrap(), LEGACY);
        // Golden SHA-256 of domain + NUL + the literal legacy JSON above.
        assert_eq!(
            crate::control::content_hash("ai-guard:workspace-manifest:v1", &recorded).unwrap(),
            "320af68ae02d702a0178b54d950f987332d3799d07888e840c3a68c48951f4c5"
        );
    }

    #[test]
    fn recorded_coverage_keeps_nonempty_legacy_imports_and_known_modes_exact() {
        let observed = scan_file("tool", 0o755, b"data");
        let mut legacy_value = serde_json::to_value(&observed).unwrap();
        legacy_value["tool"].as_object_mut().unwrap().remove("mode");
        let recorded: Manifest = serde_json::from_value(legacy_value).unwrap();
        assert!(!recorded.is_empty());
        assert_ne!(recorded, observed, "ordinary equality remains strict");
        assert!(diff_manifests(&recorded, &observed).is_empty());
        let aligned = manifest_at_recorded_coverage(&recorded, &observed);
        assert_eq!(aligned, recorded);
        assert_eq!(
            crate::control::content_hash("ai-guard:workspace-manifest:v1", &aligned).unwrap(),
            crate::control::content_hash("ai-guard:workspace-manifest:v1", &recorded).unwrap()
        );
        assert_eq!(observed["tool"].mode, Some(0o755));

        let mut added = observed.clone();
        added.insert("extra".into(), observed["tool"].clone());
        assert_ne!(manifest_at_recorded_coverage(&recorded, &added), recorded);
        assert_ne!(
            manifest_at_recorded_coverage(&recorded, &Manifest::new()),
            recorded
        );
        let changed = scan_file("tool", 0o755, b"edit");
        assert_ne!(manifest_at_recorded_coverage(&recorded, &changed), recorded);
        let different_mode = scan_file("tool", 0o644, b"data");
        assert_ne!(
            manifest_at_recorded_coverage(&observed, &different_mode),
            observed
        );
        assert_ne!(
            manifest_at_recorded_coverage(&observed, &recorded),
            observed
        );
        let lost = diff_manifests(&observed, &recorded);
        assert_eq!(lost.len(), 1, "known mode cannot silently become unknown");
        assert_eq!(lost[0].change, "modified");
    }

    #[test]
    fn imported_archive_scan_and_export_distinguish_recorded_modes() {
        let original = tempfile::tempdir().unwrap();
        let path = original.path().join("tool");
        fs::write(&path, b"data").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o4755)).unwrap();
        let prepared = prepare_workspace(original.path(), Limits::default()).unwrap();
        assert_eq!(prepared.manifest["tool"].mode, Some(0o755));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o4755
        );
        let mut archive = tar::Archive::new(prepared.archive.reopen().unwrap());
        let mut entries = archive.entries().unwrap();
        let entry = entries.next().unwrap().unwrap();
        assert_eq!(entry.header().mode().unwrap(), 0o755);
        assert!(entries.next().is_none());
        let scanned =
            scan_workspace_archive(prepared.archive.reopen().unwrap(), Limits::default()).unwrap();
        assert_eq!(scanned, prepared.manifest);
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let exported = extract_workspace_with_report(
            prepared.archive.reopen().unwrap(),
            &destination,
            original.path(),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(exported.omitted_entries, 0);
        assert_eq!(exported.manifest["tool"].mode, Some(0o644));
        assert_eq!(exported.manifest["tool"].sha256, scanned["tool"].sha256);
        assert_eq!(
            fs::metadata(destination.join("tool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o644
        );
    }

    #[test]
    fn scan_records_mode_only_changes_without_restoring_worker_modes() {
        let scan = |mode| {
            let mut builder = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(4);
            header.set_mode(mode);
            builder
                .append_data(&mut header, "tool", b"data".as_slice())
                .unwrap();
            let bytes = builder.into_inner().unwrap();
            scan_workspace_archive(bytes.as_slice(), Limits::default()).unwrap()
        };
        let before = scan(0o644);
        let after = scan(0o755);
        let changes = diff_manifests(&before, &after);
        assert_eq!(
            changes.len(),
            1,
            "observed chmod-only change must be visible"
        );
        assert_eq!(changes[0].path, "tool");
        assert_eq!(changes[0].change, "modified");
        assert_eq!(before["tool"].sha256, after["tool"].sha256);
        assert_eq!(before["tool"].bytes, after["tool"].bytes);
    }

    #[test]
    fn authorized_export_filters_host_metadata_while_scan_keeps_observed_files() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let sources = base.join("sources");
        let exports = base.join("exports");
        for path in [&sources, &exports] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data) in [
            (
                "./.git/config",
                b"[core]\nfsmonitor = fixture-command\n".as_slice(),
            ),
            (
                "./.git/hooks/pre-commit",
                b"inert fixture; never execute".as_slice(),
            ),
            ("nested/.envrc", b"inert fixture; never execute".as_slice()),
            ("nested/tool", b"ordinary source".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            builder.append_data(&mut header, path, data).unwrap();
        }
        let bytes = builder.into_inner().unwrap();
        let scanned = scan_workspace_archive(bytes.as_slice(), Limits::default()).unwrap();
        assert_eq!(scanned.len(), 4);
        assert!(scanned.contains_key(".git/config"));
        assert!(scanned.contains_key("nested/.envrc"));
        let parent = AuthorizedExportParent::open(
            &exports,
            std::slice::from_ref(&exports),
            std::slice::from_ref(&sources),
        )
        .unwrap();
        let report = parent
            .extract_with_report(bytes.as_slice(), OsStr::new("result"), Limits::default())
            .unwrap();
        assert_eq!(report.omitted_entries, 3);
        let manifest = report.manifest;
        let destination = exports.join("result");
        assert!(!destination.join(".git").exists());
        assert!(!destination.join("nested/.envrc").exists());
        assert_eq!(manifest.len(), 1);
        assert_eq!(
            manifest["nested/tool"].sha256,
            scanned["nested/tool"].sha256
        );
        assert_eq!(manifest["nested/tool"].bytes, scanned["nested/tool"].bytes);
        assert_eq!(manifest["nested/tool"].mode, Some(0o644));
        assert_eq!(scanned["nested/tool"].mode, Some(0o755));
        assert_eq!(
            fs::read(destination.join("nested/tool")).unwrap(),
            b"ordinary source"
        );
        assert_eq!(
            fs::metadata(destination.join("nested/tool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o644
        );
    }

    #[test]
    fn authorized_export_refuses_bare_git_before_publication_and_cleans_staging() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let sources = base.join("sources");
        let exports = base.join("exports");
        for path in [&sources, &exports] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let parent = AuthorizedExportParent::open(
            &exports,
            std::slice::from_ref(&exports),
            std::slice::from_ref(&sources),
        )
        .unwrap();
        for prefix in ["", "nested/repository/"] {
            let mut builder = tar::Builder::new(Vec::new());
            if prefix.is_empty() {
                for directory in ["objects", "refs"] {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    header.set_mode(0o755);
                    builder
                        .append_data(&mut header, directory, &[][..])
                        .unwrap();
                }
            }
            // The nested case has no directory headers. Both cases place HEAD
            // after every other entry, so policy cannot depend on tar order.
            for (name, data) in [
                ("objects/aa/object", b"inert object bytes".as_slice()),
                ("refs/heads/main", b"inert reference bytes".as_slice()),
                ("config", b"[core]\n\tbare = true\n".as_slice()),
                ("HEAD", b"ref: refs/heads/main\n".as_slice()),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                builder
                    .append_data(&mut header, format!("{prefix}{name}"), data)
                    .unwrap();
            }
            let bytes = builder.into_inner().unwrap();
            let scanned = scan_workspace_archive(bytes.as_slice(), Limits::default()).unwrap();
            assert_eq!(scanned.len(), 4, "valid archive reaches the export policy");
            assert!(scanned.contains_key(&format!("{prefix}HEAD")));
            let error = parent
                .extract_with_report(bytes.as_slice(), OsStr::new("result"), Limits::default())
                .err()
                .expect("worker Git directory must be refused");
            assert!(
                error.to_string().contains("worker-authored Git directory"),
                "refusal must come from export policy after valid authorization: {error:#}"
            );
            assert!(!exports.join("result").exists());
            assert_eq!(
                fs::read_dir(&exports).unwrap().count(),
                0,
                "refusal must not leave an orphan staging directory"
            );
        }
    }
}
pub fn diff_manifests(before: &Manifest, after: &Manifest) -> Vec<FileChange> {
    let paths: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let old = before.get(path);
            let new = after.get(path);
            let unchanged = match (old, new) {
                (Some(old), Some(new)) => {
                    old.sha256 == new.sha256
                        && old.bytes == new.bytes
                        && old.mode.is_none_or(|mode| new.mode == Some(mode))
                }
                (None, None) => true,
                _ => false,
            };
            if unchanged {
                return None;
            }
            Some(FileChange {
                path: path.clone(),
                change: if old.is_none() {
                    "created"
                } else if new.is_none() {
                    "deleted"
                } else {
                    "modified"
                },
                before: old.cloned(),
                after: new.cloned(),
            })
        })
        .collect()
}
