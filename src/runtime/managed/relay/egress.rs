//! Pipe attachment for the egress helper of a retained prearmed bootstrap.
#![allow(dead_code)] // The E3 callback is composed separately.

use super::*;
use crate::{
    egress::proxy_env::proxy_environment, provider::BOOTSTRAP_PROGRAM,
    runtime::bootstrap_stdin::PreparedBootstrapStdin,
};
use std::sync::{Mutex, PoisonError};

impl ManagedRuntime {
    /// Spawn only the loopback egress helper of this retained bootstrap. The
    /// token attests the retained local stdin and its observed incarnation.
    /// The callback owns the held challenge, channel admission, epoch and
    /// STOP ordering. This attachment cannot write to the bootstrap's stdin.
    pub(crate) async fn open_owned_egress_proxy(
        &self,
        bootstrap: &PreparedBootstrapStdin,
        authorized: &ContainerSpec,
    ) -> std::result::Result<(EgressExecOwner, EgressExecIo), RelayExecError> {
        let snapshot = bootstrap.owned();
        self.validate_handle(&snapshot.handle)
            .map_err(|_| RelayExecError::Authority)?;
        validate_running_snapshot(snapshot, &snapshot.observation)?;
        let held = authorized
            .held_bootstrap_for(true)
            .map_err(|_| RelayExecError::Profile)?
            .ok_or(RelayExecError::Profile)?;
        if &held.attempt != bootstrap.attempt() {
            return Err(RelayExecError::Identity);
        }
        if held.runtime != self.binding.kind || held.authority_id != self.binding.authority_id {
            return Err(RelayExecError::Authority);
        }
        if snapshot.handle.session != authorized.session
            || snapshot.handle.operation != authorized.operation_id()
            || snapshot.handle.fingerprint_version() != authorized.fingerprint_version()
            || !matches!(authorized.argv.as_slice(), [program, verb, ..]
                if program == BOOTSTRAP_PROGRAM && verb == "exec")
        {
            return Err(RelayExecError::Profile);
        }
        let profile = self.relay_profile()?;
        let facts = self
            .inspect_facts(&snapshot.handle.id)
            .await
            .map_err(|_| RelayExecError::Authority)?;
        let observed = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            snapshot.handle.fingerprint_version(),
        )
        .map_err(|_| RelayExecError::Identity)?;
        validate_running_snapshot(snapshot, &observed)?;
        validate_authorized_identity(authorized, &observed).map_err(|_| RelayExecError::Profile)?;
        validate_configuration_for(authorized, &facts.bytes, self.binding.kind, Some(held))
            .map_err(|_| RelayExecError::Profile)?;
        validate_egress_helper_environment(&facts.bytes, held)?;
        authorized
            .validate_secret_env_file()
            .map_err(|_| RelayExecError::Profile)?;
        let (mut command, retained) = self.relay_command()?;
        command.args(helper_exec_args(
            self.binding.kind,
            &snapshot.handle.id,
            RelayHelperMode::Egress(held.proxy_port),
        )?);
        if self.relay_profile()? != profile {
            return Err(RelayExecError::Profile);
        }
        // inspect_facts checked engine authority before inspecting this CID.
        // Keep the final local checks and spawn in the same poll. The engine
        // still owns exec creation, and spawn does not attest READY.
        self.runtime
            .anchor
            .verify(self.runtime.invocation(), self.binding.kind)
            .map_err(|_| RelayExecError::Authority)?;
        let transport = RelayExecTransport::spawn(command, snapshot.clone(), retained)?;
        Ok(EgressExecOwner::split(transport))
    }
}

fn validate_egress_helper_environment(
    bytes: &[u8],
    held: &HeldBootstrapBinding,
) -> std::result::Result<(), RelayExecError> {
    let mut required = proxy_environment(held.proxy_port);
    required.push((HELD_CHALLENGE_ENV, held.attempt.as_str().to_owned()));
    validate_helper_environment_for(bytes, &required)
}

struct EgressPipes {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}

type SharedPipes = Arc<Mutex<Option<EgressPipes>>>;

fn close_pipes(pipes: &SharedPipes) {
    let closed = pipes.lock().unwrap_or_else(PoisonError::into_inner).take();
    drop(closed);
}

/// The callback retains this owner even if the channel drops its IO during
/// handshake or activation. The owner can revoke both pipes without waiting
/// for the channel to hand them back.
#[must_use = "retain the exec owner through close and reconciliation"]
pub(crate) struct EgressExecOwner {
    transport: RelayExecTransport,
    pipes: SharedPipes,
}

