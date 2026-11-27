//! The actual broker must create the first Legacy guardian. These fixtures
//! retain its private endpoint and native process identity across broker
//! restarts. Teardown never adopts an already existing guardian.
use agentsandbox::{
    broker::local_ipc::{EndpointRecordV1, read_frame, write_frame},
    control_fs::{Directory, PrivateFile},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{net::UnixStream, process::Child, time::timeout};
use uuid::Uuid;

#[cfg(target_os = "macos")]
use super::owned_darwin::DarwinIdentity as NativeIdentity;
#[cfg(target_os = "linux")]
#[path = "owned_linux.rs"]
mod owned_linux;
#[cfg(target_os = "linux")]
use owned_linux::LinuxIdentity as NativeIdentity;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct NativeIdentity;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl NativeIdentity {
    fn capture(_: &UnixStream, _: u32, _: &Path) -> Result<Self> {
        anyhow::bail!("native Legacy guardian custody unavailable on this host");
    }
    fn revalidate_retained_peer(&self, _: &UnixStream) -> Result<()> {
        anyhow::bail!("native Legacy guardian custody unavailable on this host");
    }
    fn signal_retained_peer(&self, _: &UnixStream) -> Result<()> {
        anyhow::bail!("native Legacy guardian custody unavailable on this host");
    }
    async fn wait_exit(&self) -> Result<()> {
        anyhow::bail!("native Legacy guardian custody unavailable on this host");
    }
}

pub(super) fn require_fresh_role(root: &Path) -> Result<()> {
    ensure!(
        matches!(
            std::fs::symlink_metadata(root.join("enforcer")),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        ),
        "first fixture Legacy guardian role must be absent"
    );
    Ok(())
}

fn owned_broker_pid(broker: &mut Child) -> Result<u32> {
    ensure!(
        broker.try_wait()?.is_none(),
        "fixture broker exited before Legacy guardian custody"
    );
    broker.id().context("fixture broker identity unavailable")
}

fn one_publication(announcements: &Directory) -> Result<()> {
    announcements.revalidate()?;
    let entries = std::fs::read_dir(announcements.path())?
        .take(2)
        .collect::<io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() == 1,
        "fixture Legacy guardian did not retain exactly one announcement"
    );
    Ok(())
}

pub(super) async fn require_socket_closed(path: &Path) -> Result<()> {
    let connection = timeout(Duration::from_secs(2), UnixStream::connect(path)).await?;
    ensure!(
        matches!(
            connection,
            Err(error) if matches!(
                error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            )
        ),
        "fixture IPC closure unconfirmed"
    );
    Ok(())
}

pub(super) struct OwnedLegacyGuardian {
    role: Directory,
    announcements: Directory,
    publication: PrivateFile,
    endpoints: Directory,
    endpoint: EndpointRecordV1,
    capability: PrivateFile,
    runtime_root: Directory,
    runtime_socket: PathBuf,
    runtime_identity: (u64, u64),
    identity: NativeIdentity,
}

impl OwnedLegacyGuardian {
    pub(super) async fn capture(root: &Path, broker: &mut Child, program: &Path) -> Result<Self> {
        let broker_pid = owned_broker_pid(broker)?;
        let root = Directory::open_existing(root)?;
        let role = Directory::open_existing(&root.path().join("enforcer"))?;
        let announcements = Directory::open_existing(&role.path().join("announcements"))?;
        one_publication(&announcements)?;
        let publication = announcements.read_private_file("e-0001.json", 4096)?;
        let record: Value = serde_json::from_slice(publication.bytes())?;
        ensure!(
            record["version"] == 1 && record["sequence"] == 1,
            "invalid fixture Legacy guardian announcement"
        );
        let endpoint: EndpointRecordV1 = serde_json::from_value(record["endpoint"].clone())?;
        let endpoints = Directory::open_existing(&role.path().join("endpoints"))?;
        let capability = role.read_private_file("auth", 32)?;
        ensure!(
            capability.bytes().len() == 32,
            "invalid private fixture capability length"
        );
        let runtime_root = Directory::open_existing(
            root.path()
                .parent()
                .context("fixture runtime root unavailable")?,
        )?;
        let runtime_socket = runtime_root.path().join("r");
        let runtime = std::fs::symlink_metadata(&runtime_socket)?;
        ensure!(
            runtime.file_type().is_socket(),
            "fixture runtime socket invalid"
        );
        let runtime_identity = (runtime.dev(), runtime.ino());
        let mut stream = endpoint.connect_checked(&endpoints).await?;
        let identity = NativeIdentity::capture(&stream, broker_pid, program)?;
        let owner = Self {
            role,
            announcements,
            publication,
            endpoints,
            endpoint,
            capability,
            runtime_root,
            runtime_socket,
            runtime_identity,
            identity,
        };
        let reply = owner
            .rpc(&mut stream, json!({"kind":"prospective_capabilities"}))
            .await?;
        let observed = &reply["value"];
        let nonce: Uuid = serde_json::from_value(observed["fresh_launch_nonce"].clone())?;
        ensure!(
            reply["kind"] == "prospective_capabilities"
                && observed["version"] == 1
                && observed["epoch"] == serde_json::to_value(owner.endpoint.epoch())?
                && observed
                    .get("recovery_generation")
                    .is_some_and(Value::is_null)
                && observed["recovery_incomplete"] == false
                && observed["known_stop_slots"] == 0
                && observed["owned_active"] == 0
                && !nonce.is_nil()
                && owned_broker_pid(broker)? == broker_pid,
            "fixture Legacy guardian is not the broker's fresh empty instance"
        );
        owner.revalidate()?;
        Ok(owner)
    }

