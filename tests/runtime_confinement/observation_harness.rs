//! Test custody only. No gateway audit, provider or production qualification.
use agentsandbox::runtime::{
    self, Capture, ContainerObservation, ContainerSpec, ExecutionResult, ManagedRetainedExecution,
    ManagedRuntime, OwnedContainerSnapshot, Runtime, RuntimeKind, StartDirective,
    observation::{ObservationCollector, Phase},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    future::Future,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub fn receipt(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

pub struct Qualification {
    pub managed: ManagedRuntime,
    pub raw: Runtime,
    pub evidence: PathBuf,
}
impl Qualification {
    pub async fn create_volume(&self, session: Uuid) -> Result<()> {
        self.managed
            .create_named_volume(
                &runtime::volume_name(session),
                &[("agentsandbox.qualification".into(), session.to_string())],
            )
            .await
    }
    /// Only read-only inspect/ps probes, using the pinned program and exactly
    /// the endpoint already selected/verified by ManagedRuntime.
    pub async fn cli(&self, args: &[String], duration: Duration) -> Result<Capture> {
        ensure!(
            matches!(args.first().map(String::as_str), Some("inspect" | "ps")),
            "qualification probe is read-only"
        );
        let binding = self.managed.binding();
        let mut pinned = match binding.kind {
            RuntimeKind::Docker => vec!["--host".into(), binding.endpoint.clone()],
            RuntimeKind::Podman if binding.endpoint == "local" => vec!["--remote=false".into()],
            RuntimeKind::Podman => {
                vec!["--remote".into(), "--url".into(), binding.endpoint.clone()]
            }
        };
        pinned.extend_from_slice(args);
        // Same retained Child/pipe owner as ordinary control commands. Timeout
        // requests termination of only this CLI transport, then collects that
        // same wait and both readers; there is no consuming timeout here.
        self.raw.cli(&pinned, duration).await
    }
    pub async fn execute<F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<fs::File>,
        duration: Duration,
        stdout_limit: usize,
        mut on_started: F,
    ) -> Result<ExecutionResult>
    where
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let operation = spec.operation_id();
        receipt(
            &self.evidence.join(format!("{operation}-intent.json")),
            &serde_json::json!({"operation": operation, "session": spec.session, "requested_timeout_ms": duration.as_millis(), "status":"INCONCLUSIVE"}),
        )?;
        let image = self
            .managed
            .resolve_image_observed(operation, &spec.image)
            .await?;
        let mut spec = spec.clone();
        spec.image = image;
        let name = spec.name();
        let volume = runtime::volume_name(spec.session);
        let spec = spec.with_managed_names(&name, &volume)?;
        let owned = RefCell::new(None::<OwnedContainerSnapshot>);
        let result = self
            .managed
            .execute_retained(
                &spec,
                input,
                duration,
                stdout_limit,
                |created| {
                    let saved = receipt(
                        &self.evidence.join(format!("{operation}-created.json")),
                        &created,
                    );
                    owned.replace(Some(created));
                    async move {
                        saved?;
                        Ok(StartDirective::EmitOnce)
                    }
                },
                |state| on_started(state),
            )
            .await;
        let created = owned.into_inner();
        match result {
            Ok(retained) => {
                let r = retained.result();
                // Preserve the public test's output assertions after native cleanup.
                let result = ExecutionResult {
                    stdout: copy_capture(&r.stdout),
                    stderr: copy_capture(&r.stderr),
                    state: r.state.clone(),
                    exit_code: r.exit_code,
                    timed_out: r.timed_out,
                    duration_ms: r.duration_ms,
                    transport_stop_reason: r.transport_stop_reason.clone(),
                };
                let cleanup: Result<()> = async {
                    receipt(
                        &self.evidence.join(format!("{operation}-terminal.json")),
                        retained.terminal(),
                    )?;
                    self.managed.cleanup(&retained).await?;
                    receipt(
                        &self.evidence.join(format!("{operation}-cleanup.json")),
                        &serde_json::json!({"id":retained.handle().id(),"operation":operation,"status":"OWNED_TERMINAL_RM_RETURNED_OK"}),
                    )
                }.await;
                match cleanup {
                    Ok(()) => Ok(result),
                    Err(primary) => Err(ExecutionFailure {
                        primary,
                        secondary: Vec::new(),
                        created,
                        terminal: None,
                        // Retain the actual successful execution, captures and
                        // terminal even when receipt/cleanup subsequently fails.
                        retained: Some(retained),
                    }
                    .into()),
                }
            }
            Err(primary) => {
                let mut terminal = None;
                // This secondary attempt never replaces the real primary error
                // (which may itself own the physical IO and concurrent causes).
                let cleanup = match created.as_ref() {
                    Some(created) => self.cleanup_created(created, &mut terminal).await,
                    None => Ok(()),
                };
                Err(retain_execution_error(primary, created, terminal, cleanup))
            }
        }
    }

    async fn cleanup_created(
        &self,
        created: &OwnedContainerSnapshot,
        terminal: &mut Option<ContainerObservation>,
    ) -> Result<()> {
        let operation = created.handle.operation_id();
        // Errors do not authorize removal. Store the actual counter-observation
        // before validation, so unknown/nonterminal evidence is also retained.
        *terminal = Some(self.managed.observe(&created.handle).await?);
        let observed = terminal.as_ref().context("terminal observation missing")?;
        observed.validate_terminal(self.managed.binding().kind, &created.observation)?;
        receipt(
            &self.evidence.join(format!("{operation}-terminal.json")),
            observed,
        )?;
        self.managed
            .cleanup_terminal(&created.handle, observed)
            .await?;
        receipt(
            &self.evidence.join(format!("{operation}-cleanup.json")),
            &serde_json::json!({"id":created.handle.id(),"operation":operation,"status":"OWNED_TERMINAL_RM_RETURNED_OK"}),
        )
    }
}

