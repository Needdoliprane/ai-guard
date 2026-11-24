//! Prepared source of one operator `build`, `up` or `down` command.
//!
//! Before any engine build or Compose command, the public inputs of the
//! selected distribution are copied, bounded and verified, into a private
//! directory. The engine CLI is then given only paths inside that copy, and the
//! copy is held until the CLI process is *collected*: its real wait status is
//! known and both of its output pipes reached end of file. The original is
//! read, never written and never named to the engine.
//!
//! `operations` never sees a path into the copy. It receives a
//! [`PreparedSource`], asks it for a [`PreparedCommand`] through a closed
//! argument grammar, and hands that command to the retained consumer. Neither
//! value can be made from a pathname.
//!
//! What this binds is the content the engine CLI is pointed at. It is not
//! trust in the selected distribution, not an atomic snapshot of its tree and
//! not a confinement of the engine CLI process. The control UID, root and the
//! storage and engine administrators remain trusted.
mod build_identity;
mod capture;
mod interrupt;
mod release_archive;
mod release_metadata;
#[cfg(test)]
mod tests;

use super::{OperatorCommand, OperatorRuntime};
use crate::runtime::{
    Capture, ManagedRuntime, Runtime, RuntimeKind,
    retained_cli::{
        COMPOSE_UP_ACTION, Collected, Interruption, InvocationEnd, InvocationFailure,
        RetainedInput, STOP_OBSERVATION, Stop, StopReason, UnknownEnd, signal_name,
    },
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use capture::{Limits, Scope, Snapshot};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Where the recovery procedure of a retained copy is documented.
const RETAINED_GUIDE: &str = "docs/setup/local/README.md#prepared-source-retained";

/// Owned prepared copy of the inputs one operator command hands to the engine.
///
/// Its only constructors run the bounded, filtered, verified preparation. The
/// copy is removed when the last reference to it is dropped: this value and
/// each of its commands inside the retained consumer.
pub(super) struct PreparedSource {
    snapshot: Arc<Snapshot>,
}

/// One engine command bound to a prepared source. Created only by
/// [`PreparedSource`]; its arguments name nothing but the prepared copy.
pub(super) struct PreparedCommand {
    snapshot: Arc<Snapshot>,
    arguments: Vec<String>,
    verb: &'static str,
}

/// Prepares the build inputs of `source`, the canonical path of the selected
/// distribution, then applies the structural rules of a build source to the
/// copy itself.
pub(super) async fn prepare_build(source: &Path) -> Result<PreparedSource> {
    prepare(source, Scope::Build).await
}

/// Prepares exactly `deploy/<kind>/compose.yaml` and an empty environment file.
pub(super) async fn prepare_compose(source: &Path, kind: RuntimeKind) -> Result<PreparedSource> {
    prepare(source, Scope::Compose(kind)).await
}

/// One `release` operator command, on a build source prepared as `build`
/// prepares it. It starts no engine command.
pub(super) async fn release(command: super::ReleaseCommand) -> Result<()> {
    release_metadata::execute(command).await
}

async fn prepare(source: &Path, scope: Scope) -> Result<PreparedSource> {
    let interruption = interrupt::current();
    // Counted on the command's own task for as long as the copy runs off it:
    // an interrupted command is not dropped while files are being written, so
    // the copy stops at its next entry and removes what it created.
    let hold = interruption.as_ref().map(Interruption::hold);
    let source = source.to_owned();
    let stop = interruption.clone();
    let prepared =
        tokio::task::spawn_blocking(move || prepare_blocking(&source, scope, stop.as_ref())).await;
    drop(hold);
    let snapshot = match prepared {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            return Err(match error.downcast_ref::<InvocationFailure>() {
                // The engine may already have been asked about itself: `up`
                // and `down` prepare their source after the engine probes.
                Some(InvocationFailure::Interrupted { signal }) => anyhow!(
                    "interrupted by {} while the source was being prepared; no engine build or \
                     Compose command was started",
                    signal_name(*signal)
                ),
                _ => error,
            });
        }
        Err(_) => {
            bail!(
                "the source preparation ended without a result; no engine build or Compose \
                 command was started"
            )
        }
    };
    Ok(PreparedSource {
        snapshot: Arc::new(snapshot),
    })
}

