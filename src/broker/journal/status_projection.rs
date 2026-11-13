//! Bounded diagnostic data from one strict journal observation. This type has
//! no runner binding, checkpoint, history or conversion to execution authority.
use super::*;
use crate::{
    broker::checkpoint::SupervisionOperationV1,
    model::{Decision, Outcome},
};

#[derive(Clone)]
pub(crate) struct LastConfirmedStatus {
    entry: JournalEntryV1,
    intent_hash: Option<String>,
    last_decision: Option<Decision>,
    review_request_hash: Option<String>,
}
impl LastConfirmedStatus {
    pub(in crate::broker) fn from_status(operation: Uuid, status: &JournalStatusV1) -> Self {
        let mut projection = Self {
            entry: Journal::summary(operation, status),
            intent_hash: None,
            last_decision: None,
            review_request_hash: None,
        };
        if let JournalStatusV1::Accepted(job) = status {
            projection.fill_accepted(job);
        }
        projection
    }
    pub(in crate::broker) fn from_accepted(job: &AcceptedJobV1) -> Self {
        let mut projection = Self {
            entry: Journal::accepted_summary(job.operation_id(), job),
            intent_hash: None,
            last_decision: None,
            review_request_hash: None,
        };
        projection.fill_accepted(job);
        projection
    }
    fn fill_accepted(&mut self, job: &AcceptedJobV1) {
        self.intent_hash = Some(job.intent_hash().into());
        if let Some(checkpoint) = job.broker_checkpoint_v2() {
            self.last_decision =
                checkpoint
                    .supervision()
                    .operations()
                    .iter()
                    .rev()
                    .find_map(|operation| match operation {
                        SupervisionOperationV1::Decision {
                            response: Some(answer),
                            ..
                        } => Some(answer.decision().clone()),
                        _ => None,
                    });
            let current = job.checkpoint().broker_context();
            if !self.entry.cancel_requested && !current.managed && !current.request.human_approved {
                self.review_request_hash =
                    checkpoint
                        .supervision()
                        .operations()
                        .iter()
                        .find_map(|operation| match operation {
                            SupervisionOperationV1::Decision {
                                request,
                                response: Some(answer),
                            } if !request.human_approved
                                && answer.decision().outcome == Outcome::Review
                                && request.request_id == current.request.request_id =>
                            {
                                Some(crate::policy::request_hash(request))
                            }
                            _ => None,
                        });
            }
        }
    }
    pub(crate) fn entry(&self) -> &JournalEntryV1 {
        &self.entry
    }
    pub(crate) fn intent_hash(&self) -> Option<&str> {
        self.intent_hash.as_deref()
    }
    pub(crate) fn last_decision(&self) -> Option<&Decision> {
        self.last_decision.as_ref()
    }
    pub(crate) fn review_request_hash(&self) -> Option<&str> {
        self.review_request_hash.as_deref()
    }
    pub(in crate::broker) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.entry.head.as_ref().map_or(0, |head| head.hash().len()))
            .saturating_add(self.entry.last_observed_at.as_ref().map_or(0, String::len))
            .saturating_add(self.intent_hash.as_ref().map_or(0, String::len))
            .saturating_add(self.review_request_hash.as_ref().map_or(0, String::len))
            .saturating_add(self.last_decision.as_ref().map_or(0, |decision| {
                decision
                    .engine
                    .len()
                    .saturating_add(decision.rule.len())
                    .saturating_add(decision.reason.len())
            }))
    }
    pub(in crate::broker) fn update_head(&mut self, head: RevisionRefV1) -> JournalResult<()> {
        let current = self.entry.head.as_ref().ok_or(JournalError::InvalidInput)?;
        if head.revision() < current.revision() {
            return Ok(());
        }
        if head.revision() == current.revision() && &head != current {
            return Err(JournalError::Conflict);
        }
        self.entry.head = Some(head);
        Ok(())
    }
    pub(in crate::broker) fn update_lifecycle(
        &mut self,
        head: RevisionRefV1,
        next: &super::super::lifecycle::BrokerCheckpointV3,
    ) -> JournalResult<()> {
        if self
            .entry
            .head
            .as_ref()
            .is_some_and(|current| head.revision() < current.revision())
        {
            return Ok(());
        }
        self.update_head(head)?;
        self.entry.cancel_requested = false;
        self.entry.approval_submitted = false;
        self.entry.failure_count = 0;
        for command in next.commands() {
            self.apply_local(command.request());
        }
        self.entry.pending_rpc_count = Some(
            next.broker()
                .supervision()
                .operations()
                .iter()
                .filter(|op| !op.has_response())
                .count(),
        );
        let context = next.broker().runner().broker_context();
        self.entry.worker_state = context.worker.as_ref().map(|worker| worker.state);
        self.entry.quota_held = context.worker.as_ref().map(|worker| worker.quota_held);
        let observed = context
            .worker
            .as_ref()
            .and_then(|worker| worker.last_observation.as_ref());
        self.entry.runtime_status = observed.map(|observed| observed.status);
        self.entry.last_observed_at = observed.map(|observed| observed.observed_at.clone());
        self.last_decision = next
            .broker()
            .supervision()
            .operations()
            .iter()
            .rev()
            .find_map(|operation| match operation {
                SupervisionOperationV1::Decision {
                    response: Some(answer),
                    ..
                } => Some(answer.decision().clone()),
                _ => None,
            });
        self.review_request_hash =
            if self.entry.cancel_requested || context.managed || context.request.human_approved {
                None
            } else {
                next.broker().supervision().operations().iter().find_map(
                    |operation| match operation {
                        SupervisionOperationV1::Decision {
                            request,
                            response: Some(answer),
                        } if !request.human_approved
                            && answer.decision().outcome == Outcome::Review
                            && request.request_id == context.request.request_id =>
                        {
                            Some(crate::policy::request_hash(request))
                        }
                        _ => None,
                    },
                )
            };
        Ok(())
    }
    fn apply_local(&mut self, request: &super::super::lifecycle::LocalCommandRequestV1) {
        use super::super::lifecycle::LocalCommandRequestV1;
        match request {
            LocalCommandRequestV1::CancelRequested => {
                self.entry.cancel_requested = true;
                self.review_request_hash = None;
            }
            LocalCommandRequestV1::ApprovalSubmitted { .. } => self.entry.approval_submitted = true,
            LocalCommandRequestV1::FailureObserved { .. } => self.entry.failure_count += 1,
        }
    }
    pub(in crate::broker) fn update_local(
        &mut self,
        head: RevisionRefV1,
        request: &super::super::lifecycle::LocalCommandRequestV1,
    ) -> JournalResult<()> {
        let current = self.entry.head.as_ref().ok_or(JournalError::InvalidInput)?;
        if head.revision() <= current.revision() {
            return self.update_head(head);
        }
        if current.revision().checked_add(1) != Some(head.revision()) {
            return Err(JournalError::Conflict);
        }
        self.update_head(head)?;
        self.apply_local(request);
        Ok(())
    }
}
