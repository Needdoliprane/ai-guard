//! Live provider activity for one supervised batch execution: the trusted
//! context of the plan, opened before the start control is emitted.
use super::Execution;
use crate::provider_activity::{ActivityClient, ActivityContext, session::LiveSession};
use anyhow::{Context, Result};

/// Map the plan's provider option; other providers have no live channel.
pub(super) fn client(provider: &str) -> Option<ActivityClient> {
    match provider {
        "codex" => Some(ActivityClient::Codex),
        "claude" | "claude-code" => Some(ActivityClient::ClaudeCode),
        _ => None,
    }
}

impl Execution<'_> {
    /// Open (or, after an interruption, reopen) this session's live activity.
    /// `None` only when the plan has no supported provider client.
    pub(super) fn open_activity(
        &self,
        draft: &super::Draft,
    ) -> Result<Option<(LiveSession, ActivityClient)>> {
        let options = draft.run.options();
        let Some(client) = client(&options.provider) else {
            return Ok(None);
        };
        let root = self
            .executor
            .activity
            .as_ref()
            .context("provider activity directory is not configured")?;
        let request = &draft.run.plan.request;
        let worker = draft.worker().ok();
        let context = ActivityContext {
            session: draft.run.session(),
            session_id: request.session_id.clone(),
            project_id: request.project_id.clone(),
            sandbox_id: request.sandbox_id.clone(),
            agent_id: request.agent_id.clone(),
            correlation_id: request.correlation_id.clone(),
            trace_id: request.trace_id.clone(),
            worker_id: worker.map(|w| w.worker_id.clone()),
            attempt_id: worker.map(|w| w.attempt.attempt_id.clone()),
            client,
            interactive: options.interactive,
            client_version_label: None,
        };
        let session = LiveSession::open(
            root,
            context,
            self.client.redactor.clone(),
            self.client.clone(),
            self.executor.activity_limits,
            true,
        )?;
        Ok(Some((session, client)))
    }
}
