use super::driver_error;
use crate::{audit::supervision_genesis, control::hash, model::StoredEvent};
use anyhow::{Context, Result, anyhow, ensure};
use mysql_async::{Row, Transaction, from_row_opt, prelude::Queryable};
use serde::Serialize;
use uuid::Uuid;

/// Durable boundary and the process-local witness of its fresh creation.
/// A witness alone conveys no permission to start or manage a worker.
#[derive(Debug, Serialize)]
pub struct MysqlSupervisionOrigin {
    pub boundary: Option<StoredEvent>,
    pub fresh_origin_retained: bool,
}

pub(super) struct Boundary {
    seq: i64,
    hash: String,
}

impl Boundary {
    pub(super) fn matches(&self, event: &StoredEvent) -> bool {
        self.seq == event.seq && self.hash == event.hash
    }
}

pub(super) async fn metadata(tx: &mut Transaction<'_>) -> Result<Option<Boundary>> {
    let rows: Vec<Row> = tx
        .query(
            "SELECT singleton,protocol,genesis_seq,genesis_hash
                FROM ai_guard_supervision_meta ORDER BY singleton LIMIT 2",
        )
        .await
        .map_err(driver_error)?;
    ensure!(rows.len() <= 1, "unexpected MySQL supervision metadata");
    rows.into_iter()
        .next()
        .map(|row| {
            let (singleton, protocol, seq, hash_value): (u8, String, i64, String) =
                from_row_opt(row).map_err(|_| anyhow!("unexpected MySQL supervision metadata"))?;
            ensure!(
                singleton == 1 && protocol == supervision_genesis::PROTOCOL && seq == 2,
                "invalid MySQL supervision metadata"
            );
            hash(&hash_value)?;
            Ok(Boundary {
                seq,
                hash: hash_value,
            })
        })
        .transpose()
}

pub(super) async fn persist(tx: &mut Transaction<'_>, event: &StoredEvent) -> Result<()> {
    tx.exec_drop(
        "INSERT INTO ai_guard_supervision_meta (singleton,protocol,genesis_seq,genesis_hash)
         VALUES (1,?,?,?)",
        (supervision_genesis::PROTOCOL, event.seq, &event.hash),
    )
    .await
    .map_err(driver_error)?;
    Ok(())
}

async fn server_uuid(tx: &mut Transaction<'_>) -> Result<Uuid> {
    let id = tx
        .query_first::<String, _>("SELECT @@GLOBAL.server_uuid")
        .await
        .map_err(driver_error)?
        .context("MySQL server identity is unavailable")?;
    let uuid = Uuid::parse_str(&id).map_err(|_| anyhow!("invalid MySQL server identity"))?;
    ensure!(
        !uuid.is_nil() && uuid.to_string() == id,
        "invalid MySQL server identity"
    );
    Ok(uuid)
}

pub(super) struct Witness {
    boundary_hash: String,
    server: Uuid,
}

impl Witness {
    pub(super) async fn capture(tx: &mut Transaction<'_>, boundary: &StoredEvent) -> Result<Self> {
        Ok(Self {
            boundary_hash: boundary.hash.clone(),
            server: server_uuid(tx).await?,
        })
    }

    pub(super) async fn retained(
        &self,
        tx: &mut Transaction<'_>,
        boundary: Option<&StoredEvent>,
    ) -> Result<bool> {
        Ok(
            boundary.is_some_and(|value| value.hash == self.boundary_hash)
                && server_uuid(tx).await? == self.server,
        )
    }
}
