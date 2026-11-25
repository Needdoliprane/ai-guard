//! Private, versioned approval of the host CLI. This is not a worker capability.
use super::RuntimeKind;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationBindingV1 {
    pub policy_version: u32,
    pub kind: RuntimeKind,
    pub approved_entry: PathBuf,
    pub canonical_program: PathBuf,
    pub file_identity: ExecutableIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_directory: Option<ExecutableDirectoryIdentityV1>,
    pub environment: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutableIdentityV1 {
    pub device: u64,
    pub inode: u64,
    pub owner: u32,
    pub group: u32,
    pub mode: u32,
    pub length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutableDirectoryIdentityV1 {
    pub device: u64,
    pub inode: u64,
    pub owner: u32,
    pub group: u32,
    pub mode: u32,
}

pub(super) struct InvocationAnchor {
    directory: Arc<File>,
    basename: OsString,
}
impl std::fmt::Debug for InvocationAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationAnchor").finish_non_exhaustive()
    }
}

#[cfg(test)]
impl super::Runtime {
    /// Executor fixtures that must complete without any runtime invocation.
    pub(crate) fn unavailable_for_test(
        kind: RuntimeKind,
        root: &crate::control_fs::Directory,
    ) -> Result<Self> {
        Ok(Self {
            kind,
            invocation: InvocationBindingV1 {
                // The normal command path refuses this before the account or
                // filesystem validation. There is no approved executable.
                policy_version: 0,
                kind,
                approved_entry: PathBuf::new(),
                canonical_program: PathBuf::new(),
                file_identity: ExecutableIdentityV1 {
                    device: 0,
                    inode: 0,
                    owner: 0,
                    group: 0,
                    mode: 0,
                    length: 0,
                    sha256: String::new(),
                },
                executable_directory: None,
                environment: BTreeMap::new(),
                cwd: root.path().to_owned(),
            },
            anchor: Arc::new(InvocationAnchor {
                directory: Arc::new(root.try_clone_file()?),
                basename: OsString::new(),
            }),
            observer: None,
            observation: None,
            native_control: None,
            podman_compose_endpoint: None,
        })
    }
}

impl std::fmt::Debug for InvocationBindingV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationBindingV1")
            .field("policy_version", &self.policy_version)
            .field("kind", &self.kind)
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl InvocationBindingV1 {
    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"ai-guard:runtime-invocation:v1\0");
        let mut add = |bytes: &[u8]| {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        };
        add(&self.policy_version.to_be_bytes());
        add(self.kind.program().as_bytes());
        add(self.approved_entry.as_os_str().as_encoded_bytes());
        add(self.canonical_program.as_os_str().as_encoded_bytes());
        add(&self.file_identity.device.to_be_bytes());
        add(&self.file_identity.inode.to_be_bytes());
        add(&self.file_identity.owner.to_be_bytes());
        add(&self.file_identity.group.to_be_bytes());
        add(&self.file_identity.mode.to_be_bytes());
        add(&self.file_identity.length.to_be_bytes());
        add(self.file_identity.sha256.as_bytes());
        if let Some(directory) = &self.executable_directory {
            add(&directory.device.to_be_bytes());
            add(&directory.inode.to_be_bytes());
            add(&directory.owner.to_be_bytes());
            add(&directory.group.to_be_bytes());
            add(&directory.mode.to_be_bytes());
        }
        add(self.cwd.as_os_str().as_encoded_bytes());
        for (key, value) in &self.environment {
            add(key.as_bytes());
            add(value.as_bytes());
        }
        hex::encode(hash.finalize())
    }

    pub(super) fn approve(kind: RuntimeKind, entry: &Path) -> Result<Self> {
        validate_absolute(entry)?;
        let canonical_program = entry
            .canonicalize()
            .context("approved runtime program cannot be resolved")?;
        let cwd = canonical_program
            .parent()
            .context("runtime executable directory is unavailable")?
            .to_path_buf();
        let directory = open_directory(&cwd)?;
        let executable_directory = Some(identify_directory(&directory)?);
        let basename = canonical_program
            .file_name()
            .context("runtime executable basename is unavailable")?;
        let file_identity = identify(&directory, basename)?;
        Ok(Self {
            policy_version: 2,
            kind,
            approved_entry: entry.to_path_buf(),
            canonical_program,
            file_identity,
            executable_directory,
            environment: standard_environment()?,
            cwd,
        })
    }

    fn validate_policy(&self, kind: RuntimeKind) -> Result<()> {
        ensure!(
            self.policy_version == 2,
            "runtime invocation policy migration required: explicitly approve the descriptor-anchored policy and persist audit/checkpoint before effects"
        );
        ensure!(
            self.kind == kind,
            "runtime invocation kind differs from control selection"
        );
        ensure!(
            self.canonical_program.parent() == Some(self.cwd.as_path())
                && self.environment == standard_environment()?,
            "runtime invocation environment or anchored working directory differs from control policy"
        );
        validate_absolute(&self.approved_entry)?;
        validate_absolute(&self.canonical_program)?;
        validate_absolute(&self.cwd)?;
        ensure!(
            self.executable_directory.is_some(),
            "runtime invocation lacks its approved directory identity"
        );
        Ok(())
    }

    pub(super) fn pin(&self, kind: RuntimeKind) -> Result<InvocationAnchor> {
        self.validate_policy(kind)?;
        ensure!(
            self.approved_entry
                .canonicalize()
                .context("approved runtime entry is unavailable")?
                == self.canonical_program,
            "approved runtime entry changed; explicit reapproval required"
        );
        let anchor = InvocationAnchor {
            directory: Arc::new(open_directory(&self.cwd)?),
            basename: self
                .canonical_program
                .file_name()
                .context("runtime executable basename is unavailable")?
                .to_os_string(),
        };
        anchor.verify(self, kind)?;
        Ok(anchor)
    }

    pub(super) fn factory(kind: RuntimeKind) -> Result<Self> {
        for entry in installation_entries(kind) {
            match std::fs::symlink_metadata(&entry) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).context("runtime installation entry cannot be inspected");
                }
                Ok(_) => {
                    let approved = Self::approve(kind, &entry)?;
                    ensure!(
                        installed_target(kind, &approved.canonical_program),
                        "runtime installation symlink points outside approved installation roots; provide an explicit absolute runtime program"
                    );
                    return Ok(approved);
                }
            }
        }
        bail!(
            "container runtime is absent from approved installation locations; provide --runtime-program with an approved absolute executable; no PATH or local fallback exists"
        )
    }
}

