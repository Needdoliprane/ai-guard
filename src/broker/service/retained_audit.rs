//! Audit-only recovery on the retained Stops actor. This path never opens a
//! business context, provider credential, runtime or a Full journal writer.
use super::*;
use crate::{
    broker::{cold_terminal::check_retained_terminal, writer::ReservedColdAudit},
    runner::ColdAuditBudget,
};

#[cfg(test)]
#[path = "cold_owner_tests.rs"]
mod tests;

impl Service {
    pub(super) async fn reconcile_retained_audit(&self) {
        // Inspect only capabilities already opened during recovery. In
        // particular, discovery must not initialize a fresh profile journal.
        if !self.profiles.values().any(|jobs| {
            jobs.writer
                .get()
                .and_then(ProfileWriterCapabilities::cold_audit)
                .is_some()
        }) {
            return;
        }
        if self.validate_control().await.is_err() {
            self.fenced.store(true, Ordering::SeqCst);
            return;
        }
        for profile in &self.registry.data.profiles {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let Ok(jobs) = self.profile_jobs(&profile.id) else {
                continue;
            };
            let Some(caps) = jobs.writer.get() else {
                continue;
            };
            let Some(cold) = caps.cold_audit() else {
                continue;
            };
            let Ok(capacity) = Arc::clone(&self.execution).try_acquire_owned() else {
                return;
            };
            // validate_control has revalidated these retained private bytes on
            // the bounded filesystem pool. No worker credential is consulted.
            let Ok(credential) = self.material.reconciler(profile) else {
                continue;
            };
            let stops = caps.stops();
            let mut after = None;
            'pages: loop {
                let page = match stops.recovery_inventory(after, 100).await {
                    Ok(page) => page,
                    Err(_) => {
                        self.fenced.store(true, Ordering::SeqCst);
                        break;
                    }
                };
                for entry in page.entries() {
                    if entry.incomplete() || self.stopping.load(Ordering::SeqCst) {
                        continue;
                    }
                    let operation = entry.operation_id();
                    // Preparation proves an admissible pending batch and owns
                    // both reserves. Historical/Busy entries consume neither a
                    // Job cache entry nor its bounded autonomous retry budget.
                    let Ok(reserved) = cold.prepare(operation).await else {
                        continue;
                    };
                    let Ok(job) = jobs.job(operation) else {
                        continue;
                    };
                    let Ok(running) = job.enter() else {
                        continue;
                    };
                    if !job.autonomous_attempt() {
                        continue;
                    }
                    RetainedAuditOwner {
                        registry: Arc::clone(&self.registry),
                        profile_id: profile.id.clone(),
                        credential,
                        reserved,
                        job,
                        _running: running,
                        _capacity: capacity,
                        stopping: Arc::clone(&self.stopping),
                        #[cfg(test)]
                        stop_after_first_receipt: false,
                    }
                    .start();
                    // At most one owner per retained actor at a time. Its
                    // completion reservation remains live through all HTTP.
                    break 'pages;
                }
                after = page.next_after();
                if after.is_none() {
                    break;
                }
            }
        }
    }
}

struct RetainedAuditOwner {
    registry: Arc<Registry>,
    profile_id: String,
    credential: String,
    reserved: ReservedColdAudit,
    job: Arc<Job>,
    _running: RunningJob,
    _capacity: OwnedSemaphorePermit,
    stopping: Arc<AtomicBool>,
    #[cfg(test)]
    stop_after_first_receipt: bool,
}

impl RetainedAuditOwner {
    fn start(self) {
        // Dropping an observer or the reconcile-loop task does not abort this
        // owner. The process/runtime lifetime is a separate, unqualified gate.
        tokio::spawn(async move {
            let job = Arc::clone(&self.job);
            job.result(self.publish().await.err());
        });
    }

    async fn publish(self) -> std::result::Result<(), BrokerErrorCodeV1> {
        let budget = ColdAuditBudget::start();
        // Bound identity by the original budget, not an outer timeout that
        // could drop an already received first POST receipt during the second.
        let verified = tokio::time::timeout_at(
            budget.deadline(),
            VerifiedProfile::authenticate(
                &self.registry,
                &self.profile_id,
                &self.credential,
                false,
            ),
        )
        .await
        .map_err(|_| BrokerErrorCodeV1::Unavailable)??;
        let prepared = self.reserved.prepared();
        if !same_scope(&verified.scope, prepared.scope()) {
            return Err(BrokerErrorCodeV1::Forbidden);
        }
        let facts = prepared
            .retained_terminal_facts()
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        let retained_worker = facts
            .checkpoint
            .lifecycle()
            .broker()
            .runner()
            .broker_context()
            .worker
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        let worker = verified
            .client
            .cold_worker(&retained_worker.worker_id, &budget)
            .await
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        check_retained_terminal(facts, &worker).map_err(|_| BrokerErrorCodeV1::Conflict)?;
        let mut receipts = Vec::with_capacity(2);
        let mut incomplete = false;
        for entry in prepared.pending_events() {
            if self.stopping.load(Ordering::SeqCst) {
                incomplete = true;
                break;
            }
            match verified
                .client
                .post_cold_audit_event(entry.event(), &budget)
                .await
            {
                Ok(receipt) => {
                    receipts.push(receipt);
                    #[cfg(test)]
                    if self.stop_after_first_receipt && receipts.len() == 1 {
                        self.stopping.store(true, Ordering::SeqCst);
                    }
                }
                Err(_) => {
                    incomplete = true;
                    break;
                }
            }
        }
        if receipts.is_empty() {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        // No await, new capacity or stopping guard between the last HTTP and
        // this single-use enqueue. A failed second POST still publishes the
        // exact first receipt, without constructing a replacement event.
        let waiter = self
            .reserved
            .submit(receipts)
            // NotQueued proves no publication ran; the already durable event
            // IDs remain pending for explicit recovery. An actor/process loss
            // is not a confirmed local ACK or an in-memory receipt guarantee.
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        waiter
            .confirmed()
            .await
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        if incomplete {
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        } else {
            Ok(())
        }
    }
}

fn same_scope(left: &JournalScopeV1, right: &JournalScopeV1) -> bool {
    left.installation_id() == right.installation_id()
        && left.tenant_id() == right.tenant_id()
        && left.gateway_origin() == right.gateway_origin()
        && left.control_namespace_uuid() == right.control_namespace_uuid()
        && left.profile_id() == right.profile_id()
}