struct StageFailure {
    stage: &'static str,
    error: anyhow::Error,
}
impl std::fmt::Debug for StageFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("StageFailure").field(&self.stage).finish()
    }
}

/// Own the errors and the actual snapshots/results; formatting never copies
/// captures into a receipt or substitutes a string for the physical owner.
struct ExecutionFailure {
    primary: anyhow::Error,
    secondary: Vec<StageFailure>,
    created: Option<OwnedContainerSnapshot>,
    terminal: Option<ContainerObservation>,
    retained: Option<ManagedRetainedExecution>,
}
fn retain_execution_error(
    primary: anyhow::Error,
    created: Option<OwnedContainerSnapshot>,
    terminal: Option<ContainerObservation>,
    cleanup: Result<()>,
) -> anyhow::Error {
    ExecutionFailure {
        primary,
        secondary: cleanup
            .err()
            .into_iter()
            .map(|error| StageFailure {
                stage: "error cleanup",
                error,
            })
            .collect(),
        created,
        terminal,
        retained: None,
    }
    .into()
}

impl std::fmt::Display for ExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "qualification execution/cleanup failed; original causes and resources retained",
        )
    }
}
impl std::fmt::Debug for ExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionFailure")
            .field("secondary", &self.secondary)
            .field("created_retained", &self.created.is_some())
            .field("observation_retained", &self.terminal.is_some())
            .field("execution_retained", &self.retained.is_some())
            .finish()
    }
}
impl std::error::Error for ExecutionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