impl EgressExecOwner {
    fn split(mut transport: RelayExecTransport) -> (Self, EgressExecIo) {
        let pipes = Arc::new(Mutex::new(Some(EgressPipes {
            stdin: transport.stdin.take(),
            stdout: transport.stdout.take(),
        })));
        let io = EgressExecIo {
            pipes: Arc::clone(&pipes),
            health: Arc::clone(&transport.health),
        };
        (Self { transport, pipes }, io)
    }

    /// Local pipe health only. READY belongs to the egress protocol.
    pub(crate) fn state(&self) -> RelayExecState {
        self.transport.state()
    }

    /// Join only local CLI/drainer tasks within the existing relay bound.
    /// Outstanding joins stay with this owner for a later attempt.
    pub(crate) async fn close_and_join(&mut self) -> RelayExecClosure {
        close_pipes(&self.pipes);
        self.transport.close_and_join().await
    }

    /// A local join alone cannot prove that the remote helper ended. This
    /// observes the exact worker terminal and has no audit or quota effects.
    pub(crate) async fn reconcile_stopped(
        &mut self,
        runtime: &ManagedRuntime,
    ) -> std::result::Result<ContainerObservation, RelayExecError> {
        self.transport.reconcile_stopped(runtime).await
    }
}

impl Drop for EgressExecOwner {
    fn drop(&mut self) {
        close_pipes(&self.pipes);
    }
}

/// The channel can take this owned duplex IO. Polls hold the lock only while
/// accessing the local pipes. The exec owner retains process custody.
pub(crate) struct EgressExecIo {
    pipes: SharedPipes,
    health: Arc<Health>,
}

impl EgressExecIo {
    fn poll_pipes<T>(
        &self,
        poll: impl FnOnce(&mut EgressPipes) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        if let Ok(mut pipes) = self.pipes.lock()
            && let Some(pipes) = pipes.as_mut()
        {
            return poll(pipes);
        }
        Poll::Ready(Err(closed_pipe()))
    }

    fn poll_writer<T>(
        &self,
        poll: impl FnOnce(Pin<&mut RelayExecWriter<'_>>) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        self.poll_pipes(|pipes| {
            let mut writer = RelayExecWriter {
                pipe: &mut pipes.stdin,
                health: &self.health,
            };
            poll(Pin::new(&mut writer))
        })
    }
}

fn closed_pipe() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "egress helper pipes closed")
}

impl AsyncRead for EgressExecIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_pipes(|pipes| {
            let Some(pipe) = pipes.stdout.as_mut() else {
                return Poll::Ready(Err(closed_pipe()));
            };
            let mut reader = RelayExecReader {
                pipe,
                health: &self.health,
            };
            Pin::new(&mut reader).poll_read(cx, buffer)
        })
    }
}

