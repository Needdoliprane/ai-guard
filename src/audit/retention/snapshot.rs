//! Legacy dashboard projections carried across a retention base.
//!
//! The technical entity views and overview counters fold every event. After
//! a segment reclaims leaf events, they resume from the snapshot of the fold
//! through the segment boundary and observe only later events, so the views
//! stay those of the complete history. `verify` recomputes each snapshot from
//! the archives and compares its digest.
use super::failure::{RetentionFailure, code};
use crate::{
    audit::entities::{Projection, ProjectionStateV1},
    model::{Event, Outcome},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bound of one persisted snapshot; a larger one refuses the segment.
pub(crate) const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotCountsV1 {
    pub(crate) commands: i64,
    pub(crate) allowed: i64,
    pub(crate) denied: i64,
    pub(crate) security: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetentionSnapshotV1 {
    pub(crate) version: u32,
    pub(crate) through_seq: i64,
    pub(crate) agents: ProjectionStateV1,
    pub(crate) sandboxes: ProjectionStateV1,
    pub(crate) counts: SnapshotCountsV1,
}

pub(crate) struct SnapshotFold {
    through_seq: i64,
    agents: Projection,
    sandboxes: Projection,
    counts: SnapshotCountsV1,
}

impl SnapshotFold {
    pub(crate) fn empty() -> Self {
        Self {
            through_seq: 0,
            agents: Projection::new(false),
            sandboxes: Projection::new(true),
            counts: SnapshotCountsV1::default(),
        }
    }

    pub(crate) fn resume(snapshot: RetentionSnapshotV1) -> Self {
        Self {
            through_seq: snapshot.through_seq,
            agents: Projection::from_state(false, snapshot.agents),
            sandboxes: Projection::from_state(true, snapshot.sandboxes),
            counts: snapshot.counts,
        }
    }

    /// Mirror the SQL of `Store::entities` and `Store::overview` exactly.
    pub(crate) fn observe(&mut self, seq: i64, event: &Event) {
        self.through_seq = seq;
        if event.agent_id.is_some() {
            self.agents.observe(event);
        }
        if event.sandbox_id.is_some() {
            self.sandboxes.observe(event);
        }
        observe_counts(&mut self.counts, event);
    }

    pub(crate) fn snapshot(&self) -> RetentionSnapshotV1 {
        RetentionSnapshotV1 {
            version: 1,
            through_seq: self.through_seq,
            agents: self.agents.state(),
            sandboxes: self.sandboxes.state(),
            counts: self.counts.clone(),
        }
    }

    pub(crate) fn finish(self) -> RetentionSnapshotV1 {
        self.snapshot()
    }
}

pub(crate) fn observe_counts(counts: &mut SnapshotCountsV1, event: &Event) {
    if event.kind == "command.requested" {
        counts.commands += 1;
    }
    if event.kind == "security.decision" {
        match event.decision.as_ref().map(|decision| &decision.outcome) {
            Some(Outcome::Allow) => counts.allowed += 1,
            Some(Outcome::Deny) => counts.denied += 1,
            _ => {}
        }
    }
    // SQLite `kind LIKE 'security.%'` is ASCII case-insensitive.
    const PREFIX: &str = "security.";
    if event
        .kind
        .get(..PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
    {
        counts.security += 1;
    }
}

/// Canonical encoding and its digest; refuses an oversized snapshot.
pub(crate) fn encode(snapshot: &RetentionSnapshotV1) -> Result<(String, String), RetentionFailure> {
    let json = serde_json::to_string(snapshot).map_err(|error| {
        RetentionFailure::new(
            code::SNAPSHOT_TOO_LARGE,
            "the snapshot could not be encoded",
        )
        .with("cause", error.to_string())
    })?;
    if json.len() > MAX_SNAPSHOT_BYTES {
        return Err(RetentionFailure::new(
            code::SNAPSHOT_TOO_LARGE,
            "the legacy entity snapshot exceeds 8 MiB; reduce the segment or contact the maintainers",
        )
        .with("bytes", json.len()));
    }
    let digest = sha256(&json);
    Ok((json, digest))
}

pub(crate) fn sha256(json: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:audit-retention-snapshot:v1\0");
    digest.update(json.as_bytes());
    hex::encode(digest.finalize())
}

pub(crate) fn decode(json: &str) -> anyhow::Result<RetentionSnapshotV1> {
    let snapshot: RetentionSnapshotV1 = serde_json::from_str(json)?;
    anyhow::ensure!(snapshot.version == 1, "unsupported retention snapshot");
    Ok(snapshot)
}
