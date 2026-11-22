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

fn activity_context(draft: &super::Draft) -> Option<ActivityContext> {
    let options = draft.run.options();
    let client = client(&options.provider)?;
    let request = &draft.run.plan.request;
    let worker = draft.worker().ok();
    Some(ActivityContext {
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
    })
}

impl Execution<'_> {
    /// Open live activity before starting a supported provider client.
    pub(super) fn open_activity(
        &self,
        draft: &super::Draft,
    ) -> Result<Option<(LiveSession, ActivityClient)>> {
        let Some(context) = activity_context(draft) else {
            return Ok(None);
        };
        let client = context.client;
        let root = self
            .executor
            .activity
            .as_ref()
            .context("provider activity directory is not configured")?;
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

    /// Finish or replay activity after the terminal result and scan.
    pub(super) async fn finish_terminal_activity(
        &self,
        draft: &super::Draft,
        activity: Option<LiveSession>,
    ) -> Result<()> {
        if let Some(session) = activity {
            session.finish().await?;
        } else if let (Some(root), Some(context)) =
            (&self.executor.activity, activity_context(draft))
        {
            LiveSession::replay_retained(
                root,
                context,
                self.client.redactor.clone(),
                &self.client,
                self.executor.activity_limits,
            )
            .await?;
        }
        Ok(())
    }
}
