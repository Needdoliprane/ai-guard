use super::{driver_error, supervision::server_uuid};
use anyhow::{Context, Result, bail, ensure};
use mysql_async::{Conn, Transaction, prelude::Queryable};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Cooperative ownership of a physical schema on one dedicated SQL session.
/// Only this session can prove or release the lock.
pub(super) struct OwnedAuditLock {
    name: String,
    session: u64,
}

impl OwnedAuditLock {
    pub(super) async fn acquire(conn: &mut Conn, server: Uuid, database: &str) -> Result<Self> {
        ensure!(
            server_uuid(conn).await? == server,
            "MySQL owned audit server UUID mismatch"
        );
        let schema = conn
            .query_first::<String, _>(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
                 WHERE BINARY SCHEMA_NAME = BINARY DATABASE()",
            )
            .await
            .map_err(driver_error)?
            .context("MySQL owned audit schema is not canonically bound")?;
        ensure!(schema == database, "MySQL owned audit database mismatch");

        // Use the canonical physical schema. A changed namespace or tenant
        // cannot create a second lock for that schema. The hex key is 64 bytes.
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:owned-mysql-source:v1\0");
        digest.update(server.as_bytes());
        digest.update(schema.as_bytes());
        let name = hex::encode(digest.finalize());
        let acquired = conn
            .exec_first::<Option<i64>, _, _>("SELECT GET_LOCK(?, 0)", (&name,))
            .await
            .map_err(driver_error)?;
        match acquired {
            Some(Some(1)) => Ok(Self {
                name,
                session: u64::from(conn.id()),
            }),
            Some(Some(0)) => bail!("MySQL owned audit target is already held"),
            _ => bail!("MySQL owned audit lock acquisition is uncertain"),
        }
    }

    pub(super) async fn verify(&self, tx: &mut Transaction<'_>) -> Result<()> {
        let ownership = tx
            .exec_first::<(u64, Option<u64>), _, _>(
                "SELECT CONNECTION_ID(), IS_USED_LOCK(?)",
                (&self.name,),
            )
            .await
            .map_err(driver_error)?;
        ensure!(
            ownership == Some((self.session, Some(self.session))),
            "MySQL owned audit session does not hold the lock"
        );
        Ok(())
    }

    pub(super) async fn release(self, conn: &mut Conn) -> Result<()> {
        ensure!(
            u64::from(conn.id()) == self.session,
            "MySQL owned audit session changed"
        );
        let released = conn
            .exec_first::<Option<i64>, _, _>("SELECT RELEASE_LOCK(?)", (&self.name,))
            .await
            .map_err(driver_error)?;
        ensure!(
            released == Some(Some(1)),
            "MySQL owned audit lock release is uncertain"
        );
        Ok(())
    }
}
