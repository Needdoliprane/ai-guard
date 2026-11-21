//! Private local transport shared by the broker and its internal stop helper.
//! Kernel UID and endpoint identity are necessary transport checks, not a tenant
//! identity. Callers still authenticate every business request with the gateway.
//! Each listener uses a new namespace; no existing socket or record is removed.

use crate::control_fs::Directory;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use std::{
    ffi::CString,
    fmt,
    io::{self, Write},
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
use uuid::Uuid;

const MAX_FRAME: u32 = 512 * 1024;
const MAX_GENERATIONS: usize = 128;
// The common Linux/Darwin pathname bound, including room for a terminating NUL.
const MAX_SOCKET_PATH: usize = 103;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub type IpcResult<T> = Result<T, IpcError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcError {
    InvalidInput,
    TooLarge,
    Timeout,
    Unauthorized,
    Unavailable,
    UnsafeFilesystem,
    Conflict,
    PathTooLong,
    LimitExceeded,
}
impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid local IPC message",
            Self::TooLarge => "local IPC message exceeds its bound",
            Self::Timeout => "local IPC operation timed out",
            Self::Unauthorized => "local IPC peer is unauthorized",
            Self::Unavailable => "local IPC transport is unavailable",
            Self::UnsafeFilesystem => "local IPC filesystem validation failed",
            Self::Conflict => "local IPC identity conflicts with an existing entry",
            Self::PathTooLong => "local IPC root is too long; choose a shorter private root",
            Self::LimitExceeded => "local IPC generation capacity is exhausted",
        })
    }
}
impl std::error::Error for IpcError {}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointWireV1 {
    version: u32,
    epoch: Uuid,
    directory_name: String,
    directory_device: u64,
    directory_inode: u64,
    socket_device: u64,
    socket_inode: u64,
    owner_uid: u32,
}

/// A discovery record from a trusted private namespace, never an authorization.
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct EndpointRecordV1(EndpointWireV1);
impl fmt::Debug for EndpointRecordV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EndpointRecordV1(<private>)")
    }
}
impl<'de> Deserialize<'de> for EndpointRecordV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let wire = EndpointWireV1::deserialize(deserializer)
            .map_err(|_| D::Error::custom("invalid private IPC endpoint"))?;
        let record = Self(wire);
        record
            .validate_shape()
            .map_err(|_| D::Error::custom("invalid private IPC endpoint"))?;
        Ok(record)
    }
}
impl EndpointRecordV1 {
    #[cfg(test)]
    pub(crate) fn unavailable_for_test() -> Self {
        Self(EndpointWireV1 {
            version: 1,
            epoch: Uuid::nil(),
            directory_name: String::new(),
            directory_device: 0,
            directory_inode: 0,
            socket_device: 0,
            socket_inode: 0,
            owner_uid: 0,
        })
    }

    pub fn epoch(&self) -> Uuid {
        self.0.epoch
    }

    pub fn directory_name(&self) -> &str {
        &self.0.directory_name
    }

    fn validate_shape(&self) -> IpcResult<()> {
        let expected = directory_name(self.0.epoch);
        if self.0.version != 1
            || self.0.epoch.is_nil()
            || self.0.directory_name != expected
            || self.0.directory_inode == 0
            || self.0.socket_inode == 0
        {
            return Err(IpcError::InvalidInput);
        }
        Ok(())
    }

    fn checked_directory(&self, root: &Directory) -> IpcResult<Directory> {
        self.validate_shape()?;
        root.revalidate().map_err(|_| IpcError::UnsafeFilesystem)?;
        if self.0.owner_uid != effective_uid() {
            return Err(IpcError::Unauthorized);
        }
        let directory = root
            .persistent_child(&self.0.directory_name, false)
            .map_err(|_| IpcError::UnsafeFilesystem)?
            .ok_or(IpcError::UnsafeFilesystem)?;
        self.check_retained(&directory)?;
        Ok(directory)
    }

    pub(crate) fn validate_at(&self, root: &Directory) -> IpcResult<()> {
        self.checked_directory(root).map(|_| ())
    }

    fn check_retained(&self, directory: &Directory) -> IpcResult<()> {
        use std::os::unix::fs::MetadataExt;
        directory
            .revalidate()
            .map_err(|_| IpcError::UnsafeFilesystem)?;
        let file = directory
            .try_clone_file()
            .map_err(|_| IpcError::UnsafeFilesystem)?;
        let facts = file.metadata().map_err(|_| IpcError::UnsafeFilesystem)?;
        if facts.dev() != self.0.directory_device
            || facts.ino() != self.0.directory_inode
            || facts.uid() != self.0.owner_uid
            || facts.mode() & 0o7777 != 0o700
        {
            return Err(IpcError::UnsafeFilesystem);
        }
        let socket = socket_facts(directory, true)?;
        if socket.device != self.0.socket_device || socket.inode != self.0.socket_inode {
            return Err(IpcError::UnsafeFilesystem);
        }
        directory
            .revalidate()
            .map_err(|_| IpcError::UnsafeFilesystem)
    }

