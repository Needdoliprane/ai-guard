//! Shared private transport for real broker IPC tests.
use super::{Directory, PrivateListener, RoleLease};
use anyhow::{Result, anyhow};
use std::{fs, future::Future, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};
use tokio::{net::UnixStream, sync::oneshot, task::JoinHandle};

pub(super) struct OwnedIpcPeer {
    _root: tempfile::TempDir,
    pub(super) root: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}

impl OwnedIpcPeer {
    pub(super) async fn start<F, Fut>(mut handle: F) -> Result<Self>
    where
        F: FnMut(UnixStream) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        // A separate, owned transport root keeps Unix paths within their native
        // bound. Service authentication still uses its original real registry.
        let temp = tempfile::Builder::new()
            .prefix("ag-rs2-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(
                std::env::var_os("AI_GUARD_TEST_D5_IPC_ROOT").unwrap_or_else(|| "/tmp".into()),
            )?;
        let root = temp.path().canonicalize()?;
        let directory = Directory::open_existing(&root)?;
        let broker = directory.persistent_child("broker", true)?.unwrap();
        let lease = RoleLease::acquire(&broker)?;
        let listener = PrivateListener::bind_new(&lease.listener_root()?)?;
        lease.publish_endpoint(listener.endpoint())?;
        let (stop, mut receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _lease = lease;
            loop {
                tokio::select! {
                    _ = &mut receive => return Ok(()),
                    accepted = listener.accept_checked() => {
                        handle(accepted?).await?;
                    }
                }
            }
        });
        Ok(Self {
            _root: temp,
            root,
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub(super) async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(anyhow!("owned IPC fixture did not drain"));
                }
            }
        }
        Ok(())
    }
}

impl Drop for OwnedIpcPeer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
