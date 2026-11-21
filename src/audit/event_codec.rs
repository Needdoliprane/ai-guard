use super::RETIREMENT_RESUME_EVENT_KIND;
use crate::{
    model::{Event, event_kind_in_namespace},
    redaction::Redactor,
};
use sha2::{Digest, Sha256};

pub(crate) fn prepare_event(
    redactor: &Redactor,
    mut event: Event,
) -> anyhow::Result<(Event, String)> {
    event.validate()?;
    event.timestamp = chrono::DateTime::parse_from_rfc3339(&event.timestamp)?
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let event: Event = serde_json::from_value(redactor.value(&serde_json::to_value(event)?))?;
    let payload = serde_json::to_string(&event)?;
    anyhow::ensure!(
        payload.len() <= 131_072,
        "event exceeds 128 KiB after redaction"
    );
    Ok((event, payload))
}

pub(crate) fn digest(seq: i64, received: &str, prev: &str, payload: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"agentsandbox:audit:v1\0");
    h.update(seq.to_be_bytes());
    for s in [received, prev, payload] {
        h.update(s.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

pub(crate) fn require_ordinary_kind(kind: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !event_kind_in_namespace(kind, "control")
            && kind != RETIREMENT_RESUME_EVENT_KIND
            && !crate::relay::admission::reserved_kind(kind),
        "control events require typed mutation"
    );
    Ok(())
}