fn validate_absolute(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            && path
                .to_str()
                .is_some_and(|text| !text.chars().any(char::is_control)),
        "runtime program must be an absolute UTF-8 path without traversal or control characters"
    );
    Ok(())
}

impl InvocationAnchor {
    pub(super) fn verify(&self, binding: &InvocationBindingV1, kind: RuntimeKind) -> Result<()> {
        binding.validate_policy(kind)?;
        ensure!(
            binding.executable_directory.as_ref() == Some(&identify_directory(&self.directory)?),
            "approved runtime directory identity or permissions changed; explicit reapproval required"
        );
        ensure!(
            identify(&self.directory, &self.basename)? == binding.file_identity,
            "approved runtime executable identity or content changed; explicit reapproval required"
        );
        Ok(())
    }

    pub(super) fn command(&self) -> Result<tokio::process::Command> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut command = tokio::process::Command::new(Path::new(".").join(&self.basename));
            // The absolute root is only the pre-fork staging cwd. The child
            // switches to the retained, approved directory immediately at exec.
            command.current_dir("/");
            let directory = Arc::clone(&self.directory);
            // SAFETY: this hook only calls async-signal-safe fchdir and reads an
            // already-owned FD. The captured Arc keeps it alive until spawn;
            // CLOEXEC closes the descriptor in the executed CLI.
            unsafe {
                command.pre_exec(move || {
                    if libc::fchdir(directory.as_raw_fd()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            Ok(command)
        }
        #[cfg(not(unix))]
        bail!("trusted runtime invocation requires a qualified Unix platform")
    }
}

#[cfg(unix)]
fn open_directory(path: &Path) -> Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    validate_absolute(path)?;
    let flags = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open("/")
        .context("runtime filesystem root cannot be pinned")?;
    // Walk from a retained root FD, refusing symlinks in every component. An
    // ancestor swapped after canonicalization can never redirect this open to
    // a project or other symlink target before its identity is approved.
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let name = std::ffi::CString::new(name.as_bytes())
            .context("runtime directory component contains NUL")?;
        // SAFETY: directory is valid and name is NUL terminated; ownership of a
        // successful new descriptor is transferred exactly once below.
        let fd =
            unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | flags) };
        ensure!(
            fd >= 0,
            "approved runtime directory cannot be pinned without following a link: {}",
            std::io::Error::last_os_error()
        );
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

