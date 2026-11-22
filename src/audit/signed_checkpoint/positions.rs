//! Audit positions that a checkpoint directory signs or carries forward.
//!
//! Audit retention keeps the events at these positions in the live store, so
//! that `verify --data`, `anchor advance --data` and `create` keep comparing
//! the database with every signed position after a segment is reclaimed.
//! Signatures are not checked here: keeping more events live is always safe,
//! and the trust decision stays with `verify` and its anchor.
use super::{
    log::CheckpointLog,
    report::{Code, Failure},
};
use std::{collections::BTreeSet, path::Path};

pub(crate) fn signed_positions(
    directory: &Path,
    tenant_id: &str,
) -> Result<BTreeSet<u64>, Failure> {
    let set = CheckpointLog::open_reader(directory)?.load()?;
    let mut positions = BTreeSet::new();
    for record in set.records.values().flatten() {
        let statement = record.statement();
        if statement.tenant_id() != tenant_id {
            return Err(Failure::new(
                Code::ScopeMismatch,
                "the checkpoint directory belongs to another tenant",
            )
            .with("field", "tenant_id"));
        }
        positions.insert(statement.signed_position().event_count);
        positions.insert(statement.previous_position().event_count);
    }
    positions.remove(&0);
    Ok(positions)
}
