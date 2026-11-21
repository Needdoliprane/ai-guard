//! Canonical replay of the administrative portion of a prepared registry.
use super::{Change, Genesis, Projection, Record, projection_rows, require};
use crate::{
    audit::workload::projection::{ProjectionRows, canonical_control_time},
    broker_registry::{BrokerRegistryFailureV1, BrokerViewV1, parse_time},
    model::{StoredEvent, event_kind_in_namespace},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use std::ops::Bound::{Excluded, Unbounded};

const GENESIS: &str = "control.broker_registry.genesis";

pub(crate) fn is_event(kind: &str) -> bool {
    event_kind_in_namespace(kind, "control.broker_registry")
}

pub(crate) struct Replay {
    genesis_seq: i64,
    genesis_hash: String,
    seen_genesis: bool,
    projection: Projection,
}

impl Replay {
    pub(crate) fn new(genesis_seq: i64, genesis_hash: String) -> Self {
        Self {
            genesis_seq,
            genesis_hash,
            seen_genesis: false,
            projection: Projection::default(),
        }
    }

    pub(crate) fn observe(
        &mut self,
        stored: &StoredEvent,
        tenant: &str,
        namespace: &str,
        previous_time_floor: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let event = &stored.event;
        require(
            event.source == "gateway"
                && event.provenance == "observed"
                && event.coverage == "observed",
        )?;
        if event.kind == GENESIS {
            let genesis: Genesis = serde_json::from_value(event.data.clone())
                .map_err(|_| BrokerRegistryFailureV1::InvalidRequest)?;
            require(
                !self.seen_genesis
                    && stored.seq == self.genesis_seq
                    && stored.hash == self.genesis_hash
                    && genesis.version == 1
                    && genesis.tenant_id == tenant
                    && genesis.control_namespace_uuid == namespace
                    && stored.seq.checked_sub(1) == Some(genesis.previous_seq)
                    && genesis.previous_hash == stored.prev_hash,
            )?;
            self.seen_genesis = true;
            return Ok(());
        }

        require(self.seen_genesis)?;
        ensure!(
            matches!(
                event.kind.as_str(),
                "control.broker_registry.grant" | "control.broker_registry.revoke"
            ),
            "unsupported MySQL broker registry event"
        );
        let record: Record = serde_json::from_value(event.data.clone())
            .map_err(|_| BrokerRegistryFailureV1::InvalidRequest)?;
        require(
            matches!(&record.change, Change::Grant { .. } | Change::Revoke { .. })
                && event.kind == record.change.kind(),
        )?;
        let control_time = parse_time(&record.received_at)?;
        require(control_time <= parse_time(&stored.received_at)?)?;
        let floor =
            previous_time_floor.context("MySQL broker registry audit time floor is missing")?;
        ensure!(
            canonical_control_time(control_time)? >= floor,
            BrokerRegistryFailureV1::ClockRegression
        );
        self.projection.apply(&record, tenant, stored.seq)?;
        Ok(())
    }

    pub(crate) fn rows(&self) -> Result<ProjectionRows> {
        ensure!(
            self.seen_genesis,
            "MySQL broker registry genesis is missing"
        );
        projection_rows(&self.projection, self.genesis_seq, &self.genesis_hash)
    }

    pub(crate) fn page(&self, cursor: &str, limit: u32, now: &str) -> Result<Vec<BrokerViewV1>> {
        self.projection
            .brokers
            .range((Excluded(cursor.to_owned()), Unbounded))
            .take(limit as usize)
            .map(|(_, (broker, _))| broker.view_at(now))
            .collect()
    }
}
