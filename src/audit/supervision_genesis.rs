//! Wire format shared by stores for the start of the supervision protocol.
use crate::{
    control::invalid,
    model::{Event, StoredEvent},
};
use serde::{Deserialize, Serialize};

pub(crate) const KIND: &str = "control.supervision.genesis";
pub(crate) const PROTOCOL: &str = "broker_supervision_v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    namespace_uuid: String,
    previous_seq: i64,
    previous_hash: String,
}

pub(crate) fn event(
    namespace: &str,
    previous_seq: i64,
    previous_hash: &str,
) -> anyhow::Result<Event> {
    let mut event = Event::new(KIND);
    event.data = serde_json::to_value(Genesis {
        version: 3,
        namespace_uuid: namespace.into(),
        previous_seq,
        previous_hash: previous_hash.into(),
    })?;
    Ok(event)
}

/// Check the marker against the stored envelope. The backend still checks
/// its position in the audit chain and its exact supervision projection.
pub(crate) fn validate(stored: &StoredEvent, namespace: &str) -> anyhow::Result<()> {
    let genesis: Genesis = serde_json::from_value(stored.event.data.clone())?;
    invalid(
        genesis.version == 3
            && genesis.namespace_uuid == namespace
            && stored.seq.checked_sub(1) == Some(genesis.previous_seq)
            && genesis.previous_hash == stored.prev_hash
            && stored.event.kind == KIND
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed",
    )
}
