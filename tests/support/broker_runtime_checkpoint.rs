//! Read one confirmed revision of the fixture's own journal, without a writer.
use agentsandbox::{
    broker::{
        journal::{JournalScopeV1, RevisionRefV1},
        outbox::{BrokerCheckpointV4, OutboxEventV1},
        service::protocol::{JobStateV1, JobStatusV1},
    },
    control_fs::Directory,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::{
    num::NonZeroU64,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const REVISION_BYTES: u64 = 1024 * 1024;

pub struct TerminalCheckpointProbe {
    job_root: PathBuf,
    operation: Uuid,
    session: String,
    head: Option<CheckpointHead>,
}

struct CheckpointHead {
    revision: RevisionRefV1,
    intent_hash: String,
}

#[derive(Default, Serialize)]
pub struct TerminalCheckpointObservation {
    head_revision: Option<NonZeroU64>,
    result_audit_at_head: OutboxDelivery,
    failure_audit_at_head: OutboxDelivery,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OutboxDelivery {
    #[default]
    Unknown,
    Absent,
    Pending,
    Acknowledged,
}

impl TerminalCheckpointProbe {
    pub fn new(
        fixture_root: &Path,
        scope: &JournalScopeV1,
        operation: Uuid,
        session: &str,
    ) -> serde_json::Result<Self> {
        // The same canonical scope and hash domain used by Journal::open.
        let mut scope_value = serde_json::to_value(scope)?;
        scope_value.sort_all_objects();
        let scope_hash = hex::encode(
            Sha256::new()
                .chain_update(b"ai-guard:broker-journal-scope:v1\0")
                .chain_update(serde_json::to_vec(&scope_value)?)
                .finalize(),
        );
        let job_root = fixture_root
            .join("b/journals")
            .join(scope.profile_id())
            .join("broker-journal-v1")
            .join(scope_hash)
            .join("jobs")
            .join(operation.to_string());
        Ok(Self {
            job_root,
            operation,
            session: session.into(),
            head: None,
        })
    }

    pub fn record_status(&mut self, status: &JobStatusV1) {
        // Status combines a confirmed journal head with a separate executor sample.
        self.head = (|| {
            if status.operation_id != self.operation
                || status.state != JobStateV1::Accepted
                || status.executor_task_running
                || status.executor_error.is_none()
            {
                return None;
            }
            Some(CheckpointHead {
                revision: RevisionRefV1::new(status.head_revision?, status.head_hash.as_deref()?)
                    .ok()?,
                intent_hash: status.intent_hash.as_ref()?.clone(),
            })
        })();
    }

    pub fn read(&self) -> TerminalCheckpointObservation {
        self.read_confirmed().unwrap_or_default()
    }

    fn read_confirmed(&self) -> Option<TerminalCheckpointObservation> {
        let head = self.head.as_ref()?;
        let head_revision = NonZeroU64::new(head.revision.revision())?;
        let directory = Directory::open_existing(&self.job_root).ok()?;
        // Like the Full3 fixture, pin the revision named by Status rather than
        // reading a moving current.json pointer or acquiring the writer lease.
        let file = directory
            .read_private_file(
                format!("rev-{:020}.json", head_revision.get()),
                REVISION_BYTES,
            )
            .ok()?;
        let outbox = confirmed_outbox_payload(file.bytes(), self.operation, head)?;
        let checkpoint = BrokerCheckpointV4::from_bytes(outbox.get().as_bytes()).ok()?;
        file.revalidate().ok()?;
        Some(TerminalCheckpointObservation {
            head_revision: Some(head_revision),
            result_audit_at_head: outbox_delivery(
                checkpoint.events(),
                "broker.batch.result",
                &self.session,
            ),
            failure_audit_at_head: outbox_delivery(
                checkpoint.events(),
                "broker.batch.failure",
                &self.session,
            ),
        })
    }
}

fn confirmed_outbox_payload<'a>(
    bytes: &'a [u8],
    operation: Uuid,
    head: &CheckpointHead,
) -> Option<&'a RawValue> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Revision<'a> {
        version: u32,
        operation_id: Uuid,
        intent_hash: &'a str,
        revision: u64,
        #[serde(rename = "previous")]
        _previous: Option<RevisionRefV1>,
        #[serde(borrow)]
        payload: &'a RawValue,
    }
    #[derive(Deserialize)]
    struct RetirementPayload<'a> {
        #[serde(borrow)]
        outbox: &'a RawValue,
    }

    if bytes.len() as u64 > REVISION_BYTES {
        return None;
    }
    let revision: Revision<'_> = serde_json::from_slice(bytes).ok()?;
    if revision.operation_id != operation
        || revision.revision != head.revision.revision()
        || revision.intent_hash != head.intent_hash
    {
        return None;
    }
    let domain = match revision.version {
        4 => b"ai-guard:broker-checkpoint:v4\0",
        5 => b"ai-guard:broker-checkpoint:v5\0",
        _ => return None,
    };
    let hash = hex::encode(
        Sha256::new()
            .chain_update(domain)
            .chain_update(bytes)
            .finalize(),
    );
    if hash != head.revision.hash() {
        return None;
    }
    if revision.version == 4 {
        Some(revision.payload)
    } else {
        let payload: RetirementPayload<'_> = serde_json::from_str(revision.payload.get()).ok()?;
        Some(payload.outbox)
    }
}

