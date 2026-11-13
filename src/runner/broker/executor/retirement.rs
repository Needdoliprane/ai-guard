//! Live abandonment of an explicitly selected, still unused original Arm.
use super::*;
use crate::broker::{
    service::preclaim::{AbandonBeforeClaimV1, PreclaimSignalReservationV1, RetainedRetirementV1},
    writer::{BeginUnusedRetirement, RetirementContinuation, RetirementFailure},
};

const RETIREMENT_QUEUE_BACKOFF: Duration = Duration::from_millis(5);

/// NotQueued still owns the original value. Unknown has crossed the queue's
/// ownership boundary and must never be retried as if no work was submitted.
enum RetryFailure<I> {
    Owned { input: I, error: anyhow::Error },
    Unknown(JournalError),
}

async fn retry_retained<I, O, Submit, Submitted, Budget, Recover, Resume, Resumed>(
    input: I,
    mut submit: Submit,
    mut remaining: Budget,
    mut recover: Recover,
    mut resume: Resume,
) -> std::result::Result<O, RetryFailure<I>>
where
    Submit: FnMut(I) -> Submitted,
    Submitted: std::future::Future<Output = std::result::Result<O, RetirementFailure<I>>>,
    Budget: FnMut() -> Result<Duration>,
    Recover: FnMut() -> std::result::Result<Option<O>, RetirementFailure<I>>,
    Resume: FnMut(RetirementContinuation<I>) -> Resumed,
    Resumed: std::future::Future<Output = std::result::Result<O, RetirementFailure<I>>>,
{
    enum Attempt<I> {
        Input(I),
        Retained(RetirementContinuation<I>),
    }
    let mut attempt = Attempt::Input(input);
    loop {
        let available = remaining().and_then(|duration| {
            ensure!(!duration.is_zero(), "original Arm deadline has elapsed");
            Ok(duration)
        });
        if let Err(error) = available {
            return Err(match attempt {
                Attempt::Input(input) => RetryFailure::Owned { input, error },
                Attempt::Retained(_) => RetryFailure::Unknown(JournalError::Busy),
            });
        }
        let outcome = match attempt {
            Attempt::Input(input) => submit(input).await,
            Attempt::Retained(continuation) => resume(continuation).await,
        };
        let submitted = match outcome {
            Err(RetirementFailure::Unknown { error }) => loop {
                match recover() {
                    Ok(Some(output)) => return Ok(output),
                    Err(RetirementFailure::Unknown { error }) => {
                        return Err(RetryFailure::Unknown(error));
                    }
                    Err(returned) => break Err(returned),
                    Ok(None) => {
                        let delay = match remaining() {
                            Ok(duration) if !duration.is_zero() => {
                                duration.min(RETIREMENT_QUEUE_BACKOFF)
                            }
                            _ => return Err(RetryFailure::Unknown(error)),
                        };
                        tokio::time::sleep(delay).await;
                    }
                }
            },
            outcome => outcome,
        };
        match submitted {
            Ok(output) => return Ok(output),
            Err(RetirementFailure::Unknown { error }) => return Err(RetryFailure::Unknown(error)),
            Err(RetirementFailure::NotQueued { error, input }) => {
                // The same SyncSender cannot recover after Disconnected. Keep
                // this live input with the owner instead of spinning on Io.
                if error != JournalError::Busy {
                    return Err(RetryFailure::Owned {
                        input,
                        error: error.into(),
                    });
                }
                let delay = match remaining() {
                    Ok(duration) if !duration.is_zero() => duration.min(RETIREMENT_QUEUE_BACKOFF),
                    Ok(_) => {
                        return Err(RetryFailure::Owned {
                            input,
                            error: anyhow!("original Arm deadline has elapsed"),
                        });
                    }
                    Err(error) => return Err(RetryFailure::Owned { input, error }),
                };
                attempt = Attempt::Input(input);
                tokio::time::sleep(delay).await;
            }
            Err(RetirementFailure::Refused {
                error,
                continuation,
            }) => {
                // The actual input and capacity stay in the original writer
                // entry during this await. Dropping this ticket loses no cap.
                if error != JournalError::Busy {
                    return Err(RetryFailure::Unknown(error));
                }
                let delay = match remaining() {
                    Ok(duration) if !duration.is_zero() => duration.min(RETIREMENT_QUEUE_BACKOFF),
                    _ => return Err(RetryFailure::Unknown(error)),
                };
                attempt = Attempt::Retained(continuation);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

impl Execution<'_> {
    /// The generic outbox also contains worker-declared events. Only a retained
    /// retirement identity or this process's actual signal reservation selects
    /// the control route; a matching kind or producer label grants no budget.
    pub(super) fn retirement_control_delivery(
        &self,
        job: &AcceptedJobV1,
        event: &crate::broker::outbox::OutboxEventV1,
    ) -> Result<bool> {
        let kind = event.event().kind.as_str();
        if !matches!(
            kind,
            "broker.abandon-before-claim.requested"
                | "broker.retirement.pending"
                | "broker.retirement.final"
        ) {
            return Ok(false);
        }
        let checkpoint = job
            .broker_checkpoint_v5()
            .context("retirement audit requires its retained Full3 checkpoint")?;
        let retained = checkpoint.retirements().iter().any(|entry| {
            let pending = entry.request();
            match kind {
                "broker.abandon-before-claim.requested" => {
                    *pending.identity().retirement_id() == event.event_id()
                }
                "broker.retirement.pending" => {
                    pending.intention_event_id() == event.event_id()
                        && pending.intent_input_hash().as_str() == event.input_hash()
                }
                "broker.retirement.final" => entry.finalized().is_some_and(|finalized| {
                    pending.final_event_id() == event.event_id()
                        && finalized.final_input_hash().as_str() == event.input_hash()
                }),
                _ => false,
            }
        });
        if retained {
            return Ok(true);
        }
        ensure!(
            kind == "broker.abandon-before-claim.requested",
            "retirement audit event has no retained identity"
        );
        let value = event.event();
        let request: AbandonBeforeClaimV1 = serde_json::from_value(value.data.clone())?;
        let view = self.context.preclaim().arm_view(request.arm_id);
        ensure!(
            request.operation_id == self.operation
                && request.operation_id == job.operation_id()
                && request.command_id == event.event_id()
                && view.command_id == Some(request.command_id)
                && view.expected_epoch == Some(request.expected_epoch)
                && view.generation == Some(request.generation)
                && checkpoint.bindings().iter().any(|binding| {
                    binding.arm_id() == request.arm_id
                        && binding.original_epoch() == request.expected_epoch
                })
                && value.source == "ai-guard-broker"
                && value.provenance == "observed"
                && value.coverage == "native"
                && value.agent_id == job.intent().request_data().agent_id
                && value.parent_agent_id.is_none()
                && value.command.is_none()
                && value.decision.is_none()
                && value.result.is_none(),
            "preclaim audit has no retained signal reservation"
        );
        Ok(true)
    }

    fn retirement_arm_remaining(&self, arm: &ArmV1) -> Result<Duration> {
        self.generation_current()?;
        let sample = self.executor.clock.sample()?;
        ensure!(
            sample.method == arm.clock.method && sample.boot_id == arm.clock.boot_id,
            "original Arm clock identity is unavailable"
        );
        let now = sample.continuous_ns()?;
        ensure!(
            now >= arm.clock.continuous_ns()?,
            "original Arm clock moved backwards"
        );
        let deadline = arm.deadline_ns_decimal.parse::<u128>()?;
        let remaining = deadline
            .checked_sub(now)
            .filter(|duration| *duration > 0)
            .context("original Arm deadline has elapsed")?;
        let nanos =
            u64::try_from(remaining).context("remaining Arm duration is outside runtime bounds")?;
        Ok(Duration::from_nanos(nanos))
    }

    fn retirement_retry_remaining(
        &self,
        arm: &ArmV1,
        original_deadline: tokio::time::Instant,
    ) -> Result<Duration> {
        let native = self.retirement_arm_remaining(arm)?;
        let local = original_deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|duration| !duration.is_zero())
            .context("original Arm deadline has elapsed")?;
        Ok(native.min(local))
    }

    fn retain_retirement_error(
        &self,
        request: &AbandonBeforeClaimV1,
        continuation: RetainedRetirementV1,
        error: anyhow::Error,
    ) -> anyhow::Error {
        self.context.preclaim().retain_retirement(
            request.arm_id,
            request.expected_epoch,
            request.generation,
            continuation,
        );
        error
            .context("retirement is blocked; its live continuation remains with this process owner")
    }

    pub(super) async fn retire_unused_before_claim(
        &self,
        slot: EnforcedSlot,
        arm: &ArmV1,
        signal: PreclaimSignalReservationV1,
    ) -> Result<PrearmedStartDirective> {
        let full = self
            .executor
            .enforcer
            .full3()
            .context("retirement requires explicit Full3 authority")?;
        let remaining = self.retirement_arm_remaining(arm)?;
        let original_deadline = tokio::time::Instant::now()
            .checked_add(remaining)
            .context("original Arm duration is outside runtime bounds")?;
        // This local bound is derived once from the existing Arm. Retries also
        // check its native clock/boot and cancellation; no retry grants grace.
        let request = tokio::time::timeout(remaining, signal.wait_audit_confirmed())
            .await
            .context("abandon signal audit did not complete before the original deadline")??;
        ensure!(
            request.operation_id == self.operation
                && request.arm_id.to_string() == arm.operation_id
                && request.expected_epoch == full.epoch()
                && request.generation == self.generation,
            "abandon signal no longer matches this original Arm"
        );
        // Read and validate constructor data before consuming the native
        // revocation witness. A failed read leaves the original STOP owner.
        let (job, _) = self.load().await?;
        ensure!(!self.operation.is_nil(), "invalid retirement operation");
        let expected =
            crate::broker::journal::RevisionRefV1::new(job.head().revision(), job.head().hash())?;
        self.retirement_retry_remaining(arm, original_deadline)?;
        let unused = slot.revoke_unused(request.command_id).await?;
        // Begin repeats only the exact UUID/hash checks completed above.
        let begin = BeginUnusedRetirement::new(self.operation, expected, unused)?;
        let pending = match retry_retained(
            begin,
            |input| self.executor.writer.begin_unused_retirement(input),
            || self.retirement_retry_remaining(arm, original_deadline),
            || {
                self.executor.writer.recover_begin_unused_retirement(
                    request.operation_id,
                    request.arm_id,
                    request.command_id,
                )
            },
            |continuation| {
                self.executor
                    .writer
                    .retry_begin_unused_retirement(continuation)
            },
        )
        .await
        {
            Ok(pending) => pending,
            Err(RetryFailure::Owned { input, error }) => {
                return Err(self.retain_retirement_error(
                    &request,
                    RetainedRetirementV1::Begin { _owner: input },
                    error,
                ));
            }
            Err(RetryFailure::Unknown(error)) => {
                return Err(anyhow::Error::new(error).context(
                    "retirement Begin outcome is unknown; inspect its fixed durable identities",
                ));
            }
        };
        // Borrow the same persisted event throughout audit delivery. Any
        // failure here still leaves this function owning the live Pending.
        let audit_result: Result<()> = async {
            let event_id = pending.intention_event_id();
            let event = serde_json::to_value(pending.intention_event())?;
            let remaining = self.retirement_retry_remaining(arm, original_deadline)?;
            let audit_seq = tokio::time::timeout(remaining, self.client.post_control_event(event))
                .await
                .context("retirement intention audit exceeded the original Arm deadline")??;
            let remaining = self.retirement_retry_remaining(arm, original_deadline)?;
            tokio::time::timeout(
                remaining,
                self.executor
                    .writer
                    .acknowledge_event(self.operation, event_id, audit_seq),
            )
            .await
            .context("retirement intention ACK is unknown at the original Arm deadline")??;
            Ok(())
        }
        .await;
        if let Err(error) = audit_result {
            return Err(self.retain_retirement_error(
                &request,
                RetainedRetirementV1::Pending { _owner: pending },
                error,
            ));
        }
        let authorized = match retry_retained(
            pending,
            |input| self.executor.writer.authorize_unused_retirement(input),
            || self.retirement_retry_remaining(arm, original_deadline),
            || {
                self.executor.writer.recover_authorized_retirement(
                    request.operation_id,
                    request.arm_id,
                    request.command_id,
                )
            },
            |continuation| {
                self.executor
                    .writer
                    .retry_authorized_retirement(continuation)
            },
        )
        .await
        {
            Ok(authorized) => authorized,
            Err(RetryFailure::Owned { input, error }) => {
                return Err(self.retain_retirement_error(
                    &request,
                    RetainedRetirementV1::Pending { _owner: input },
                    error,
                ));
            }
            Err(RetryFailure::Unknown(error)) => return Err(anyhow::Error::new(error).context(
                "retirement authorization outcome is unknown; no fresh owner may be reconstructed",
            )),
        };
        let ready: Result<()> = (|| {
            self.retirement_retry_remaining(arm, original_deadline)?;
            self.context.preclaim().mark_retiring(
                request.arm_id,
                request.expected_epoch,
                self.generation,
            )?;
            Ok(())
        })();
        if let Err(error) = ready {
            return Err(self.retain_retirement_error(
                &request,
                RetainedRetirementV1::Authorized {
                    _owner: Box::new(authorized),
                },
                error,
            ));
        }
        // Install this actual writer-produced authorization synchronously,
        // before any transport future can suspend or be dropped by its caller.
        let owner = match self
            .context
            .preclaim()
            .retain_authorized_submission(&request, authorized)
        {
            Ok(owner) => owner,
            Err((error, authorized)) => {
                return Err(self.retain_retirement_error(
                    &request,
                    RetainedRetirementV1::Authorized {
                        _owner: Box::new(authorized),
                    },
                    anyhow::Error::new(error),
                ));
            }
        };
        let verified = full
            .retire_unused_retained(&owner, || {
                self.context
                    .cancel_generation()
                    .with_current_retirement(self.generation, || {
                        self.retirement_retry_remaining(arm, original_deadline)
                            .map_err(|_| crate::broker::enforcer::EnforcerError::NotReady)?;
                        self.context
                            .preclaim()
                            .mark_retirement_submitted(&request, &owner)
                    })
            })
            .await?;
        let guardian_receipt_hash = verified.receipt().digest()?;
        let stops = self.executor.writer.stops();
        let final_receipt = match retry_retained(
            verified,
            |input| stops.finalize_retirement(input),
            || self.retirement_retry_remaining(arm, original_deadline),
            || {
                stops.recover_final_retirement(
                    request.operation_id,
                    request.arm_id,
                    request.command_id,
                )
            },
            |continuation| stops.retry_final_retirement(continuation),
        )
        .await
        {
            Ok(receipt) => receipt,
            Err(RetryFailure::Owned { input, error }) => {
                if let Err(input) = owner.retain_verified(input) {
                    return Err(self.retain_retirement_error(
                        &request,
                        RetainedRetirementV1::Verified {
                            _owner: Box::new(input),
                        },
                        error,
                    ));
                }
                return Err(error.context(
                    "retirement Final was not queued; its actual verified input remains retained",
                ));
            }
            Err(RetryFailure::Unknown(error)) => {
                return Err(anyhow::Error::new(error)
                    .context("retirement Final outcome is retained by its original writer owner"));
            }
        };
        // Final consumes the live wrapper. An authenticated, exact Q replay
        // supplies another wrapper for local removal, strictly after F.
        let mut replay = full
            .query_retirement_retained(&owner)
            .await?
            .context("guardian final receipt is unavailable")?;
        ensure!(
            replay.receipt().digest()? == guardian_receipt_hash,
            "guardian final receipt changed"
        );
        let snapshot = stops
            .recovery_status(self.operation)
            .await?
            .context("confirmed retirement metadata is unavailable after Final")?;
        full.retire_retained_local_after_final(&owner, &mut replay, &snapshot)?;
        self.context
            .preclaim()
            .complete_retirement_submission(&request, &owner, &replay)?;
        // F and its exact final event are already durable. Use only the narrow
        // metadata port here; the autonomous service pump owns later retries.
        let delivery: Result<()> = async {
            let stops = self.executor.writer.stops();
            let mut acknowledgements = Vec::new();
            for entry in stops.pending_retirement_events(self.operation).await? {
                let seq = self
                    .client
                    .post_control_event(serde_json::to_value(entry.event())?)
                    .await?;
                acknowledgements.push((entry.event_id(), entry.input_hash().to_owned(), seq));
            }
            if !acknowledgements.is_empty() {
                stops
                    .acknowledge_retirement_events(self.operation, acknowledgements)
                    .await?;
            }
            Ok(())
        }
        .await;
        if delivery.is_err() {
            // Closed warning only: no error text, credential, or invented ACK.
            eprintln!(
                "ai-guard: retirement Final is durable; final audit delivery remains pending"
            );
        }
        let retirement = crate::runtime::AbandonedBeforeClaim::new(
            self.operation,
            request.arm_id,
            final_receipt.head().clone(),
        )?;
        Ok(PrearmedStartDirective::AbandonedBeforeClaim(retirement))
    }
}

#[cfg(test)]
async fn retry_unqueued<I, O, Submit, Submitted, Budget>(
    input: I,
    submit: Submit,
    remaining: Budget,
) -> std::result::Result<O, RetryFailure<I>>
where
    Submit: FnMut(I) -> Submitted,
    Submitted: std::future::Future<Output = std::result::Result<O, RetirementFailure<I>>>,
    Budget: FnMut() -> Result<Duration>,
{
    retry_retained(
        input,
        submit,
        remaining,
        || {
            Err(RetirementFailure::Unknown {
                error: JournalError::Io,
            })
        },
        |_| {
            std::future::ready(Err(RetirementFailure::Unknown {
                error: JournalError::Io,
            }))
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Pure ownership marker, not a fabricated Unused/Pending/Authorized proof.
    struct OwnedInput {
        identity: Uuid,
        allocation: Box<u8>,
    }

    #[tokio::test]
    async fn not_queued_busy_retries_exact_owner_and_identity() {
        let identity = Uuid::new_v4();
        let allocation = Box::new(7);
        let pointer = std::ptr::from_ref(allocation.as_ref()) as usize;
        let attempts = AtomicUsize::new(0);
        let output = retry_unqueued(
            OwnedInput {
                identity,
                allocation,
            },
            |input| {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(input.identity, identity);
                    assert_eq!(
                        std::ptr::from_ref(input.allocation.as_ref()) as usize,
                        pointer
                    );
                    if count < 2 {
                        Err(RetirementFailure::NotQueued {
                            error: JournalError::Busy,
                            input,
                        })
                    } else {
                        Ok(input)
                    }
                }
            },
            || Ok(Duration::from_secs(1)),
        )
        .await;
        let output = match output {
            Ok(value) => value,
            Err(_) => panic!("eventually available queue"),
        };
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(output.identity, identity);
        assert_eq!(
            std::ptr::from_ref(output.allocation.as_ref()) as usize,
            pointer
        );
    }

    #[tokio::test]
    async fn unknown_is_never_retried_or_returned_as_owned_input() {
        let attempts = AtomicUsize::new(0);
        let outcome: std::result::Result<(), RetryFailure<OwnedInput>> = retry_unqueued(
            OwnedInput {
                identity: Uuid::new_v4(),
                allocation: Box::new(7),
            },
            |_input| {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(RetirementFailure::Unknown {
                        error: JournalError::Io,
                    })
                }
            },
            || Ok(Duration::from_secs(1)),
        )
        .await;
        assert!(matches!(
            outcome,
            Err(RetryFailure::Unknown(JournalError::Io))
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_or_deadline_after_busy_returns_same_owner_for_retention() {
        let checks = AtomicUsize::new(0);
        let attempts = AtomicUsize::new(0);
        let input = OwnedInput {
            identity: Uuid::new_v4(),
            allocation: Box::new(7),
        };
        let identity = input.identity;
        let pointer = std::ptr::from_ref(input.allocation.as_ref()) as usize;
        let result: std::result::Result<(), RetryFailure<OwnedInput>> = retry_unqueued(
            input,
            |input| {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(RetirementFailure::NotQueued {
                        error: JournalError::Busy,
                        input,
                    })
                }
            },
            || {
                if checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(Duration::from_secs(1))
                } else {
                    Err(anyhow!("original execution budget stopped"))
                }
            },
        )
        .await;
        let Err(RetryFailure::Owned { input, .. }) = result else {
            panic!("unsubmitted owner must be retained");
        };
        assert_eq!(input.identity, identity);
        assert_eq!(
            std::ptr::from_ref(input.allocation.as_ref()) as usize,
            pointer
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let expired_attempts = AtomicUsize::new(0);
        let expired: std::result::Result<(), RetryFailure<OwnedInput>> = retry_unqueued(
            input,
            |input| {
                expired_attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(RetirementFailure::NotQueued {
                        error: JournalError::Busy,
                        input,
                    })
                }
            },
            || Ok(Duration::ZERO),
        )
        .await;
        let Err(RetryFailure::Owned { input, .. }) = expired else {
            panic!("elapsed Arm retains its owner without submission");
        };
        assert_eq!(input.identity, identity);
        assert_eq!(expired_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disconnected_queue_returns_input_without_spinning_or_reissuing() {
        let attempts = AtomicUsize::new(0);
        let input = OwnedInput {
            identity: Uuid::new_v4(),
            allocation: Box::new(7),
        };
        let identity = input.identity;
        let result: std::result::Result<(), RetryFailure<OwnedInput>> = retry_unqueued(
            input,
            |input| {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(RetirementFailure::NotQueued {
                        error: JournalError::Io,
                        input,
                    })
                }
            },
            || Ok(Duration::from_secs(1)),
        )
        .await;
        let Err(RetryFailure::Owned { input, .. }) = result else {
            panic!("disconnected queue must return live input");
        };
        assert_eq!(input.identity, identity);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