    fn revalidate(&self) -> Result<()> {
        self.role.revalidate()?;
        self.endpoints.revalidate()?;
        self.publication.revalidate()?;
        self.capability.revalidate()?;
        one_publication(&self.announcements)
    }

    async fn checked_peer(&self) -> Result<UnixStream> {
        self.revalidate()?;
        let stream = self.endpoint.connect_checked(&self.endpoints).await?;
        self.identity.revalidate_retained_peer(&stream)?;
        self.revalidate()?;
        Ok(stream)
    }

    pub(super) async fn require_live(&self) -> Result<()> {
        self.runtime_root.revalidate()?;
        self.require_runtime_identity(&std::fs::symlink_metadata(&self.runtime_socket)?)?;
        self.checked_peer().await.map(drop)
    }

    async fn rpc(&self, stream: &mut UnixStream, request: Value) -> Result<Value> {
        let wait = Duration::from_secs(3);
        let challenge: Value = read_frame(stream, 1024, wait).await?;
        let epoch: Uuid = serde_json::from_value(challenge["epoch"].clone())?;
        let nonce: Uuid = serde_json::from_value(challenge["nonce"].clone())?;
        ensure!(
            epoch == self.endpoint.epoch() && !nonce.is_nil(),
            "fixture guardian challenge identity differs"
        );
        self.revalidate()?;
        self.identity.revalidate_retained_peer(stream)?;
        let mut proof = Sha256::new();
        proof.update(b"ai-guard:enforcer-auth:v1\0");
        proof.update(self.capability.bytes());
        proof.update(epoch.as_bytes());
        proof.update(nonce.as_bytes());
        write_frame(
            stream,
            &json!({"proof": format!("{:x}", proof.finalize())}),
            1024,
            wait,
        )
        .await?;
        write_frame(stream, &request, 1024, wait).await?;
        Ok(read_frame(stream, 2048, wait).await?)
    }

    async fn require_runtime_closed(&self) -> Result<()> {
        self.runtime_root.revalidate()?;
        match std::fs::symlink_metadata(&self.runtime_socket) {
            Ok(socket) => self.require_runtime_identity(&socket)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        require_socket_closed(&self.runtime_socket).await?;
        self.runtime_root.revalidate()
    }

    fn require_runtime_identity(&self, socket: &std::fs::Metadata) -> Result<()> {
        ensure!(
            socket.file_type().is_socket() && (socket.dev(), socket.ino()) == self.runtime_identity,
            "fixture runtime socket identity differs"
        );
        Ok(())
    }

    /// Called after the fixture has joined its synthetic runtime server.
    /// Occupied Legacy slots are retained by design. A confirmed NotIdle
    /// permits only teardown of the attested test-process identity.
    pub(super) async fn close(&self) -> Result<&'static str> {
        self.require_runtime_closed().await?;
        let mut stream = self.checked_peer().await?;
        let reply = self
            .rpc(&mut stream, json!({"kind":"shutdown_if_idle"}))
            .await?;
        drop(stream);
        let result = if reply == json!({"kind":"acknowledged"}) {
            "idle_acknowledged_exit"
        } else {
            ensure!(
                reply == json!({"kind":"error","code":"not_idle"}),
                "fixture idle request unconfirmed"
            );
            self.require_runtime_closed().await?;
            let stream = self.checked_peer().await?;
            self.identity.signal_retained_peer(&stream)?;
            "exact_fixture_signal_exit"
        };
        self.identity.wait_exit().await?;
        let socket = self
            .endpoints
            .path()
            .join(self.endpoint.directory_name())
            .join("s");
        require_socket_closed(&socket).await?;
        self.revalidate()?;
        Ok(result)
    }
}
