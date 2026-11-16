//! Tenant session reservation ledger. No runtime effect capability is issued.
use super::*;
use crate::broker_registry::{
    BrokerIdentityV1, BrokerRuntimeKindV1, BrokerV1, format_time, parse_time,
};
use crate::control::kubernetes_session::validation::{canonical, digest, hash, require, uuid};
use crate::control::kubernetes_session::*;
use chrono::{DateTime, Utc};

mod authorization;
mod confirmation;
mod control_recovery;
mod migration;
mod model;
mod mutations;
mod replay;
#[cfg(test)]
pub(crate) mod tests;

use KubernetesSessionReservationFailureV2 as Failure;
pub(super) const GENESIS_KIND: &str = "control.kubernetes.session.reservation.genesis";
pub(super) const PREFIX: &str = "control.kubernetes.session.";
pub(super) const SCHEMA: [(&str, &str, &str); 3] = [
    (
        "table",
        "kubernetes_session_reservation_meta",
        "CREATE TABLE kubernetes_session_reservation_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "kubernetes_session_reservations",
        "CREATE TABLE kubernetes_session_reservations(reservation_id TEXT PRIMARY KEY NOT NULL,session_id TEXT NOT NULL UNIQUE,attempt_id TEXT NOT NULL UNIQUE,worker_id TEXT NOT NULL UNIQUE,generation INTEGER NOT NULL CHECK(generation=1),input_hash TEXT NOT NULL UNIQUE,session_hash TEXT NOT NULL,attempt_hash TEXT NOT NULL,profile_hash TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('reserved','closed_before_effect')),revision INTEGER NOT NULL CHECK(revision>=1),quota_held INTEGER NOT NULL CHECK(quota_held IN (0,1)),effect_state TEXT NOT NULL CHECK(effect_state='never_issued'),effect_revision INTEGER NOT NULL CHECK(effect_revision=0),cpu_millis INTEGER NOT NULL CHECK(cpu_millis>0 AND cpu_millis<=200000),memory_bytes INTEGER NOT NULL CHECK(memory_bytes>0 AND memory_bytes<=107374182400),deadline_seconds INTEGER NOT NULL CHECK(deadline_seconds>=1 AND deadline_seconds<=86400),snapshot_json TEXT NOT NULL,snapshot_hash TEXT NOT NULL,last_event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),CHECK((state='reserved' AND revision=1 AND quota_held=1) OR (state='closed_before_effect' AND revision=2 AND quota_held=0)))",
    ),
    (
        "index",
        "kubernetes_session_reservations_held",
        "CREATE INDEX kubernetes_session_reservations_held ON kubernetes_session_reservations(quota_held)",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 2] = [
    ("kubernetes_session_reservation_meta", 1),
    ("kubernetes_session_reservations", 6),
];
pub(super) fn conflict(valid: bool) -> anyhow::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Failure::Conflict.into())
    }
}
pub(super) fn permission(valid: bool) -> anyhow::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Failure::PermissionDenied.into())
    }
}
fn require_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if [11, 12, 13].contains(&version) {
        Ok(())
    } else {
        Err(Failure::MigrationRequired.into())
    }
}
// Descriptive replay must preserve the complete11..13 projection in14.
// Mutations continue using require_schema and do not gain modern authority.
fn require_replay_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if [11, 12, 13, 14, 15].contains(&version) {
        Ok(())
    } else {
        Err(Failure::MigrationRequired.into())
    }
}
pub(super) fn memory_bytes(intent: &KubernetesSessionReservationIntentV2) -> anyhow::Result<u64> {
    u64::from(intent.resources().memory_mib)
        .checked_mul(1_048_576)
        .ok_or_else(|| Failure::QuotaExceeded.into())
}
pub(crate) use authorization::{
    KubernetesSessionRecoverBeforeEffectV2, KubernetesSessionRecoveryAuthorizationV2,
    KubernetesSessionRecoveryReceiptLookupV2, KubernetesSessionReservationAuthorizationV2,
};
pub(crate) use confirmation::ConfirmedSessionReservationV2;
pub(crate) use control_recovery::{
    ControlRecoverBeforeEffectV2, ControlRecoveryReceiptLookupV2,
    KubernetesSessionControlRecoveryAuthorizationV2,
};
pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    migration::initialize(store, tx)
}
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    replay::validate(db)
}
pub(super) fn held_usage_before(db: &Connection, before: i64) -> anyhow::Result<(u64, u64, u64)> {
    replay::held_usage_before(db, before)
}

pub(super) fn workload_audit_identity(
    event: &Event,
) -> anyhow::Result<Option<capacity::WorkloadAuditPrincipalV1>> {
    if !event.kind.starts_with(PREFIX) {
        return Ok(None);
    }
    if event.kind == GENESIS_KIND {
        migration::marker(event)?;
        return Ok(None);
    }
    let record = model::record(event)?;
    let broker_id = match &record.creator {
        model::Creator::ThisReserve => record.actor.identity()?.broker_id,
        model::Creator::Reservation { actor, .. } => actor.broker_id.clone(),
    };
    permission(broker_id == record.intent.ids()?.broker_id.to_string())?;
    Ok(Some(capacity::WorkloadAuditPrincipalV1::Broker {
        broker_id,
    }))
}

impl Store {
    /// Explicit backed-up10→11 maintenance. Never recreates execution authority.
    pub fn migrate_kubernetes_session_reservations_v11(
        &self,
        backup: &Path,
    ) -> anyhow::Result<crate::control::MigrationReportV1> {
        self.migrate_control_version(backup, 10, 11)
    }
}