struct CaseFailure {
    causes: Vec<StageFailure>,
    events: Option<Vec<runtime::observation::Event>>,
}
impl std::fmt::Display for CaseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("qualification case failed; all returned causes and evidence retained")
    }
}
impl std::fmt::Debug for CaseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaseFailure")
            .field("causes", &self.causes)
            .field("events_retained", &self.events.as_ref().map(Vec::len))
            .finish()
    }
}
impl std::error::Error for CaseFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.causes
            .first()
            .map(|cause| cause.error.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Called only after writer.await. No sequential ? may discard another result.
pub fn finish_case(
    outcome: Result<()>,
    inventory: Result<()>,
    diagnostics: Result<Vec<runtime::observation::Event>>,
    finish: Result<()>,
    receipt: Result<()>,
    timeline: Result<()>,
) -> Result<Vec<runtime::observation::Event>> {
    let mut causes = Vec::new();
    for (stage, result) in [("case", outcome), ("inventory", inventory)] {
        if let Err(error) = result {
            causes.push(StageFailure { stage, error });
        }
    }
    let events = match diagnostics {
        Ok(events) => Some(events),
        Err(error) => {
            causes.push(StageFailure {
                stage: "diagnostic consumer",
                error,
            });
            None
        }
    };
    for (stage, result) in [
        ("consumer finish signal", finish),
        ("case receipt", receipt),
        ("timeline", timeline),
    ] {
        if let Err(error) = result {
            causes.push(StageFailure { stage, error });
        }
    }
    match events {
        Some(events) if causes.is_empty() => Ok(events),
        events => Err(CaseFailure { causes, events }.into()),
    }
}

fn copy_capture(c: &Capture) -> Capture {
    Capture {
        bytes: c.bytes.clone(),
        total_bytes: c.total_bytes,
        truncated: c.truncated,
    }
}

/// File IO is owned by a separate async consumer; no producer awaits it.
/// The initial verdict remains INCONCLUSIVE even after process interruption.
pub async fn persist(
    mut collector: ObservationCollector,
    path: PathBuf,
    mut finish: tokio::sync::oneshot::Receiver<()>,
) -> Result<Vec<runtime::observation::Event>> {
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    let mut file = tokio::fs::File::from_std(file);
    let mut events = Vec::new();
    let mut bytes = 0usize;
    let outcome: Result<()> = async {
        loop {
            let event = tokio::select! {
                event = collector.recv() => event,
                _ = &mut finish => break,
            };
            let Some(event) = event else { break };
            append(&mut file, &event, &mut bytes).await?;
            ensure!(
                events.len() < 4096,
                "observation retention exceeded bound; INCONCLUSIVE"
            );
            events.push(event);
        }
        while let Some(event) = collector.try_recv() {
            append(&mut file, &event, &mut bytes).await?;
            ensure!(
                events.len() < 4096,
                "observation retention exceeded bound; INCONCLUSIVE"
            );
            events.push(event);
        }
        file.sync_all().await?;
        ensure!(
            !collector.lost(),
            "diagnostic loss/disconnection; INCONCLUSIVE"
        );
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => Ok(events),
        Err(primary) => Err(ConsumerFailure { primary, events }.into()),
    }
}

/// Partial diagnostic events stay owned with an actual consumer error. They
/// never become a successful timeline, and no capture/payload is added here.
struct ConsumerFailure {
    primary: anyhow::Error,
    events: Vec<runtime::observation::Event>,
}
impl std::fmt::Display for ConsumerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("diagnostic consumer failed; partial events retained; INCONCLUSIVE")
    }
}
impl std::fmt::Debug for ConsumerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsumerFailure")
            .field("events_retained", &self.events.len())
            .finish()
    }
}
impl std::error::Error for ConsumerFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

async fn append(
    file: &mut tokio::fs::File,
    event: &runtime::observation::Event,
    bytes: &mut usize,
) -> Result<()> {
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    *bytes += line.len();
    ensure!(
        *bytes <= 1024 * 1024,
        "diagnostic file cap exceeded; INCONCLUSIVE"
    );
    file.write_all(&line).await?;
    Ok(())
}