fn prepare_blocking(source: &Path, scope: Scope, stop: Option<&Interruption>) -> Result<Snapshot> {
    let snapshot = capture::capture(
        source,
        scope,
        Limits::default(),
        &std::env::temp_dir(),
        stop,
    )?
    .confirm(stop)?;
    if scope == Scope::Build {
        super::preflight_build_inputs(snapshot.context())?;
    }
    eprintln!("{}", snapshot.provenance_line());
    Ok(snapshot)
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .context("the prepared-source path must be UTF-8; choose a UTF-8 TMPDIR")
}

/// `^[a-z0-9][a-z0-9._-]*:[A-Za-z0-9._-]+$`: a local image name and a tag.
/// Neither part can start an option or name a path.
fn image_tag(tag: &str) -> bool {
    let Some((name, version)) = tag.split_once(':') else {
        return false;
    };
    let plain = |character: char| matches!(character, '.' | '_' | '-');
    let lower = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    name.chars().next().is_some_and(lower)
        && name
            .chars()
            .all(|character| lower(character) || plain(character))
        && !version.is_empty()
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || plain(character))
}

impl PreparedSource {
    /// `options` must be exactly `["build", "--tag", <tag>]` followed by
    /// `"--build-arg", "<NAME>=<VALUE>"` pairs for `CODEX_VERSION` and
    /// `CLAUDE_CODE_VERSION`, each at most once with an exact version, and
    /// `CLAUDE_ACTIVITY_PROFILE` at most once with `v1` or `v1-onfailure`.
    /// `dockerfile` must be a prepared `images/<name>.Dockerfile`.
    ///
    /// Result: `build --file <copy>/<dockerfile> --tag <tag> [--build-arg …]… <copy>`.
    /// No value of this grammar can name a path the engine reads as input.
    pub(super) fn build_command(
        &self,
        dockerfile: &str,
        options: &[String],
    ) -> Result<PreparedCommand> {
        ensure!(
            self.snapshot.scope() == Scope::Build,
            "this prepared source holds no build inputs"
        );
        let prepared_dockerfile = dockerfile
            .strip_prefix("images/")
            .is_some_and(|name| !name.contains('/') && name.ends_with(".Dockerfile"))
            && self.snapshot.holds(dockerfile);
        ensure!(
            prepared_dockerfile,
            "the Dockerfile is not a prepared build input: {dockerfile}"
        );
        let [verb, flag, tag, build_arguments @ ..] = options else {
            bail!("unsupported engine build options");
        };
        ensure!(
            verb == "build" && flag == "--tag" && image_tag(tag),
            "unsupported engine build options"
        );
        let (pairs, unpaired) = build_arguments.as_chunks::<2>();
        ensure!(unpaired.is_empty(), "unsupported engine build options");
        let mut seen = Vec::new();
        for [option, assignment] in pairs {
            let assignment = assignment.split_once('=');
            let Some((name, value)) = assignment.filter(|_| option == "--build-arg") else {
                bail!("unsupported engine build options");
            };
            ensure!(
                matches!(
                    name,
                    "CODEX_VERSION" | "CLAUDE_CODE_VERSION" | "CLAUDE_ACTIVITY_PROFILE"
                ) && !seen.contains(&name),
                "unsupported engine build argument"
            );
            if name == "CLAUDE_ACTIVITY_PROFILE" {
                ensure!(
                    matches!(value, "v1" | "v1-onfailure"),
                    "unsupported engine build argument"
                );
            } else {
                super::validate_provider_version(value)?;
            }
            seen.push(name);
        }
        let context = utf8(self.snapshot.context())?;
        let mut arguments = vec![
            "build".to_owned(),
            "--file".to_owned(),
            utf8(&self.snapshot.context().join(dockerfile))?.to_owned(),
            "--tag".to_owned(),
            tag.clone(),
        ];
        arguments.extend(build_arguments.iter().cloned());
        arguments.push(context.to_owned());
        Ok(PreparedCommand {
            snapshot: Arc::clone(&self.snapshot),
            arguments,
            verb: "build",
        })
    }