    pub async fn connect_checked(&self, root: &Directory) -> IpcResult<UnixStream> {
        let directory = self.checked_directory(root)?;
        let path = directory.path().join("s");
        check_path(&path)?;
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(path))
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| IpcError::Unavailable)?;
        check_peer(&stream)?;
        self.check_retained(&directory)?;
        root.revalidate().map_err(|_| IpcError::UnsafeFilesystem)?;
        Ok(stream)
    }
}

pub struct PrivateListener {
    listener: UnixListener,
    directory: Directory,
    endpoint: EndpointRecordV1,
}
impl fmt::Debug for PrivateListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateListener(<private>)")
    }
}
impl PrivateListener {
    /// The caller holds its installation/role lease. Old generations are kept;
    /// reaching this logical cap requires explicit operator retention/recovery.
    pub fn bind_new(root: &Directory) -> IpcResult<Self> {
        use std::os::unix::fs::MetadataExt;
        tokio::runtime::Handle::try_current().map_err(|_| IpcError::Unavailable)?;
        root.revalidate().map_err(|_| IpcError::UnsafeFilesystem)?;
        let entries = std::fs::read_dir(root.path()).map_err(|_| IpcError::UnsafeFilesystem)?;
        let mut count = 0;
        for entry in entries {
            entry.map_err(|_| IpcError::UnsafeFilesystem)?;
            count += 1;
            if count >= MAX_GENERATIONS {
                return Err(IpcError::LimitExceeded);
            }
        }
        root.revalidate().map_err(|_| IpcError::UnsafeFilesystem)?;
        let epoch = Uuid::new_v4();
        let name = directory_name(epoch);
        let path = root.path().join(&name).join("s");
        check_path(&path)?;
        let encoded = CString::new(name.as_str()).map_err(|_| IpcError::InvalidInput)?;
        let parent = root
            .try_clone_file()
            .map_err(|_| IpcError::UnsafeFilesystem)?;
        // SAFETY: a checked single-component name in a retained private parent.
        // EEXIST never adopts, removes or repairs a pre-existing generation.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), encoded.as_ptr(), 0o700) } != 0 {
            return Err(
                if io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                    IpcError::Conflict
                } else {
                    IpcError::UnsafeFilesystem
                },
            );
        }
        let directory = root
            .persistent_child(&name, false)
            .map_err(|_| IpcError::UnsafeFilesystem)?
            .ok_or(IpcError::UnsafeFilesystem)?;
        let listener =
            std::os::unix::net::UnixListener::bind(&path).map_err(|_| IpcError::Unavailable)?;
        let initial = socket_facts(&directory, false)?;
        let retained = directory
            .try_clone_file()
            .map_err(|_| IpcError::UnsafeFilesystem)?;
        // SAFETY: no-follow metadata change of the fixed leaf under its retained
        // private directory; no symlink or arbitrary caller pathname is eligible.
        if unsafe {
            libc::fchmodat(
                retained.as_raw_fd(),
                c"s".as_ptr(),
                0o600,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(IpcError::UnsafeFilesystem);
        }
        let socket = socket_facts(&directory, true)?;
        if initial.device != socket.device || initial.inode != socket.inode {
            return Err(IpcError::UnsafeFilesystem);
        }
        let facts = retained
            .metadata()
            .map_err(|_| IpcError::UnsafeFilesystem)?;
        let endpoint = EndpointRecordV1(EndpointWireV1 {
            version: 1,
            epoch,
            directory_name: name,
            directory_device: facts.dev(),
            directory_inode: facts.ino(),
            socket_device: socket.device,
            socket_inode: socket.inode,
            owner_uid: effective_uid(),
        });
        endpoint.check_retained(&directory)?;
        directory.sync().map_err(|_| IpcError::UnsafeFilesystem)?;
        root.sync().map_err(|_| IpcError::UnsafeFilesystem)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| IpcError::Unavailable)?;
        let listener = UnixListener::from_std(listener).map_err(|_| IpcError::Unavailable)?;
        endpoint.check_retained(&directory)?;
        Ok(Self {
            listener,
            directory,
            endpoint,
        })
    }

    pub fn endpoint(&self) -> &EndpointRecordV1 {
        &self.endpoint
    }

    pub async fn accept_checked(&self) -> IpcResult<UnixStream> {
        self.endpoint.check_retained(&self.directory)?;
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|_| IpcError::Unavailable)?;
        check_peer(&stream)?;
        self.endpoint.check_retained(&self.directory)?;
        Ok(stream)
    }
}

