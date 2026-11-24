//! Dedicated short-lived TLS preparer transport. No worker/session authority,
//! shell, inherited environment, payload logging or detached IO collectors.
use super::{Runtime, RuntimeKind, capture_stream};
use crate::model_tls::{Failure, VolumeSelection};
use serde::Serialize;
use serde_json::Value;
use std::{
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command, sync::Notify, time::Instant};
#[cfg(test)]
mod tests;

#[derive(Clone, Default)]
pub(crate) struct Cancellation {
    flag: Arc<AtomicBool>,
    changed: Arc<Notify>,
}
impl Cancellation {
    pub fn check(&self) -> Result<(), Failure> {
        if self.flag.load(Ordering::Acquire) {
            Err(Failure::Interrupted)
        } else {
            Ok(())
        }
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
    pub async fn wait(&self) {
        let notified = self.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.flag.load(Ordering::Acquire) {
            notified.await;
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Attempt {
    pub generation: String,
    pub preparer: String,
    pub cid: Option<String>,
    pub volumes: VolumeSelection,
    pub stop_observation: &'static str,
    pub cleanup_local: CleanupLocalStatus,
    pub runtime: Option<RuntimeProvenance>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanupLocalStatus {
    NotAttempted,
    TransportsJoined,
    TransportTerminationUnconfirmed,
}
impl Attempt {
    /// Preserve the primary failure independently of cleanup's local process
    /// status. This projects error results; it never invents an OS observation.
    pub fn finish_failure(&mut self, primary: Failure, cleanup: Result<(), Failure>) -> Failure {
        if cleanup.is_err() {
            self.stop_observation = "unknown";
        }
        self.cleanup_local = match cleanup {
            Err(Failure::LocalProcessUnknown) => {
                CleanupLocalStatus::TransportTerminationUnconfirmed
            }
            _ if self.stop_observation == "not_created" => CleanupLocalStatus::NotAttempted,
            _ => CleanupLocalStatus::TransportsJoined,
        };
        primary
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RuntimeProvenance {
    pub invocation_sha256: String,
    pub authority: RuntimeAuthority,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RuntimeAuthority {
    Docker {
        unix_endpoint: String,
        engine_id: String,
    },
    PodmanLocal {
        graph_root: String,
        run_root: String,
    },
}
impl RuntimeAuthority {
    fn from_info(kind: RuntimeKind, endpoint: Option<&str>, bytes: &[u8]) -> Result<Self, Failure> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Failure::Runtime)?;
        if kind == RuntimeKind::Docker {
            let endpoint = endpoint
                .filter(|endpoint| {
                    endpoint.len() <= 4096
                        && endpoint.strip_prefix("unix://").is_some_and(local_path)
                })
                .ok_or(Failure::Runtime)?;
            let id = value
                .get("ID")
                .and_then(Value::as_str)
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= 256
                        && value.bytes().all(|byte| byte.is_ascii_graphic())
                })
                .ok_or(Failure::Runtime)?;
            Ok(Self::Docker {
                unix_endpoint: endpoint.to_owned(),
                engine_id: id.to_owned(),
            })
        } else {
            if endpoint.is_some() {
                return Err(Failure::Runtime);
            }
            let graph = value
                .pointer("/store/graphRoot")
                .and_then(Value::as_str)
                .filter(|path| local_path(path))
                .ok_or(Failure::Runtime)?;
            let run = value
                .pointer("/store/runRoot")
                .and_then(Value::as_str)
                .filter(|path| local_path(path))
                .ok_or(Failure::Runtime)?;
            Ok(Self::PodmanLocal {
                graph_root: graph.to_owned(),
                run_root: run.to_owned(),
            })
        }
    }
}
fn local_path(value: &str) -> bool {
    Path::new(value).is_absolute() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
pub(crate) struct ProvisionRuntime {
    runtime: Runtime,
    endpoint: Option<String>,
    authority: Option<RuntimeAuthority>,
    deadline: Instant,
    cancel: Cancellation,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct VolumeIdentity {
    name: String,
    mountpoint: String,
    created_at: String,
}
fn hex_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(crate) fn image_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(hex_id)
}

fn matches_runtime_image(kind: RuntimeKind, observed: &Value, expected: &str) -> bool {
    let Some(digest) = expected
        .strip_prefix("sha256:")
        .filter(|digest| hex_id(digest))
    else {
        return false;
    };
    let observed = observed.as_str();
    observed == Some(expected) || (kind == RuntimeKind::Podman && observed == Some(digest))
}

async fn run(
    mut command: Command,
    args: &[String],
    input: Option<&[u8]>,
    limit: usize,
    deadline: Instant,
    cancel: &Cancellation,
) -> Result<Vec<u8>, Failure> {
    if cancel.flag.load(Ordering::Acquire) {
        return Err(Failure::Interrupted);
    }
    if Instant::now() >= deadline {
        return Err(Failure::TimedOut);
    }
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| Failure::Runtime)?;
    #[cfg(test)]
    tests::spawned(child.id());
    let stdin = child.stdin.take();
    let streams = child.stdout.take().zip(child.stderr.take());
    let Some((stdout, stderr)) = streams else {
        let _ = child.start_kill();
        if !tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .is_ok_and(|result| result.is_ok())
        {
            return Err(Failure::LocalProcessUnknown);
        }
        return Err(Failure::Runtime);
    };
    let result = {
        let work = async {
            tokio::join!(
                async {
                    if let (Some(mut stdin), Some(bytes)) = (stdin, input) {
                        stdin.write_all(bytes).await?;
                        stdin.shutdown().await?;
                    }
                    Ok::<(), std::io::Error>(())
                },
                child.wait(),
                capture_stream(stdout, limit),
                capture_stream(stderr, 4096)
            )
        };
        tokio::pin!(work);
        tokio::select! {
            result=&mut work=>Ok(result),
            _=tokio::time::sleep_until(deadline)=>Err(Failure::TimedOut),
            _=cancel.wait()=>Err(Failure::Interrupted),
        }
    };
    match result {
        Ok((Ok(()), Ok(status), Ok(stdout), Ok(_stderr)))
            if status.success() && !stdout.truncated && stdout.total_bytes <= limit as u64 =>
        {
            Ok(stdout.bytes)
        }
        Ok((_, Err(_), _, _)) => {
            let _ = child.start_kill();
            if !tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_ok_and(|result| result.is_ok())
            {
                return Err(Failure::LocalProcessUnknown);
            }
            Err(Failure::Runtime)
        }
        Ok(_) => Err(Failure::Runtime),
        Err(error) => {
            let _ = child.start_kill();
            // No JoinHandle is dropped: the inline writer/collectors are
            // cancelled before this owned local child is killed and reaped.
            if !tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_ok_and(|result| result.is_ok())
            {
                return Err(Failure::LocalProcessUnknown);
            }
            Err(error)
        }
    }
}

impl ProvisionRuntime {
    pub async fn connect(
        kind: RuntimeKind,
        program: Option<&Path>,
        endpoint: Option<&str>,
        allow_rootful: bool,
        deadline: Instant,
        cancel: Cancellation,
    ) -> Result<Self, Failure> {
        let runtime = match program {
            Some(path) => Runtime::with_program(kind, path),
            None => Runtime::new(kind),
        }
        .map_err(|_| Failure::Runtime)?;
        let mut selected = Self {
            runtime,
            endpoint: None,
            authority: None,
            deadline,
            cancel,
        };
        if kind == RuntimeKind::Docker {
            let endpoint = match endpoint {
                Some(endpoint) => endpoint.to_owned(),
                None => {
                    let context = selected.query(&["context", "show"], 4096).await?;
                    let context = std::str::from_utf8(&context)
                        .map_err(|_| Failure::Runtime)?
                        .trim();
                    if context.is_empty()
                        || context.len() > 128
                        || context.starts_with('-')
                        || !context
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                    {
                        return Err(Failure::Runtime);
                    }
                    let response = selected
                        .query(&["context", "inspect", context], 65536)
                        .await?;
                    let value: Value =
                        serde_json::from_slice(&response).map_err(|_| Failure::Runtime)?;
                    value
                        .get(0)
                        .and_then(|value| value.pointer("/Endpoints/docker/Host"))
                        .and_then(Value::as_str)
                        .ok_or(Failure::Runtime)?
                        .to_owned()
                }
            };
            let socket = endpoint
                .strip_prefix("unix://")
                .ok_or(Failure::InvalidInput)?;
            if !local_path(socket) || endpoint.len() > 4096 {
                return Err(Failure::InvalidInput);
            }
            selected.endpoint = Some(endpoint);
        } else if endpoint.is_some() {
            return Err(Failure::InvalidInput);
        }
        let info = selected.info().await?;
        super::parse_runtime_info(kind, &info, allow_rootful).map_err(|_| Failure::Runtime)?;
        selected.authority = Some(selected.authority_from(&info)?);
        Ok(selected)
    }
    pub async fn revalidate_provenance(&self) -> Result<RuntimeProvenance, Failure> {
        self.verify().await?;
        self.provenance()
    }
    pub fn provenance(&self) -> Result<RuntimeProvenance, Failure> {
        Ok(RuntimeProvenance {
            invocation_sha256: self.runtime.invocation().fingerprint(),
            authority: self.authority.clone().ok_or(Failure::Runtime)?,
        })
    }
    fn command(&self) -> Result<Command, Failure> {
        let mut command = self.runtime.command().map_err(|_| Failure::Runtime)?;
        match (&self.runtime.kind, &self.endpoint) {
            (RuntimeKind::Docker, Some(endpoint)) => {
                command.arg("--host").arg(endpoint);
            }
            (RuntimeKind::Podman, _) => {
                command.arg("--remote=false");
            }
            _ => {}
        }
        Ok(command)
    }
    async fn query(&self, args: &[&str], limit: usize) -> Result<Vec<u8>, Failure> {
        self.query_owned(
            &args
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            None,
            limit,
            &self.cancel,
        )
        .await
    }
    async fn query_owned(
        &self,
        args: &[String],
        input: Option<&[u8]>,
        limit: usize,
        cancel: &Cancellation,
    ) -> Result<Vec<u8>, Failure> {
        run(
            self.command()?,
            args,
            input,
            limit,
            self.deadline.min(Instant::now() + Duration::from_secs(15)),
            cancel,
        )
        .await
    }
    async fn info(&self) -> Result<Vec<u8>, Failure> {
        if self.runtime.kind == RuntimeKind::Docker {
            self.query(&["info", "--format", "{{json .}}"], 65536).await
        } else {
            self.query(&["info", "--format", "json"], 65536).await
        }
    }
    fn authority_from(&self, bytes: &[u8]) -> Result<RuntimeAuthority, Failure> {
        RuntimeAuthority::from_info(self.runtime.kind, self.endpoint.as_deref(), bytes)
    }
    async fn verify(&self) -> Result<(), Failure> {
        if Some(&self.authority_from(&self.info().await?)?) != self.authority.as_ref() {
            Err(Failure::Runtime)
        } else {
            Ok(())
        }
    }
    pub async fn inspect_image(&self, image: &str) -> Result<(), Failure> {
        if !image_id(image) {
            return Err(Failure::InvalidInput);
        }
        self.verify().await?;
        let bytes = self
            .query(
                &["image", "inspect", "--format", "{{json .}}", image],
                65536,
            )
            .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Runtime)?;
        if !matches_runtime_image(self.runtime.kind, &value["Id"], image) {
            return Err(Failure::Runtime);
        }
        Ok(())
    }
    pub async fn create_volume(
        &self,
        name: &str,
        generation: &str,
        role: &str,
    ) -> Result<VolumeIdentity, Failure> {
        self.verify().await?;
        // ls must succeed and be empty; an inspect failure is never treated
        // as proof of absence. UUID-derived names contain no filter syntax.
        if !self
            .query(
                &[
                    "volume",
                    "ls",
                    "--quiet",
                    "--filter",
                    &format!("name={name}"),
                ],
                4096,
            )
            .await?
            .iter()
            .all(u8::is_ascii_whitespace)
        {
            return Err(Failure::Runtime);
        }
        let args = vec![
            "volume".into(),
            "create".into(),
            "--driver".into(),
            "local".into(),
            "--label".into(),
            format!("ai-guard.model-tls.generation={generation}"),
            "--label".into(),
            format!("ai-guard.model-tls.role={role}"),
            name.into(),
        ];
        let output = self.query_owned(&args, None, 4096, &self.cancel).await?;
        if std::str::from_utf8(&output).ok().map(str::trim) != Some(name) {
            return Err(Failure::Runtime);
        }
        self.volume(name, generation, role).await
    }
    pub async fn volume(
        &self,
        name: &str,
        generation: &str,
        role: &str,
    ) -> Result<VolumeIdentity, Failure> {
        self.verify().await?;
        let bytes = self
            .query(
                &["volume", "inspect", "--format", "{{json .}}", name],
                16384,
            )
            .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Runtime)?;
        if value["Name"] != name
            || value["Driver"] != "local"
            || value["Scope"] != "local"
            || !value.get("Options").is_none_or(|options| {
                options.is_null() || options.as_object().is_some_and(serde_json::Map::is_empty)
            })
            || value["Labels"]["ai-guard.model-tls.generation"] != generation
            || value["Labels"]["ai-guard.model-tls.role"] != role
        {
            return Err(Failure::Runtime);
        }
        let mountpoint = value["Mountpoint"]
            .as_str()
            .filter(|path| {
                Path::new(path).is_absolute()
                    && path.len() <= 4096
                    && !path.chars().any(char::is_control)
            })
            .ok_or(Failure::Runtime)?;
        let created = value["CreatedAt"]
            .as_str()
            .filter(|value| {
                !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
            })
            .ok_or(Failure::Runtime)?;
        Ok(VolumeIdentity {
            name: name.into(),
            mountpoint: mountpoint.into(),
            created_at: created.into(),
        })
    }
    pub async fn create_preparer(&self, image: &str, attempt: &mut Attempt) -> Result<(), Failure> {
        self.verify().await?;
        self.inspect_image(image).await?;
        let args = preparer_args(self.runtime.kind, image, attempt);
        attempt.stop_observation = "creation_unknown";
        let bytes = self.query_owned(&args, None, 4096, &self.cancel).await?;
        let cid = std::str::from_utf8(&bytes)
            .map_err(|_| Failure::Runtime)?
            .trim();
        if !hex_id(cid) {
            return Err(Failure::Runtime);
        }
        attempt.cid = Some(cid.into());
        self.inspect_preparer(attempt, image).await?;
        Ok(())
    }
    async fn inspect_preparer(&self, attempt: &Attempt, image: &str) -> Result<Value, Failure> {
        let target = attempt.cid.as_deref().unwrap_or(&attempt.preparer);
        let bytes = self
            .query(&["inspect", "--format", "{{json .}}", target], 32768)
            .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Runtime)?;
        if !matches_runtime_image(self.runtime.kind, &value["Image"], image)
            || value["Config"]["Labels"]["ai-guard.model-tls.generation"] != attempt.generation
            || value["Config"]["Labels"]["ai-guard.model-tls.role"] != "preparer"
            || value["Config"]["User"] != "1000:1000"
            || value["HostConfig"]["Privileged"] != false
            || value["HostConfig"]["ReadonlyRootfs"] != true
            || value["HostConfig"]["NetworkMode"] != "none"
        {
            return Err(Failure::Runtime);
        }
        let host = &value["HostConfig"];
        let empty = |field: &Value| field.is_null() || field.as_array().is_some_and(Vec::is_empty);
        let caps = host["CapDrop"].as_array().is_some_and(|caps| {
            caps.len() == 1
                && caps[0]
                    .as_str()
                    .is_some_and(|cap| cap.eq_ignore_ascii_case("ALL"))
        });
        let nnp = host["SecurityOpt"].as_array().is_some_and(|options| {
            options.len() == 1
                && options[0].as_str().is_some_and(|value| {
                    value == "no-new-privileges" || value == "no-new-privileges:true"
                })
        });
        let cpu = host["NanoCpus"] == 1_000_000_000_u64
            || (host["CpuQuota"]
                .as_u64()
                .zip(host["CpuPeriod"].as_u64())
                .is_some_and(|(quota, period)| quota > 0 && quota == period));
        if !caps
            || !nnp
            || !cpu
            || !empty(&host["CapAdd"])
            || !empty(&host["Binds"])
            || host["Memory"] != 192_u64 * 1024 * 1024
            || host["MemorySwap"] != 192_u64 * 1024 * 1024
            || host["PidsLimit"] != 64
            || value["Config"]["Entrypoint"] != serde_json::json!(["/usr/local/bin/agentsandbox"])
            || value["Config"]["Cmd"] != serde_json::json!(["__model-tls-import"])
            || host["LogConfig"]["Type"] != "none"
        {
            return Err(Failure::Runtime);
        }
        let mounts = value["Mounts"].as_array().ok_or(Failure::Runtime)?;
        if mounts.len() != 2 {
            return Err(Failure::Runtime);
        }
        for (name, destination) in [
            (&attempt.volumes.ca, "/data"),
            (&attempt.volumes.identity, "/run/secrets"),
        ] {
            if !mounts.iter().any(|mount| {
                mount["Type"] == "volume"
                    && mount["Name"] == *name
                    && mount["Destination"] == destination
                    && mount["RW"] == true
            }) {
                return Err(Failure::Runtime);
            }
        }
        Ok(value)
    }
    pub async fn import(
        &self,
        image: &str,
        attempt: &mut Attempt,
        packet: &[u8],
    ) -> Result<Vec<u8>, Failure> {
        self.verify().await?;
        self.inspect_preparer(attempt, image).await?;
        let cid = attempt.cid.as_deref().ok_or(Failure::Runtime)?;
        let args = vec![
            "start".into(),
            "--attach".into(),
            "--interactive".into(),
            cid.into(),
        ];
        let result = self
            .query_owned(&args, Some(packet), 4096, &self.cancel)
            .await;
        if result.is_ok() {
            let observed = self.inspect_preparer(attempt, image).await?;
            if observed["State"]["Running"] != false || observed["State"]["ExitCode"] != 0 {
                return Err(Failure::Runtime);
            }
            attempt.stop_observation = "exited_zero_observed";
        }
        result
    }
    pub async fn stop_after_failure(
        &self,
        image: &str,
        attempt: &mut Attempt,
    ) -> Result<(), Failure> {
        if attempt.stop_observation == "not_created" {
            return Ok(());
        }
        // This cleanup is attempted even after operator cancellation. It has
        // the same remaining global budget; unavailable confirmation stays unknown.
        let cleanup = Cancellation::default();
        let info_args = if self.runtime.kind == RuntimeKind::Docker {
            vec!["info".into(), "--format".into(), "{{json .}}".into()]
        } else {
            vec!["info".into(), "--format".into(), "json".into()]
        };
        let authority = self
            .query_owned(&info_args, None, 65536, &cleanup)
            .await
            .and_then(|bytes| self.authority_from(&bytes))?;
        if Some(&authority) != self.authority.as_ref() {
            return Err(Failure::Runtime);
        }
        let target = attempt.cid.as_deref().unwrap_or(&attempt.preparer);
        let inspect = vec![
            "inspect".into(),
            "--format".into(),
            "{{json .}}".into(),
            target.into(),
        ];
        let bytes = self.query_owned(&inspect, None, 32768, &cleanup).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Runtime)?;
        if !matches_runtime_image(self.runtime.kind, &value["Image"], image)
            || value["Config"]["Labels"]["ai-guard.model-tls.generation"] != attempt.generation
            || value["Config"]["Labels"]["ai-guard.model-tls.role"] != "preparer"
        {
            return Err(Failure::Runtime);
        }
        let cid = value["Id"]
            .as_str()
            .filter(|id| hex_id(id))
            .ok_or(Failure::Runtime)?;
        attempt.cid = Some(cid.to_owned());
        if value["State"]["Running"] == false {
            attempt.stop_observation = "not_running_observed";
            return Ok(());
        }
        let args = vec!["stop".into(), "--time".into(), "1".into(), cid.into()];
        self.query_owned(&args, None, 4096, &cleanup).await?;
        let bytes = self.query_owned(&inspect, None, 32768, &cleanup).await?;
        let stopped = serde_json::from_slice::<Value>(&bytes)
            .is_ok_and(|value| value["Id"] == cid && value["State"]["Running"] == false);
        if stopped {
            attempt.stop_observation = "not_running_observed";
            Ok(())
        } else {
            Err(Failure::Runtime)
        }
    }
}

