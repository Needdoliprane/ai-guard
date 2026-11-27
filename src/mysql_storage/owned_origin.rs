use super::{audit::decode_row, driver_error, supervision::server_uuid};
use anyhow::{Context, Result, bail, ensure};
use mysql_async::{Conn, Row, Transaction, prelude::Queryable};
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod fresh_anchor;
mod preparation;
pub use preparation::{
    MysqlOwnedPreparationPreflight, MysqlOwnedPreparationReceipt, MysqlOwnedPreparationRefusal,
    MysqlOwnedPreparationRequest, preflight_owned_preparation, publish_owned_fresh_source,
};

/// Cooperative ownership of a physical schema on one dedicated SQL session.
/// Only this session can prove or release the lock.
pub(super) struct OwnedAuditLock {
    name: String,
    session: u64,
    server: Uuid,
    schema: String,
}

impl OwnedAuditLock {
    pub(super) async fn acquire(conn: &mut Conn, server: Uuid, database: &str) -> Result<Self> {
        ensure!(
            server_uuid(conn).await? == server,
            "MySQL owned audit server UUID mismatch"
        );
        let row = conn
            .query_first::<Row, _>(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
                 WHERE BINARY SCHEMA_NAME = BINARY DATABASE()",
            )
            .await
            .map_err(driver_error)?
            .context("MySQL owned audit schema is not canonically bound")?;
        let schema: String = decode_row(row)?;
        ensure!(schema == database, "MySQL owned audit database mismatch");

        // Use the canonical physical schema. A changed namespace or tenant
        // cannot create a second lock for that schema. The hex key is 64 bytes.
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:owned-mysql-source:v1\0");
        digest.update(server.as_bytes());
        digest.update(schema.as_bytes());
        let name = hex::encode(digest.finalize());
        let acquired = conn
            .exec_first::<Row, _, _>("SELECT GET_LOCK(?, 0)", (&name,))
            .await
            .map_err(driver_error)?
            .map(decode_row::<Option<i64>>)
            .transpose()?;
        match acquired {
            Some(Some(1)) => Ok(Self {
                name,
                session: u64::from(conn.id()),
                server,
                schema,
            }),
            Some(Some(0)) => bail!("MySQL owned audit target is already held"),
            _ => bail!("MySQL owned audit lock acquisition is uncertain"),
        }
    }

    pub(super) async fn verify(&self, connection: &mut impl Queryable) -> Result<()> {
        let ownership = connection
            .exec_first::<Row, _, _>("SELECT CONNECTION_ID(), IS_USED_LOCK(?)", (&self.name,))
            .await
            .map_err(driver_error)?
            .map(decode_row::<(u64, Option<u64>)>)
            .transpose()?;
        ensure!(
            ownership == Some((self.session, Some(self.session))),
            "MySQL owned audit session does not hold the lock"
        );
        Ok(())
    }

    /// Recheck the physical target in the transaction that inspects preparation state.
    async fn verify_preparation_target(&self, tx: &mut Transaction<'_>) -> Result<()> {
        let row = tx
            .exec_first::<Row, _, _>(
                "SELECT CONNECTION_ID(), IS_USED_LOCK(?), @@GLOBAL.server_uuid, \
                 (SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
                  WHERE BINARY SCHEMA_NAME = BINARY DATABASE())",
                (&self.name,),
            )
            .await
            .map_err(driver_error)?
            .context("MySQL owned preparation origin is unavailable")?;
        let (session, holder, server, schema): (u64, Option<u64>, String, Option<String>) =
            decode_row(row)?;
        ensure!(
            preparation_target_matches(
                (self.session, &self.server.to_string(), &self.schema),
                (session, holder, &server, schema.as_deref()),
            ),
            "MySQL owned preparation transaction differs from the locked target"
        );
        Ok(())
    }

    pub(super) async fn release(self, conn: &mut Conn) -> Result<()> {
        ensure!(
            u64::from(conn.id()) == self.session,
            "MySQL owned audit session changed"
        );
        self.verify(conn).await?;
        let released = conn
            .exec_first::<Row, _, _>("SELECT RELEASE_LOCK(?)", (&self.name,))
            .await
            .map_err(driver_error)?
            .map(decode_row::<Option<i64>>)
            .transpose()?;
        ensure!(
            released == Some(Some(1)),
            "MySQL owned audit lock release is uncertain"
        );
        Ok(())
    }
}

fn preparation_target_matches(
    expected: (u64, &str, &str),
    observed: (u64, Option<u64>, &str, Option<&str>),
) -> bool {
    let (expected_session, expected_server, expected_schema) = expected;
    let (session, holder, server, schema) = observed;
    session == expected_session
        && holder == Some(expected_session)
        && server == expected_server
        && schema == Some(expected_schema)
}

#[cfg(test)]
mod tests {
    use super::preparation_target_matches;

    #[test]
    fn owned_preparation_requires_the_locked_physical_target() {
        let server = "abcde000-0000-0000-0000-000000000001";
        let schema = "tenant_a";
        let expected = (12, server, schema);
        assert!(preparation_target_matches(
            expected,
            (12, Some(12), server, Some(schema))
        ));
        assert!(!preparation_target_matches(
            expected,
            (12, Some(12), server, Some("tenant_b"))
        ));
        assert!(!preparation_target_matches(
            expected,
            (12, Some(12), server, Some("TENANT_A"))
        ));
        assert!(!preparation_target_matches(
            expected,
            (12, Some(12), server, None)
        ));
        assert!(!preparation_target_matches(
            expected,
            (
                12,
                Some(12),
                "abcde000-0000-0000-0000-000000000002",
                Some(schema)
            )
        ));
        assert!(!preparation_target_matches(
            expected,
            (13, Some(12), server, Some(schema))
        ));
        assert!(!preparation_target_matches(
            expected,
            (12, Some(13), server, Some(schema))
        ));
        assert!(!preparation_target_matches(
            expected,
            (12, None, server, Some(schema))
        ));
    }
}