impl AsyncWrite for EgressExecIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_writer(|writer| writer.poll_write(cx, bytes))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.poll_writer(|writer| writer.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let mut pipes = self.pipes.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pipes) = pipes.as_mut() {
            drop(pipes.stdin.take());
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for EgressExecIo {
    fn drop(&mut self) {
        close_pipes(&self.pipes);
        self.health.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn owned_proxy_can_revoke_handed_off_pipes_and_join_only_the_cli() {
        fn channel_io(_: &(impl AsyncRead + AsyncWrite + Send + 'static)) {}

        // Only the stdio owner is under test. No engine or worker runs here.
        let snapshot = OwnedContainerSnapshot {
            handle: ContainerHandle {
                authority_fingerprint: "a".repeat(64),
                session: Uuid::from_u128(1),
                operation: Uuid::from_u128(2),
                id: "b".repeat(64),
                name: "proxy-pipe-fixture".into(),
                configuration_fingerprint_version: Some(2),
            },
            observation: parse_container_observation_version(b"{}", RuntimeKind::Docker, 2)
                .unwrap(),
        };
        let mut command = Command::new("/bin/cat");
        command.env_clear();
        let transport = RelayExecTransport::spawn(command, snapshot, None).unwrap();
        let (mut owner, mut io) = EgressExecOwner::split(transport);
        channel_io(&io);
        tokio::time::timeout(Duration::from_secs(2), async {
            io.write_all(b"proxy-pipe").await.unwrap();
            let mut reply = [0; 10];
            io.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"proxy-pipe");
        })
        .await
        .unwrap();

        let closure = owner.close_and_join().await;
        assert!(closure.local_joined);
        assert_eq!(closure.cli_exit_code, Some(0));
        assert_eq!(owner.state(), RelayExecState::OutcomeUnknown);
        assert_eq!(
            io.write_all(b"closed").await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn owned_proxy_exec_uses_only_the_approved_cid_and_fixed_loopback_helper() {
        let cid = "a".repeat(64);
        let args =
            helper_exec_args(RuntimeKind::Docker, &cid, RelayHelperMode::Egress(43128)).unwrap();
        assert_eq!(
            args,
            [
                "exec",
                "--interactive",
                "--tty=false",
                "--privileged=false",
                "--env=AI_GUARD_PROVIDER_BOOTSTRAP=",
                "--env=OPENAI_API_KEY=",
                "--env=CODEX_API_KEY=",
                "--env=ANTHROPIC_API_KEY=",
                "--env=CLAUDE_CODE_OAUTH_TOKEN=",
                "--env=CODEX_HOME=",
                "--env=CLAUDE_CONFIG_DIR=",
                "--env=AI_GUARD_EGRESS_HELD_V1=",
                "--env=HTTPS_PROXY=",
                "--env=https_proxy=",
                "--env=HTTP_PROXY=",
                "--env=http_proxy=",
                "--env=NO_PROXY=",
                "--env=no_proxy=",
                "--env=AI_GUARD_EGRESS_PROXY=",
                "--user",
                "1000:1000",
                cid.as_str(),
                "/usr/local/bin/ai-guard-relay",
                "serve-egress-v1",
                "--port",
                "43128",
            ]
        );
        let mut podman = args;
        podman.insert(4, "--detach-keys=".into());
        assert_eq!(
            helper_exec_args(RuntimeKind::Podman, &cid, RelayHelperMode::Egress(43128)).unwrap(),
            podman,
        );
        assert_eq!(
            helper_exec_args(
                RuntimeKind::Docker,
                "--latest",
                RelayHelperMode::Egress(43128)
            ),
            Err(RelayExecError::Identity),
        );
    }

    #[test]
    fn owned_proxy_environment_requires_the_exact_loopback_profile() {
        const ATTEMPT: &str = "550e8400-e29b-41d4-a716-446655440000";
        let held = HeldBootstrapBinding {
            attempt: crate::egress::ids::IdV1::parse(ATTEMPT).unwrap(),
            proxy_port: 43128,
            runtime: RuntimeKind::Docker,
            authority_id: Uuid::from_u128(7),
            spec_digest: String::new(),
        };
        let mut env = vec![
            "HOME=/home/agent".into(),
            "AI_GUARD_PROVIDER_BOOTSTRAP=synthetic-private".into(),
        ];
        let inspect = |env: &[String]| serde_json::to_vec(&json!({"Config":{"Env":env}})).unwrap();
        assert_eq!(validate_helper_environment(&inspect(&env)), Ok(()));
        let challenge = format!("AI_GUARD_EGRESS_HELD_V1={ATTEMPT}");
        env.push(challenge.clone());
        assert_eq!(
            validate_helper_environment(&inspect(&env)),
            Err(RelayExecError::Profile)
        );
        env.extend(
            proxy_environment(43128)
                .into_iter()
                .map(|(key, value)| format!("{key}={value}")),
        );
        let validate = |env: &[String]| validate_egress_helper_environment(&inspect(env), &held);
        assert_eq!(validate(&env), Ok(()));
        for unexpected in [
            "ALL_PROXY=http://127.0.0.1:43128",
            "AI_GUARD_EGRESS_PROXY=http://127.0.0.1:43128",
            "AI_GUARD_PROVIDER_BOOTSTRAP=second-synthetic-private",
            challenge.as_str(),
        ] {
            let mut with_extra = env.clone();
            with_extra.push(unexpected.into());
            assert_eq!(validate(&with_extra), Err(RelayExecError::Profile));
        }
        let mut without_challenge = env.clone();
        without_challenge.retain(|entry| entry != &challenge);
        assert_eq!(validate(&without_challenge), Err(RelayExecError::Profile));
        for value in ["", "550e8400-e29b-41d4-a716-446655440001"] {
            let mut with_wrong_challenge = without_challenge.clone();
            with_wrong_challenge.push(format!("AI_GUARD_EGRESS_HELD_V1={value}"));
            let error = validate(&with_wrong_challenge).unwrap_err();
            assert_eq!(error, RelayExecError::Profile);
            assert!(!format!("{error:?} {error}").contains("synthetic-private"));
        }

        let marker = env.pop().unwrap();
        assert!(marker.starts_with("AI_GUARD_EGRESS_PROXY="));
        assert_eq!(validate(&env), Err(RelayExecError::Profile));
        env.push("AI_GUARD_EGRESS_PROXY=http://127.0.0.1:3128".into());
        assert_eq!(validate(&env), Err(RelayExecError::Profile));
    }
}