#[cfg(unix)]
fn identify_directory(directory: &File) -> Result<ExecutableDirectoryIdentityV1> {
    use std::os::unix::fs::MetadataExt;
    crate::control_fs::validate_filesystem_permissions(directory)?;
    let facts = directory.metadata()?;
    ensure!(
        facts.is_dir()
            && (facts.uid() == 0 || facts.uid() == effective_uid())
            && facts.mode() & 0o022 == 0,
        "runtime executable directory must be owned by root or control, without group/other write permission"
    );
    reject_extended_acl(directory)?;
    Ok(ExecutableDirectoryIdentityV1 {
        device: facts.dev(),
        inode: facts.ino(),
        owner: facts.uid(),
        group: facts.gid(),
        mode: facts.mode(),
    })
}

const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

fn hash_bounded(reader: impl Read, initial_length: u64) -> Result<String> {
    ensure!(
        initial_length <= MAX_EXECUTABLE_BYTES,
        "runtime executable exceeds the 512 MiB approval limit"
    );
    let bound = initial_length
        .checked_add(1)
        .context("runtime executable length cannot be bounded")?;
    let mut reader = reader.take(bound);
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hash.update(&buffer[..n]);
    }
    ensure!(
        total == initial_length,
        "runtime executable grew or was truncated during verification"
    );
    Ok(hex::encode(hash.finish().as_ref()))
}

#[cfg(unix)]
fn identify(directory: &File, basename: &std::ffi::OsStr) -> Result<ExecutableIdentityV1> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    };
    let basename = std::ffi::CString::new(basename.as_bytes())
        .context("runtime executable basename contains NUL")?;
    // SAFETY: directory is an open directory FD and basename is NUL-terminated.
    // O_NOFOLLOW forbids a leaf substitution to a symlink; O_NONBLOCK prevents
    // a FIFO from blocking before fstat can reject non-regular files.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            basename.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    ensure!(
        fd >= 0,
        "approved runtime executable cannot be opened relative to its pinned directory: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: openat returned a new owned FD, transferred exactly once to File.
    let mut file = unsafe { File::from_raw_fd(fd) };
    crate::control_fs::validate_filesystem_permissions(&file)?;
    let before = file.metadata()?;
    ensure!(
        before.is_file()
            && (before.uid() == 0 || before.uid() == effective_uid())
            && before.mode() & 0o111 != 0
            && before.mode() & 0o022 == 0,
        "runtime program must be a regular executable owned by root or control, without group/other write permission"
    );
    reject_extended_acl(&file)?;
    let sha256 = hash_bounded(&mut file, before.len())?;
    let after = file.metadata()?;
    ensure!(
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
            && before.mode() == after.mode()
            && before.uid() == after.uid()
            && before.gid() == after.gid(),
        "runtime executable changed during verification"
    );
    reject_extended_acl(&file)?;
    Ok(ExecutableIdentityV1 {
        device: before.dev(),
        inode: before.ino(),
        owner: before.uid(),
        group: before.gid(),
        mode: before.mode(),
        length: before.len(),
        sha256,
    })
}

#[cfg(target_os = "macos")]
fn reject_extended_acl(file: &File) -> Result<()> {
    use std::{ffi::c_void, os::fd::AsRawFd};
    // Darwin's descriptor ACL API is not exposed by the libc crate. Signatures
    // and return conventions come from the Apple SDK sys/acl.h and acl_entry.c.
    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut c_void;
        fn acl_get_entry(
            acl: *mut c_void,
            entry_id: libc::c_int,
            entry: *mut *mut c_void,
        ) -> libc::c_int;
        fn acl_free(value: *mut c_void) -> libc::c_int;
    }
    // SAFETY: file owns a valid descriptor; ACL_TYPE_EXTENDED is 0x100 in Darwin.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        // Apple's filesec API reports ENOENT when FILESEC_ACL is absent. Other
        // inspection errors never become a claim of absent ACLs.
        ensure!(
            error.raw_os_error() == Some(libc::ENOENT),
            "runtime executable ACL inspection failed: {error}"
        );
        return Ok(());
    }
    let mut entry = std::ptr::null_mut();
    // SAFETY: acl is the valid allocated ACL returned above; entry is writable.
    let first = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = std::io::Error::last_os_error();
    // SAFETY: acl is released once, after its final access.
    let freed = unsafe { acl_free(acl) };
    ensure!(
        freed == 0,
        "runtime executable ACL inspection could not release its result"
    );
    ensure!(
        first == -1 && error.raw_os_error() == Some(libc::EINVAL),
        "runtime executable or its final directory has an extended ACL; explicit control installation without ACL entries is required"
    );
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reject_extended_acl(_file: &File) -> Result<()> {
    // On qualified local Linux POSIX-ACL filesystems, mode's group bits contain
    // ACL_MASK, so mode022=0 excludes write via named users/groups. NFSv4/SMB/FUSE
    // ACL semantics and administrative writers are not qualified by this check.
    Ok(())
}