    /// `options` starts with `["compose", "--project-name", <stack>]`, followed
    /// by `up --detach --force-recreate` or by `down`.
    ///
    /// Result: `compose --project-name <stack> --env-file <empty prepared
    /// file> --file <copy>/deploy/<kind>/compose.yaml <action…>`, where the stack
    /// name is `agentsandbox` unless the operator chose another valid one.
    pub(super) fn compose_command(&self, options: &[String]) -> Result<PreparedCommand> {
        let prepared = self
            .snapshot
            .scope()
            .compose_file()
            .filter(|file| self.snapshot.holds(file))
            .zip(self.snapshot.env_file());
        let Some((file, env_file)) = prepared else {
            bail!("this prepared source holds no Compose file");
        };
        let [verb, flag, project, action @ ..] = options else {
            bail!("unsupported engine Compose options");
        };
        let action: Vec<&str> = action.iter().map(String::as_str).collect();
        ensure!(
            verb == "compose"
                && flag == "--project-name"
                && super::parse_stack_name(project).is_ok()
                && (action == COMPOSE_UP_ACTION || action == ["down"]),
            "unsupported engine Compose options"
        );
        let mut arguments = vec![
            "compose".to_owned(),
            "--project-name".to_owned(),
            project.clone(),
            "--env-file".to_owned(),
            utf8(env_file)?.to_owned(),
            "--file".to_owned(),
            utf8(&self.snapshot.context().join(file))?.to_owned(),
        ];
        arguments.extend(action.into_iter().map(str::to_owned));
        Ok(PreparedCommand {
            snapshot: Arc::clone(&self.snapshot),
            arguments,
            verb: "compose",
        })
    }
}

impl RetainedInput for PreparedCommand {
    fn arguments(&self) -> Vec<String> {
        self.arguments.clone()
    }
    fn verify_before_spawn(&self) -> Result<()> {
        self.snapshot.verify()
    }
    fn verify_after_collection(&self) -> Result<()> {
        self.snapshot.verify()
    }
    fn termination_unknown(&self, end: &UnknownEnd) -> Option<PathBuf> {
        Some(self.snapshot.retain_unknown(self.verb, end))
    }
}

impl OperatorRuntime {
    /// The authority checks of `cli_env`, then the retained consumer: the
    /// Docker engine identity is re-read, the command targets the endpoint
    /// that was approved, and the configuration directory still is the one
    /// that was opened.
    pub(super) async fn consume_prepared(
        &self,
        mut command: PreparedCommand,
        duration: Duration,
        environment: &[(&str, &str)],
        directory: Option<&crate::control_fs::Directory>,
    ) -> Result<Capture> {
        inject_podman_build_ignorefile(self.approved.kind, &mut command)?;
        let mut prefix = Vec::new();
        if let Some(runtime) = &self.docker {
            // The same read-only identity check as managed recovery. The check
            // and the CLI effect are separate engine connections; this is not
            // atomic fencing against an administrator replacing the engine.
            ManagedRuntime::reopen_with_runtime(runtime.binding().clone(), self.approved.clone())
                .await?;
            prefix.extend(["--host".to_owned(), runtime.binding().endpoint.clone()]);
        }
        if let Some(directory) = directory {
            directory.revalidate()?;
        }
        consume(&self.approved, prefix, command, duration, environment).await
    }
}