pub(crate) fn preparer_args(kind: RuntimeKind, image: &str, attempt: &Attempt) -> Vec<String> {
    let mut args = vec![
        "create".into(),
        "--name".into(),
        attempt.preparer.clone(),
        "--pull=never".into(),
        "--interactive".into(),
        "--read-only".into(),
        "--user".into(),
        "1000:1000".into(),
        "--network".into(),
        "none".into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--memory".into(),
        "192m".into(),
        "--memory-swap".into(),
        "192m".into(),
        "--cpus".into(),
        "1".into(),
        "--pids-limit".into(),
        "64".into(),
        "--log-driver".into(),
        "none".into(),
        "--label".into(),
        format!("ai-guard.model-tls.generation={}", attempt.generation),
        "--label".into(),
        "ai-guard.model-tls.role=preparer".into(),
        "--mount".into(),
        format!("type=volume,source={},target=/data", attempt.volumes.ca),
        "--mount".into(),
        format!(
            "type=volume,source={},target=/run/secrets",
            attempt.volumes.identity
        ),
        "--entrypoint".into(),
        "/usr/local/bin/agentsandbox".into(),
    ];
    if kind == RuntimeKind::Podman {
        args.extend(["--userns".into(), "keep-id:uid=1000,gid=1000".into()]);
    }
    args.extend([image.into(), "__model-tls-import".into()]);
    args
}