fn directory_name(epoch: Uuid) -> String {
    // The full UUID remains in the record; O_EXCL mkdir rejects a short-name
    // collision. A compact component also works with Darwin's short sun_path.
    format!("i-{}", &epoch.simple().to_string()[..16])
}

fn check_path(path: &std::path::Path) -> IpcResult<()> {
    if path.as_os_str().as_bytes().len() > MAX_SOCKET_PATH {
        return Err(IpcError::PathTooLong);
    }
    Ok(())
}

fn effective_uid() -> u32 {
    // SAFETY: read-only process credential query.
    unsafe { libc::geteuid() }
}

fn check_peer(stream: &UnixStream) -> IpcResult<()> {
    let credentials = stream.peer_cred().map_err(|_| IpcError::Unauthorized)?;
    if credentials.uid() != effective_uid() {
        return Err(IpcError::Unauthorized);
    }
    Ok(())
}

struct SocketFacts {
    device: u64,
    inode: u64,
}

fn socket_facts(directory: &Directory, require_private: bool) -> IpcResult<SocketFacts> {
    let retained = directory
        .try_clone_file()
        .map_err(|_| IpcError::UnsafeFilesystem)?;
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: checked retained directory, fixed NUL-terminated leaf and writable
    // stat buffer. AT_SYMLINK_NOFOLLOW inspects the entry without following it.
    let result = unsafe {
        libc::fstatat(
            retained.as_raw_fd(),
            c"s".as_ptr(),
            facts.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(IpcError::UnsafeFilesystem);
    }
    // SAFETY: successful fstatat initialized the complete stat structure.
    let facts = unsafe { facts.assume_init() };
    if facts.st_mode & libc::S_IFMT != libc::S_IFSOCK
        || facts.st_uid != effective_uid()
        || facts.st_nlink != 1
        || (require_private && facts.st_mode & 0o7777 != 0o600)
    {
        return Err(IpcError::UnsafeFilesystem);
    }
    Ok(SocketFacts {
        device: facts.st_dev as u64,
        inode: facts.st_ino,
    })
}

fn check_frame_options(max_bytes: u32, timeout: Duration) -> IpcResult<()> {
    if max_bytes == 0 || max_bytes > MAX_FRAME || timeout.is_zero() {
        return Err(IpcError::InvalidInput);
    }
    Ok(())
}

fn read_error(error: io::Error) -> IpcError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        IpcError::InvalidInput
    } else {
        IpcError::Unavailable
    }
}

/// The timeout covers the entire length prefix and body, not each read anew.
pub async fn read_frame<T: DeserializeOwned>(
    stream: &mut (impl AsyncRead + Unpin),
    max_bytes: u32,
    timeout: Duration,
) -> IpcResult<T> {
    check_frame_options(max_bytes, timeout)?;
    tokio::time::timeout(timeout, async {
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.map_err(read_error)?;
        let length = u32::from_be_bytes(prefix);
        if length == 0 {
            return Err(IpcError::InvalidInput);
        }
        if length > max_bytes {
            return Err(IpcError::TooLarge);
        }
        let mut bytes = vec![0; length as usize];
        stream.read_exact(&mut bytes).await.map_err(read_error)?;
        let mut decoder = serde_json::Deserializer::from_slice(&bytes);
        let mut ignored = false;
        let value = serde_ignored::deserialize(&mut decoder, |_| ignored = true)
            .map_err(|_| IpcError::InvalidInput)?;
        decoder.end().map_err(|_| IpcError::InvalidInput)?;
        if ignored {
            return Err(IpcError::InvalidInput);
        }
        Ok(value)
    })
    .await
    .map_err(|_| IpcError::Timeout)?
}

struct BoundedBody {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for BoundedBody {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("IPC output bound exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialization is bounded too: an oversized value never builds an unbounded
/// JSON allocation or emits a partial frame before its size has been checked.
pub async fn write_frame<T: Serialize>(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &T,
    max_bytes: u32,
    timeout: Duration,
) -> IpcResult<()> {
    check_frame_options(max_bytes, timeout)?;
    let mut body = BoundedBody {
        bytes: Vec::with_capacity((max_bytes as usize).min(4096)),
        limit: max_bytes as usize,
        exceeded: false,
    };
    if serde_json::to_writer(&mut body, value).is_err() {
        return Err(if body.exceeded {
            IpcError::TooLarge
        } else {
            IpcError::InvalidInput
        });
    }
    let length = u32::try_from(body.bytes.len()).map_err(|_| IpcError::TooLarge)?;
    tokio::time::timeout(timeout, async {
        stream
            .write_all(&length.to_be_bytes())
            .await
            .map_err(|_| IpcError::Unavailable)?;
        stream
            .write_all(&body.bytes)
            .await
            .map_err(|_| IpcError::Unavailable)?;
        Ok(())
    })
    .await
    .map_err(|_| IpcError::Timeout)?
}
