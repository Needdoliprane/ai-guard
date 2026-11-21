//! Verify the administrative broker registry on an operator-prepared target.
use super::{audit::decode_row, driver_error};
use crate::{
    audit::{prepared_broker_admin, workload::projection::ProjectionCell},
    broker_registry::{BrokerViewV1, format_time},
    model::StoredEvent,
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use mysql_async::{Row, Transaction, Value, prelude::Queryable};

const QUERIES: [(&str, &str); 5] = [
    (
        "broker_registry_meta",
        "SELECT singleton,genesis_seq,genesis_hash FROM ai_guard_broker_registry_meta ORDER BY singleton",
    ),
    (
        "broker_registry_brokers",
        "SELECT broker_id,grant_revision,heartbeat_revision,
         IF(OCTET_LENGTH(payload_json)<=131072,payload_json,NULL),event_seq
         FROM ai_guard_broker_registry_brokers ORDER BY broker_id",
    ),
    (
        "broker_registry_authorities",
        "SELECT authority_id,broker_id,runtime_kind FROM ai_guard_broker_registry_authorities ORDER BY authority_id",
    ),
    (
        "broker_registry_incarnations",
        "SELECT incarnation_id,broker_id,event_seq FROM ai_guard_broker_registry_incarnations ORDER BY incarnation_id",
    ),
    (
        "broker_registry_operations",
        "SELECT operation_id,input_hash,event_seq,broker_id,
         IF(OCTET_LENGTH(result_json)<=131072,result_json,NULL)
         FROM ai_guard_broker_registry_operations ORDER BY operation_id",
    ),
];

pub(super) use crate::audit::prepared_broker_admin::is_event;

async fn schema_counts(tx: &mut Transaction<'_>) -> Result<(u64, u64)> {
    let row = tx
        .query_first::<Row, _>(
            "SELECT COUNT(*), COUNT(CASE WHEN TABLE_TYPE='BASE TABLE' AND ENGINE='InnoDB' THEN 1 END)
             FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA=BINARY DATABASE()
             AND BINARY TABLE_NAME IN ('ai_guard_broker_registry_meta','ai_guard_broker_registry_brokers',
              'ai_guard_broker_registry_authorities','ai_guard_broker_registry_incarnations',
              'ai_guard_broker_registry_operations')",
        )
        .await
        .map_err(driver_error)?
        .context("MySQL broker registry schema inventory unavailable")?;
    decode_row(row)
}

pub(super) struct Replay(prepared_broker_admin::Replay);
pub(super) struct Verified(prepared_broker_admin::Replay);

impl Replay {
    pub(super) async fn open(tx: &mut Transaction<'_>) -> Result<Option<Self>> {
        let counts = schema_counts(tx).await?;
        if counts.0 == 0 {
            return Ok(None);
        }
        let expected = QUERIES.len() as u64;
        ensure!(
            counts == (expected, expected),
            "MySQL broker registry requires all prepared InnoDB tables"
        );
        // Hold the same metadata locks through replay and comparison.
        for (_, query) in QUERIES {
            tx.query_drop(format!("{query} LIMIT 0"))
                .await
                .map_err(driver_error)?;
        }
        ensure!(
            schema_counts(tx).await? == counts,
            "MySQL broker registry schema changed during verification"
        );
        let mut markers: Vec<Row> = tx
            .query("SELECT singleton,genesis_seq,genesis_hash FROM ai_guard_broker_registry_meta LIMIT 2")
            .await
            .map_err(driver_error)?;
        ensure!(
            markers.len() == 1,
            "MySQL broker registry requires one prepared marker"
        );
        let row = markers
            .pop()
            .context("MySQL broker registry marker is missing")?;
        let (singleton, genesis_seq, genesis_hash): (i64, i64, String) = decode_row(row)?;
        ensure!(
            singleton == 1 && genesis_seq > 1 && crate::control::hash(&genesis_hash).is_ok(),
            "invalid MySQL broker registry capability marker"
        );
        Ok(Some(Self(prepared_broker_admin::Replay::new(
            genesis_seq,
            genesis_hash,
        ))))
    }

    pub(super) fn observe(
        &mut self,
        stored: &StoredEvent,
        tenant: &str,
        namespace: &str,
        previous_time_floor: Option<DateTime<Utc>>,
    ) -> Result<()> {
        self.0
            .observe(stored, tenant, namespace, previous_time_floor)
    }

    pub(super) async fn verify(self, tx: &mut Transaction<'_>) -> Result<Verified> {
        let mut expected_rows = self.0.rows()?;
        for (table, query) in QUERIES {
            let expected = expected_rows
                .remove(table)
                .context("missing broker registry projection")?;
            let mut records = tx.exec_iter(query, ()).await.map_err(driver_error)?;
            for expected_row in expected {
                let actual = records.next().await.map_err(driver_error)?;
                ensure!(
                    actual.is_some_and(|row| row_matches(&row, &expected_row)),
                    "MySQL broker registry projection differs from audit: {table}"
                );
            }
            ensure!(
                records.next().await.map_err(driver_error)?.is_none(),
                "MySQL broker registry projection contains unexpected rows: {table}"
            );
            records.drop_result().await.map_err(driver_error)?;
        }
        Ok(Verified(self.0))
    }
}

impl Verified {
    pub(super) fn page(&self, cursor: &str, limit: u32) -> Result<Vec<BrokerViewV1>> {
        self.0.page(cursor, limit, &format_time(Utc::now()))
    }
}

fn row_matches(row: &Row, expected: &[ProjectionCell]) -> bool {
    row.len() == expected.len()
        && expected.iter().enumerate().all(|(index, expected)| {
            match (row.as_ref(index), expected) {
                (Some(Value::Int(actual)), ProjectionCell::Integer(expected)) => actual == expected,
                (Some(Value::UInt(actual)), ProjectionCell::Integer(expected)) => {
                    i64::try_from(*actual).ok() == Some(*expected)
                }
                (Some(Value::Bytes(actual)), ProjectionCell::Text(expected)) => {
                    actual.as_slice() == expected.as_bytes()
                }
                _ => false,
            }
        })
}
