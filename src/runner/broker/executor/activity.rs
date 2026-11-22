//! Live provider activity for one supervised batch execution: the trusted
//! context of the plan, opened before the start control is emitted.
use super::{Execution, diagnostics::BrokerStep};
use crate::{
    control_fs::Directory,
    provider_activity::{ActivityClient, ActivityContext, session::LiveSession},
};
use anyhow::{Context, Result};

const ACTIVITY_DIRECTORY: &str = "provider-activity";

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
    /// Retain the exact private directory across fresh preparation and start.
    pub(super) async fn prepare_activity_directory(&self) -> Result<&Directory> {
        self.activity_directory
            .get_or_try_init(|| async {
                self.executor
                    .activity_root
                    .as_ref()
                    .context(BrokerStep::ActivitySpool)?
                    .child(ACTIVITY_DIRECTORY)
                    .context(BrokerStep::ActivitySpool)
            })
            .await
    }

    /// Open live activity before starting a supported provider client.
    pub(super) async fn open_activity(
        &self,
        draft: &super::Draft,
    ) -> Result<Option<(LiveSession, ActivityClient)>> {
        let Some(context) = activity_context(draft) else {
            return Ok(None);
        };
        let client = context.client;
        let root = self.prepare_activity_directory().await?;
        let session = LiveSession::open(
            root,
            context,
            self.client.redactor.clone(),
            self.client.clone(),
            self.executor.activity_limits,
            true,
        )
        .context(BrokerStep::ActivitySpool)?;
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
            (&self.executor.activity_root, activity_context(draft))
        {
            let directory = root.open_existing_child(ACTIVITY_DIRECTORY)?;
            LiveSession::replay_retained(
                &directory,
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