#[cfg(not(unix))]
fn open_directory(_path: &Path) -> Result<File> {
    bail!("trusted runtime invocation requires a qualified Unix platform")
}
#[cfg(not(unix))]
fn identify_directory(_directory: &File) -> Result<ExecutableDirectoryIdentityV1> {
    bail!("trusted runtime invocation requires a qualified Unix platform")
}
#[cfg(not(unix))]
fn identify(_directory: &File, _basename: &std::ffi::OsStr) -> Result<ExecutableIdentityV1> {
    bail!("trusted runtime invocation requires a qualified Unix platform")
}

fn validate_helper_directories() -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for path in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            let directory = open_directory(&Path::new(path).canonicalize()?)?;
            let facts = directory.metadata()?;
            ensure!(
                facts.is_dir() && facts.uid() == 0 && facts.mode() & 0o022 == 0,
                "runtime helper PATH requires system directories owned by root without group/other write permission"
            );
            reject_extended_acl(&directory)?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    bail!("trusted runtime invocation requires a qualified Unix platform")
}

fn installation_entries(kind: RuntimeKind) -> Vec<PathBuf> {
    let mut entries: Vec<_> = if cfg!(target_os = "macos") {
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .into_iter()
            .map(|dir| Path::new(dir).join(kind.program()))
            .collect()
    } else {
        ["/usr/bin", "/usr/local/bin", "/bin"]
            .into_iter()
            .map(|dir| Path::new(dir).join(kind.program()))
            .collect()
    };
    if cfg!(target_os = "macos") && kind == RuntimeKind::Docker {
        entries.push(PathBuf::from(
            "/Applications/Docker.app/Contents/Resources/bin/docker",
        ));
    }
    if cfg!(target_os = "macos") && kind == RuntimeKind::Podman {
        entries.push(PathBuf::from("/opt/podman/bin/podman"));
    }
    entries
}

