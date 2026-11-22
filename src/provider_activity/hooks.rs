//! Broker side of the worker hook channel: spool each hook payload, then
//! acknowledge it to the worker helper. A gated hook (PreToolUse,
//! UserPromptSubmit) receives `recorded` only when its complete payload
//! produced at least one durable event of the same hook type.
use super::collector::{ActivityChannel, ActivityCollector, Admission};
use crate::activity_channel::wire::EventV1;
use crate::adapters::RecordDisposition;

/// One frame received from the worker activity helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookFrame {
    Hook {
        seq: u64,
        event: EventV1,
        payload: String,
        truncated: bool,
    },
    /// Losses the helper counted inside the worker: worker declarations.
    Loss {
        seq: u64,
        count: u64,
        reason: String,
    },
}

#[derive(serde::Deserialize)]
struct HookPayloadKind {
    hook_event_name: EventV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAck {
    Recorded,
    Refused,
}

/// The transport seam: the runtime exec channel in production, an in-memory
/// transport in tests. `next` returns `None` once the helper stream ended.
pub trait HookTransport: Send {
    fn next(&mut self) -> impl Future<Output = anyhow::Result<Option<HookFrame>>> + Send;
    fn ack(&mut self, seq: u64, ack: HookAck) -> impl Future<Output = anyhow::Result<()>> + Send;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookPumpOutcome {
    pub hooks: u64,
    pub recorded: u64,
    pub refused: u64,
    /// Gated hooks refused because admission yielded no matching, complete record.
    pub unrecorded_gates: u64,
    pub declared_losses: u64,
    /// The helper skipped or repeated a sequence number: observed loss.
    pub sequence_gaps: u64,
    pub transport_error: bool,
}

/// Pump until the helper stream ends or fails. Helper sequence numbers are
/// worker-declared; a gap or repeat is reported as an observed anomaly.
pub async fn pump_hooks<T: HookTransport>(
    collector: ActivityCollector,
    mut transport: T,
) -> HookPumpOutcome {
    let mut outcome = HookPumpOutcome::default();
    let mut expected = 1_u64;
    let _ = collector.channel_state(ActivityChannel::Hook, "attached", None);
    loop {
        let frame = match transport.next().await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                outcome.transport_error = true;
                break;
            }
        };
        let seq = match &frame {
            HookFrame::Hook { seq, .. } | HookFrame::Loss { seq, .. } => *seq,
        };
        if seq != expected {
            outcome.sequence_gaps += 1;
            let _ = collector.channel_state(
                ActivityChannel::Hook,
                "sequence_gap",
                Some(&format!("expected {expected}, received {seq}")),
            );
        }
        expected = seq.saturating_add(1);
        match frame {
            HookFrame::Hook {
                seq,
                event,
                payload,
                truncated,
            } => {
                outcome.hooks += 1;
                let collector_ref = collector.clone();
                let (admission, recorded) = tokio::task::spawn_blocking(move || {
                    let (admission, disposition) = collector_ref.hook_record(&payload, truncated);
                    let recorded = admission == Admission::Recorded
                        && (!event.is_gate()
                            || (disposition == RecordDisposition::Mapped
                                && !truncated
                                && serde_json::from_str::<HookPayloadKind>(&payload)
                                    .is_ok_and(|kind| kind.hook_event_name == event)));
                    (admission, recorded)
                })
                .await
                .unwrap_or((Admission::Unavailable, false));
                if admission == Admission::Recorded && !recorded {
                    outcome.unrecorded_gates += 1;
                }
                let ack = if recorded {
                    outcome.recorded += 1;
                    HookAck::Recorded
                } else {
                    outcome.refused += 1;
                    HookAck::Refused
                };
                if transport.ack(seq, ack).await.is_err() {
                    outcome.transport_error = true;
                    break;
                }
            }
            HookFrame::Loss { count, reason, .. } => {
                outcome.declared_losses = outcome.declared_losses.saturating_add(count);
                let _ = collector.declared_hook_loss(count, &reason);
            }
        }
    }
    let _ = collector.channel_state(
        ActivityChannel::Hook,
        if outcome.transport_error {
            "failed"
        } else {
            "closed"
        },
        None,
    );
    outcome
}
