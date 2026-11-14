//! Tenant-local durable policy registry. No resolver, socket or network permit.
//! Store authority stays private; owned operator HTTP delivery is separately gated.
#![allow(dead_code)]
use super::*;
use crate::{
    api::AuthenticatedReviewRunnerCredential,
    egress::{Authority, EgressPolicyV1, EgressTransport, PolicyLimits},
    human_authority::{HumanSourcesWriteEpoch, OwnedSourceToken},
};
mod types;
pub(crate) use types::Ceiling as EgressTenantCeilingV1;
pub(crate) use types::EgressStoreFailure;
use types::*;
pub use types::{EgressPolicyReceiptV1, EgressPolicySelectorV1, EgressPolicyStateV1};
pub(crate) use types::{
    EgressPolicySnapshotV1, EgressReplacePolicyV1, EgressRevokePolicyV1, EgressStoreLimitsV1,
};
mod schema;
pub(super) use schema::{AUTO_INDEX_COUNTS, SCHEMA};
mod inspection;
mod replay;
pub(super) use inspection::require_inspection_format;
use replay::*;
pub(super) use replay::{marker, validate};
mod mutation;
pub(super) use mutation::TerminalReservation;
mod migration;
pub(crate) use migration::VerifiedEgressSourceBackup;
pub(super) use migration::{
    install_format15, prepare_backup, resume_backup, validate_maintenance_identity,
};

#[cfg(all(test, unix))]
mod fixtures;
#[cfg(all(test, unix))]
mod tests;

/// Every append class uses physical capacity less the remaining durable
/// reservations. Only the private exact-head transition can delete one.
pub(super) fn protected_capacity(
    db: &Connection,
    physical: AuditCapacityV1,
) -> anyhow::Result<AuditCapacityV1> {
    let format: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if format != 15 {
        return Ok(physical);
    }
    let (events, bytes): (i64, i64) = db.query_row(
        "SELECT coalesce(sum(events),0),coalesce(sum(bytes),0) FROM egress_policy_reservations",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(AuditCapacityV1 {
        events: physical
            .events
            .checked_sub(u64::try_from(events)?)
            .ok_or(EgressStoreFailure::Capacity)?,
        bytes: physical
            .bytes
            .checked_sub(u64::try_from(bytes)?)
            .ok_or(EgressStoreFailure::Capacity)?,
    })
}

#[cfg(all(test, unix))]
mod a1_tests;