fn installed_target(kind: RuntimeKind, canonical: &Path) -> bool {
    installation_entries(kind)
        .iter()
        .any(|entry| entry == canonical)
        || cfg!(target_os = "macos")
            && ["/opt/homebrew/Cellar", "/usr/local/Cellar"]
                .iter()
                .any(|root| canonical.starts_with(Path::new(root).join(kind.program())))
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or memory preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn with_effective_account<T>(inspect: impl FnOnce(u32, &libc::passwd) -> Result<T>) -> Result<T> {
    let uid = effective_uid();
    let mut storage = vec![0_u8; 16 * 1024];
    loop {
        let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: storage is writable for its length; account/result are valid
        // output pointers. No pointer is read until the call reports success.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                account.as_mut_ptr(),
                storage.as_mut_ptr().cast(),
                storage.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && storage.len() < 1024 * 1024 {
            storage.resize(storage.len() * 2, 0);
            continue;
        }
        ensure!(
            status == 0 && !result.is_null(),
            "control OS account home is unavailable"
        );
        // SAFETY: successful getpwuid_r initialized account. Its strings point
        // into storage, which remains alive for the callback.
        let account = unsafe { account.assume_init() };
        return inspect(uid, &account);
    }
}

#[cfg(unix)]
fn account_home_from_record(account: &libc::passwd) -> Result<PathBuf> {
    use std::{ffi::CStr, os::unix::ffi::OsStrExt};
    ensure!(
        !account.pw_dir.is_null(),
        "control OS account home is unavailable"
    );
    // SAFETY: the caller retains the getpwuid_r buffer during inspection.
    let home = PathBuf::from(std::ffi::OsStr::from_bytes(
        unsafe { CStr::from_ptr(account.pw_dir) }.to_bytes(),
    ));
    validate_absolute(&home)?;
    Ok(home)
}

#[cfg(unix)]
fn account_home() -> Result<PathBuf> {
    with_effective_account(|_, account| account_home_from_record(account))
}

#[cfg(unix)]
pub(crate) fn control_service_account() -> Result<(u32, String)> {
    ensure!(
        effective_uid() != 0,
        "broker service plan requires a non-root control account"
    );
    with_effective_account(|uid, account| {
        account_home_from_record(account)?;
        ensure!(
            account.pw_uid == uid && !account.pw_name.is_null(),
            "control OS account name is unavailable"
        );
        // SAFETY: the account buffer is retained and the name pointer is non-null.
        let name = unsafe { std::ffi::CStr::from_ptr(account.pw_name) }
            .to_str()
            .context("control OS account name is not UTF-8")?;
        ensure!(
            !name.is_empty() && !name.chars().any(char::is_control),
            "control OS account name is invalid"
        );
        Ok((uid, name.to_owned()))
    })
}

fn standard_environment() -> Result<BTreeMap<String, String>> {
    #[cfg(not(unix))]
    bail!("trusted runtime invocation requires a qualified Unix platform");
    #[cfg(unix)]
    {
        let home = account_home()?;
        let mut env = BTreeMap::new();
        for (key, path) in [
            ("HOME", home.clone()),
            ("DOCKER_CONFIG", home.join(".docker")),
            ("XDG_CONFIG_HOME", home.join(".config")),
            ("XDG_DATA_HOME", home.join(".local/share")),
            ("XDG_CACHE_HOME", home.join(".cache")),
        ] {
            env.insert(
                key.into(),
                path.to_str()
                    .context("control configuration path must be UTF-8")?
                    .into(),
            );
        }
        // Standard rootless Podman runtime location, never inherited from a project.
        let runtime_dir = PathBuf::from(format!("/run/user/{}", effective_uid()));
        if let Ok(metadata) = std::fs::metadata(&runtime_dir) {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                metadata.is_dir()
                    && metadata.uid() == effective_uid()
                    && metadata.mode() & 0o077 == 0,
                "standard control runtime directory has unsafe ownership or permissions"
            );
            env.insert(
                "XDG_RUNTIME_DIR".into(),
                runtime_dir.to_string_lossy().into_owned(),
            );
        }
        validate_helper_directories()?;
        env.insert("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into());
        env.insert("LANG".into(), "C.UTF-8".into());
        env.insert("LC_ALL".into(), "C.UTF-8".into());
        Ok(env)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn standard_podman_installer_target_is_approved_without_path_search() {
        let installed = Path::new("/opt/podman/bin/podman");
        assert!(installed_target(RuntimeKind::Podman, installed));
        assert!(
            installation_entries(RuntimeKind::Podman)
                .iter()
                .any(|entry| entry == installed)
        );
        assert!(!installed_target(
            RuntimeKind::Podman,
            Path::new("/project/podman")
        ));
        assert!(!installed_target(RuntimeKind::Docker, installed));
    }
}

#[cfg(test)]
mod bounded_read_tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct GrowingFile {
        file: File,
        path: PathBuf,
        bytes_read: Arc<AtomicUsize>,
        grew: bool,
    }
    impl Read for GrowingFile {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.grew {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&self.path)?
                    .write_all(&[b'x'; 4096])?;
                self.grew = true;
            }
            let count = self.file.read(buffer)?;
            self.bytes_read.fetch_add(count, Ordering::Relaxed);
            Ok(count)
        }
    }

    #[test]
    fn growing_file_reads_only_initial_length_plus_one_then_refuses() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("growing");
        std::fs::write(&path, b"original").unwrap();
        let file = File::open(&path).unwrap();
        let initial_length = file.metadata().unwrap().len();
        let bytes_read = Arc::new(AtomicUsize::new(0));
        let reader = GrowingFile {
            file,
            path,
            bytes_read: bytes_read.clone(),
            grew: false,
        };
        assert!(hash_bounded(reader, initial_length).is_err());
        assert_eq!(
            bytes_read.load(Ordering::Relaxed),
            initial_length as usize + 1
        );
    }

    #[test]
    fn exact_content_is_hashed_and_truncation_is_refused() {
        let bytes = b"repeated executable contents".repeat(5_000);
        let length = bytes.len() as u64;
        let digest = hash_bounded(bytes.as_slice(), length).unwrap();
        assert_eq!(digest, hex::encode(Sha256::digest(&bytes)));
        assert!(hash_bounded(bytes.as_slice(), length + 1).is_err());
    }
}

#[cfg(all(test, unix))]
mod directory_traversal_tests {
    use super::*;

    #[test]
    fn pinning_rejects_a_canonical_ancestor_replaced_by_a_symlink() {
        let temporary = tempfile::tempdir().unwrap();
        let original = temporary.path().join("installed");
        let project = temporary.path().join("project");
        std::fs::create_dir_all(original.join("bin")).unwrap();
        std::fs::create_dir_all(project.join("bin")).unwrap();
        let canonical = original.join("bin").canonicalize().unwrap();
        assert!(open_directory(&canonical).is_ok());
        std::fs::rename(&original, temporary.path().join("parked")).unwrap();
        std::os::unix::fs::symlink(&project, &original).unwrap();
        assert!(
            open_directory(&canonical).is_err(),
            "pinning must refuse every symlink component, including an ancestor changed after canonicalization"
        );
    }
}