pub fn require_timeout_timeline(
    events: &[runtime::observation::Event],
    operation: Uuid,
) -> Result<()> {
    let events: Vec<_> = events.iter().filter(|e| e.operation == operation).collect();
    let required = [
        Phase::ImageResolutionRequested,
        Phase::ImageResolutionReturned,
        Phase::Submitted,
        Phase::CreateRequested,
        Phase::Created,
        Phase::StartSpawnRequested,
        Phase::StartSpawnReturned,
        Phase::DeadlineArmed,
        Phase::DeadlineObservedByPoller,
        Phase::StopRequested,
        Phase::StopInvocationRequested,
        Phase::StopSpawnRequested,
        Phase::StopSpawnReturned,
        Phase::StopCommandExitedOk,
        Phase::StopInvocationReturnedOk,
        Phase::AttachExited,
        Phase::StdoutEof,
        Phase::StderrEof,
        Phase::TerminalInspected,
        Phase::DrainCollected,
        Phase::OperationReturnedOk,
        Phase::CleanupRequested,
        Phase::CleanupReturnedOk,
    ];
    let first = events
        .first()
        .context("no timeout timeline; INCONCLUSIVE")?;
    let created = events
        .iter()
        .find(|e| e.phase == Phase::Created)
        .context("no exact Created; INCONCLUSIVE")?;
    let instance = created
        .instance
        .context("Created ID unknown; INCONCLUSIVE")?;
    for phase in required {
        let event = events
            .iter()
            .find(|e| e.phase == phase)
            .with_context(|| format!("missing {phase:?}; INCONCLUSIVE"))?;
        ensure!(
            event.trace_id == first.trace_id,
            "different monotonic origin; INCONCLUSIVE"
        );
        if !matches!(
            phase,
            Phase::ImageResolutionRequested
                | Phase::ImageResolutionReturned
                | Phase::Submitted
                | Phase::CreateRequested
                | Phase::OperationReturnedOk
        ) {
            ensure!(
                event.instance == Some(instance),
                "instance correlation mismatch; INCONCLUSIVE"
            );
        }
    }
    let arm = events
        .iter()
        .find(|e| e.phase == Phase::DeadlineArmed)
        .unwrap();
    let due = arm
        .planned_deadline_ns
        .context("deadline plan absent; INCONCLUSIVE")?;
    let fired = events
        .iter()
        .find(|e| e.phase == Phase::DeadlineObservedByPoller)
        .unwrap();
    ensure!(
        fired.planned_deadline_ns == Some(due) && fired.elapsed_ns >= due,
        "deadline correlation mismatch; INCONCLUSIVE"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn secondary_failures_keep_the_real_primary_cli_capture_and_all_causes() {
        let raw =
            Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
        // A real software CLI failure, with its actual wait and retained captures.
        // No engine or Managed owner is fabricated for this aggregation regression.
        let primary = raw
            .cli(
                &["--rme-invalid-unit-option".into()],
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        let original_stderr = primary
            .downcast_ref::<runtime::RuntimeCommandFailure>()
            .unwrap()
            .stderr_capture()
            .bytes
            .clone();
        assert!(!original_stderr.is_empty());
        let execution = retain_execution_error(
            primary,
            None,
            None,
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into()),
        );
        let failure = finish_case(
            Err(execution),
            Err(std::io::Error::from(std::io::ErrorKind::NotFound).into()),
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe).into()),
            Ok(()),
            Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into()),
            Ok(()),
        )
        .unwrap_err();
        let case = failure.downcast_ref::<CaseFailure>().unwrap();
        assert_eq!(case.causes.len(), 4);
        let retained = case.causes[0]
            .error
            .downcast_ref::<ExecutionFailure>()
            .unwrap();
        let cli = retained
            .primary
            .downcast_ref::<runtime::RuntimeCommandFailure>()
            .unwrap();
        assert_eq!(cli.stderr_capture().bytes, original_stderr);
        assert_eq!(retained.secondary.len(), 1);
        assert!(retained.secondary[0].error.is::<std::io::Error>());
        assert!(
            case.causes[1..]
                .iter()
                .all(|cause| cause.error.is::<std::io::Error>())
        );
        assert!(
            retained.created.is_none()
                && retained.terminal.is_none()
                && retained.retained.is_none()
        );
    }
}
