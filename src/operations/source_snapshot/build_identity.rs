//! Common descriptive identity of one retained operator build.
//!
//! The ID is the existing digest of the captured public distribution, not an
//! image digest, a build attestation or a compatibility assertion. Provider
//! requests, resolved bases and the target platform retain their own facts.
use super::{PreparedCommand, PreparedSource, capture::Scope, interrupt, release_metadata};
use crate::runtime::retained_cli::{Interruption, InvocationFailure};
use anyhow::{Context, Result, bail, ensure};
use std::sync::Arc;

/// The two image commands share these values, read once from the sealed copy.
/// Keeping the source also keeps its original pre/post-spawn verification.
pub(in crate::operations) struct RustBuildPlan {
    source: PreparedSource,
    version: String,
    source_id: String,
    revision: Option<String>,
    codex_version: Option<String>,
    claude_code_version: Option<String>,
}

impl PreparedSource {
    /// Capture verification and declaration parsing can read the entire bounded
    /// copy. Keep them off the task which receives operator signals, retaining
    /// its hold until the blocking worker has actually returned.
    pub(in crate::operations) async fn prepare_build_plan(
        &self,
        revision: Option<&str>,
        codex_version: Option<&str>,
        claude_code_version: Option<&str>,
    ) -> Result<(RustBuildPlan, [PreparedCommand; 2])> {
        let interruption = interrupt::current();
        let hold = interruption.as_ref().map(Interruption::hold);
        let stop = interruption.clone();
        let source = PreparedSource {
            snapshot: Arc::clone(&self.snapshot),
        };
        let revision = revision.map(str::to_owned);
        let codex_version = codex_version.map(str::to_owned);
        let claude_code_version = claude_code_version.map(str::to_owned);
        let result = tokio::task::spawn_blocking(move || {
            stopped(stop.as_ref())?;
            let plan = source.build_plan(
                revision.as_deref(),
                codex_version.as_deref(),
                claude_code_version.as_deref(),
            )?;
            stopped(stop.as_ref())?;
            let commands = plan.commands()?;
            stopped(stop.as_ref())?;
            Ok((plan, commands))
        })
        .await;
        let stop_result = stopped(interruption.as_ref());
        drop(hold);
        stop_result?;
        match result {
            Ok(result) => result,
            Err(_) => bail!(
                "the build plan ended without a result; no engine command was started"
            ),
        }
    }

    pub(in crate::operations) fn build_plan(
        &self,
        revision: Option<&str>,
        codex_version: Option<&str>,
        claude_code_version: Option<&str>,
    ) -> Result<RustBuildPlan> {
        ensure!(
            self.snapshot.scope() == Scope::Build,
            "this prepared source holds no build inputs"
        );
        if let Some(revision) = revision {
            ensure!(
                matches!(revision.len(), 40 | 64)
                    && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "--build-revision must be exactly 40 or 64 hexadecimal characters"
            );
        }
        for version in [codex_version, claude_code_version].into_iter().flatten() {
            crate::operations::validate_provider_version(version)?;
        }
        let version = release_metadata::aligned_product_version(&self.snapshot)?;
        // This accessor is the capture's recorded identity. Never hash a live
        // checkout, generate an independent ID or write metadata into the copy.
        let source_id = self.snapshot.digest().to_owned();
        ensure!(
            source_id.len() == 64
                && source_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "the prepared source has no valid SHA-256 identity"
        );
        Ok(RustBuildPlan {
            source: PreparedSource {
                snapshot: Arc::clone(&self.snapshot),
            },
            version,
            source_id,
            revision: revision.map(str::to_owned),
            codex_version: codex_version.map(str::to_owned),
            claude_code_version: claude_code_version.map(str::to_owned),
        })
    }
}

fn stopped(interruption: Option<&Interruption>) -> Result<()> {
    if let Some(signal) = interruption.and_then(Interruption::requested) {
        return Err(InvocationFailure::Interrupted { signal }.into());
    }
    Ok(())
}

impl RustBuildPlan {
    /// Both commands remain bound to this one captured distribution. The
    /// ordinary option grammar cannot accept externally supplied identities.
    pub(in crate::operations) fn commands(&self) -> Result<[PreparedCommand; 2]> {
        self.source.snapshot.verify()?;
        Ok([
            self.command(
                "images/control.Dockerfile",
                "agentsandbox-control:local",
                false,
            )?,
            self.command(
                "images/worker.Dockerfile",
                "agentsandbox-worker:local",
                true,
            )?,
        ])
    }

    fn command(&self, recipe: &str, tag: &str, worker: bool) -> Result<PreparedCommand> {
        let mut options = vec!["build".to_owned(), "--tag".to_owned(), tag.to_owned()];
        if worker {
            for (name, version) in [
                ("CODEX_VERSION", self.codex_version.as_deref()),
                ("CLAUDE_CODE_VERSION", self.claude_code_version.as_deref()),
            ] {
                if let Some(version) = version {
                    options.extend(["--build-arg".to_owned(), format!("{name}={version}")]);
                }
            }
        }
        let mut command = self.source.build_command(recipe, &options)?;
        let context = command
            .arguments
            .pop()
            .context("the prepared build command has no context")?;
        for (name, value) in [
            ("AI_GUARD_VERSION", self.version.as_str()),
            ("AI_GUARD_BUILD_ID", self.source_id.as_str()),
            (
                "AI_GUARD_BUILD_REVISION",
                self.revision.as_deref().unwrap_or(""),
            ),
        ] {
            command
                .arguments
                .extend(["--build-arg".to_owned(), format!("{name}={value}")]);
        }
        command.arguments.push(context);
        Ok(command)
    }
}