fn inject_podman_build_ignorefile(
    runtime: RuntimeKind,
    command: &mut PreparedCommand,
) -> Result<()> {
    if runtime == RuntimeKind::Podman && command.verb == "build" {
        let ignore = command
            .snapshot
            .build_ignore_override()
            .context("prepared build has no private Podman ignore file")?;
        ignore.revalidate()?;
        let path = utf8(ignore.path())?.to_owned();
        let context_position = command
            .arguments
            .len()
            .checked_sub(1)
            .context("prepared build has no context argument")?;
        command.arguments.splice(
            context_position..context_position,
            ["--ignorefile".to_owned(), path],
        );
    }
    Ok(())
}

/// Runs one prepared command to its end and says what was observed: the wait
/// status of the local CLI process, end of file on its two pipes and the
/// signals this process sent. Nothing here is a fact about an engine daemon.
async fn consume(
    runtime: &Runtime,
    prefix: Vec<String>,
    command: PreparedCommand,
    duration: Duration,
    environment: &[(&str, &str)],
) -> Result<Capture> {
    ensure!(
        prefix.is_empty() || prefix.len() == 2 && prefix[0] == "--host",
        "unsupported engine-global options"
    );
    let subject = format!("{} {}", runtime.kind, command.verb);
    // Copied before the command is handed over: the message names the
    // directory even when the end carries no path.
    let root = command.snapshot.root().to_owned();
    let mut owner = runtime.cli_env_retained(
        prefix,
        Box::new(command),
        duration,
        environment,
        interrupt::current(),
    );
    // The end is taken on the command's own task: the hold of this command
    // ends here and nowhere else. The interruption scope cannot drop this
    // future while it waits, because that hold counts until `join()` returns
    // an end that released the input; an observer dropped without its end
    // would keep the hold, and the scope would never see the command idle.
    let end = owner.join().await;
    concluded(runtime.kind, &subject, duration, &root, end)
}

/// What the operator is told about the end of `subject`, a prepared command
/// that was bounded by `duration` and whose copy lives at `root`.
fn concluded(
    kind: RuntimeKind,
    subject: &str,
    duration: Duration,
    root: &Path,
    end: InvocationEnd,
) -> Result<Capture> {
    match end {
        InvocationEnd::Refused(error) => Err(refused(subject, error)),
        InvocationEnd::Collected(collected) => match collected.stopped {
            Some(stop) => Err(anyhow!(stopped(subject, duration, stop, &collected))),
            None => match collected.input_changed {
                Some(changed) => Err(anyhow!(
                    "the prepared copy changed while {subject} was running ({changed:#}); the CLI \
                     process exited ({}) but its result is not bound to the reported digest",
                    collected.status
                )),
                None => InvocationEnd::Collected(collected).into_result(kind),
            },
        },
        InvocationEnd::Unknown(end) => Err(anyhow!(unknown(subject, &end, root))),
        InvocationEnd::Aborted { input_released } => {
            let kept = if input_released {
                String::new()
            } else {
                format!(". {}", retained(root))
            };
            Err(anyhow::Error::new(InvocationFailure::Unavailable)
                .context(format!("{subject} ended without a collected result{kept}")))
        }
    }
}

fn refused(subject: &str, error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<InvocationFailure>() {
        Some(InvocationFailure::Interrupted { signal }) => anyhow!(
            "interrupted by {}: {subject} was not started",
            signal_name(*signal)
        ),
        Some(InvocationFailure::InputChanged) => {
            let detail = error
                .chain()
                .skip(1)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(": ");
            anyhow!(
                "the prepared copy changed before {subject} started ({detail}); {subject} was not started"
            )
        }
        // The consumer's own reason stays the cause, with its type.
        _ => error.context(format!("{subject} was not started")),
    }
}

/// The signals this process sent to the command's process group.
fn signals_sent(subject: &str, stop: Stop) -> String {
    let group = format!("the process group of the CLI process for {subject}");
    match (stop.forwarded, stop.kill_sent) {
        (Some(signal), false) => format!("{} was forwarded to {group}", signal_name(signal)),
        (Some(signal), true) => format!(
            "{} was forwarded to {group}, then SIGKILL was sent",
            signal_name(signal)
        ),
        (None, true) => format!("SIGKILL was sent to {group}"),
        (None, false) => format!("no signal was accepted by {group}"),
    }
}