fn outbox_delivery(entries: &[OutboxEventV1], kind: &str, session: &str) -> OutboxDelivery {
    let mut matching = entries.iter().filter(|entry| entry.event().kind == kind);
    let Some(entry) = matching.next() else {
        return OutboxDelivery::Absent;
    };
    let event = entry.event();
    if matching.next().is_some()
        || event.session_id.as_deref() != Some(session)
        || event.source != "runner"
        || event.provenance != "observed"
    {
        return OutboxDelivery::Unknown;
    }
    if entry.audit_seq().is_some() {
        OutboxDelivery::Acknowledged
    } else {
        OutboxDelivery::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentsandbox::model::Event;
    use serde_json::{Value, json};

    #[test]
    fn checkpoint_outbox_view_refuses_unconfirmed_frames() {
        let operation = Uuid::from_u128(1);
        let intent_hash = "a".repeat(64);
        for (version, domain, payload) in [
            (4, b"ai-guard:broker-checkpoint:v4\0", json!({"version":4})),
            (
                5,
                b"ai-guard:broker-checkpoint:v5\0",
                json!({"outbox":{"version":4}}),
            ),
        ] {
            let bytes = serde_json::to_vec(&json!({
                "version":version, "operation_id":operation, "intent_hash":intent_hash,
                "revision":19, "previous":null, "payload":payload
            }))
            .unwrap();
            let hash = hex::encode(
                Sha256::new()
                    .chain_update(domain)
                    .chain_update(&bytes)
                    .finalize(),
            );
            let head = CheckpointHead {
                revision: RevisionRefV1::new(19, &hash).unwrap(),
                intent_hash: intent_hash.clone(),
            };
            let outbox = confirmed_outbox_payload(&bytes, operation, &head).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(outbox.get()).unwrap(),
                json!({"version":4})
            );
            assert!(confirmed_outbox_payload(&bytes, Uuid::from_u128(2), &head).is_none());

            let mut differing = bytes.clone();
            differing.push(b' ');
            assert!(confirmed_outbox_payload(&differing, operation, &head).is_none());
            let wrong_revision = CheckpointHead {
                revision: RevisionRefV1::new(20, &hash).unwrap(),
                intent_hash: intent_hash.clone(),
            };
            assert!(confirmed_outbox_payload(&bytes, operation, &wrong_revision).is_none());
        }
    }

    #[test]
    fn checkpoint_outbox_delivery_distinguishes_absence_pending_ack_and_unknown() {
        let kind = "broker.batch.failure";
        let session = "synthetic-fixture-session";
        let mut event = Event::new(kind);
        event.session_id = Some(session.into());
        event.source = "runner".into();
        event.data = json!({"detail":"synthetic private fixture body"});
        let pending = OutboxEventV1::new(Uuid::from_u128(1), &"a".repeat(64), event).unwrap();
        let acknowledged = pending.acknowledged(22).unwrap();
        assert_eq!(outbox_delivery(&[], kind, session), OutboxDelivery::Absent);
        assert_eq!(
            outbox_delivery(&[pending.clone()], kind, session),
            OutboxDelivery::Pending
        );
        assert_eq!(
            outbox_delivery(&[acknowledged.clone()], kind, session),
            OutboxDelivery::Acknowledged
        );
        assert_eq!(
            outbox_delivery(&[pending.clone()], kind, "different-session"),
            OutboxDelivery::Unknown
        );
        assert_eq!(
            outbox_delivery(&[pending, acknowledged], kind, session),
            OutboxDelivery::Unknown
        );
        assert_eq!(
            serde_json::to_value(TerminalCheckpointObservation::default()).unwrap(),
            json!({"head_revision":null, "result_audit_at_head":"unknown", "failure_audit_at_head":"unknown"})
        );
        assert_eq!(
            serde_json::to_value(TerminalCheckpointObservation {
                head_revision: NonZeroU64::new(19),
                result_audit_at_head: OutboxDelivery::Acknowledged,
                failure_audit_at_head: OutboxDelivery::Pending,
            })
            .unwrap(),
            json!({"head_revision":19, "result_audit_at_head":"acknowledged", "failure_audit_at_head":"pending"})
        );
    }
}