fn stopped(subject: &str, duration: Duration, stop: Stop, collected: &Collected) -> String {
    use std::os::unix::process::ExitStatusExt;
    let reason = match stop.reason {
        StopReason::Interrupted { signal } => format!("interrupted by {}", signal_name(signal)),
        StopReason::TimedOut => format!("{subject} exceeded its {} s bound", duration.as_secs()),
        StopReason::Cancelled => format!("{subject} was cancelled"),
        StopReason::OutputUnavailable => format!("the output of {subject} could not be read"),
        StopReason::WaitUnavailable => {
            format!("the wait status of {subject} was unavailable")
        }
    };
    let status = collected.status;
    let mut message = format!(
        "{reason}: {}; that process exited ({status}) and both of its output pipes reached end of file",
        signals_sent(subject, stop)
    );
    if stop.kill_sent && status.signal() != Some(libc::SIGKILL) {
        // The status shows that SIGKILL did not end the CLI process.
        message.push_str(
            ". The CLI process had already exited when SIGKILL was sent; the command was not \
             collected earlier, usually because a process it started kept an output pipe open",
        );
    }
    if let Some(changed) = &collected.input_changed {
        message.push_str(&format!(
            ". The prepared copy also changed while it was running ({changed:#})"
        ));
    }
    message
}

fn retained(root: &Path) -> String {
    format!(
        "The prepared copy is kept at {} and is not removed by ai-guard; see {RETAINED_GUIDE}",
        capture::shown(root)
    )
}

fn unknown(subject: &str, end: &UnknownEnd, root: &Path) -> String {
    let group = end.process_group;
    let sent = match (end.stop.forwarded, end.stop.kill_sent) {
        (Some(signal), true) => format!(
            "{} was forwarded to process group {group}, then SIGKILL was sent",
            signal_name(signal)
        ),
        (None, true) => format!("SIGKILL was sent to process group {group}"),
        (Some(signal), false) => format!(
            "{} was forwarded to process group {group}, SIGKILL was not sent",
            signal_name(signal)
        ),
        (None, false) => format!("no signal was sent to process group {group}"),
    };
    // An uncertain wait is announced at once, without the observation delay.
    let then = if end.stop.kill_sent && !end.wait_uncertain {
        format!(" and, {} s later, ", STOP_OBSERVATION.as_secs())
    } else {
        " and ".to_owned()
    };
    let mut observed = Vec::new();
    if end.wait_uncertain {
        observed.push("the wait status of the CLI process could not be obtained");
    } else if !end.leader_reaped {
        observed.push("the CLI process had not been reaped");
    }
    if end.stdout_open {
        observed.push("stdout was still open");
    }
    if end.stderr_open {
        observed.push("stderr was still open");
    }
    if observed.is_empty() {
        observed.push("its end could not be established");
    }
    format!(
        "{subject} termination is unknown: {sent}{then}{}. {}",
        observed.join(" and "),
        retained(end.retained_at.as_deref().unwrap_or(root))
    )
}

/// `Build`, `Up` and `Down` hold a prepared source across engine commands, and
/// `Release` holds one while it reads and hashes; all run inside one
/// interruption scope; a command already inside one does not open another.
pub(super) fn needs_interrupt_scope(command: &OperatorCommand) -> bool {
    matches!(
        command,
        OperatorCommand::Build { .. }
            | OperatorCommand::Up { .. }
            | OperatorCommand::Down { .. }
            | OperatorCommand::Release { .. }
    ) && !interrupt::active()
}

/// Runs `command` with SIGINT, SIGTERM and SIGHUP turned into one stop request
/// for its preparations and retained engine commands. Nothing of `command`
/// runs in a process that cannot wait for its children (SIGCHLD ignored or
/// SA_NOCLDWAIT): it is refused here.
pub(super) async fn interrupt_scope<F>(command: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    interrupt::scope_with(interrupt::OsSignals::install()?, command).await
}
