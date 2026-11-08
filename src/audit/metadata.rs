//! Negative identity discovery is not a proof of authority. Neighboring stores
//! may contribute only an already validated snapshot, without waiting for work
//! or recovering/validating a changed origin on the request's behalf.
use super::*;

#[derive(Debug)]
struct PreparationUnavailable;
impl std::fmt::Display for PreparationUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("identity collision metadata preparation unavailable")
    }
}
impl std::error::Error for PreparationUnavailable {}
pub(super) fn preparation_unavailable(error: &anyhow::Error) -> bool {
    error.is::<PreparationUnavailable>()
}

#[derive(Default)]
pub(super) struct PositiveBindings {
    pub(super) local: BTreeMap<String, crate::identity::LocalCredentialV1>,
    pub(super) workload: BTreeMap<String, crate::workload::WorkloadDigestOwnerV1>,
}
impl PositiveBindings {
    pub(super) fn load(db: &Connection) -> anyhow::Result<Self> {
        Self::load_snapshot(db).context(PreparationUnavailable)
    }
    fn load_snapshot(db: &Connection) -> anyhow::Result<Self> {
        let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let mut bindings = Self::default();
        if version >= 4 {
            let mut statement =
                db.prepare("SELECT digest,payload_json FROM identity_credentials")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (digest, json) = row?;
                bindings.local.insert(digest, serde_json::from_str(&json)?);
            }
        }
        if version >= 6 {
            let mut statement = db.prepare("SELECT d.sha256_digest,d.credential_id,b.payload_json FROM workload_digests d JOIN workload_bindings b ON b.credential_id=d.credential_id")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (digest, credential_id, json) = row?;
                let binding: crate::workload::WorkloadBinding = serde_json::from_str(&json)?;
                anyhow::ensure!(
                    credential_id == binding.credential_id,
                    "workload owner diverged"
                );
                bindings.workload.insert(
                    digest,
                    crate::workload::WorkloadDigestOwnerV1 {
                        credential_id,
                        subject: binding.subject,
                    },
                );
            }
        }
        Ok(bindings)
    }
}

pub(crate) enum MetadataLookup<T> {
    Present(T),
    Absent,
    Unavailable(anyhow::Error),
}
impl<T> MetadataLookup<T> {
    pub(crate) fn into_result(self) -> anyhow::Result<Option<T>> {
        match self {
            Self::Present(value) => Ok(Some(value)),
            Self::Absent => Ok(None),
            Self::Unavailable(error) => Err(error),
        }
    }
}

impl Store {
    /// A committed positive binding may deny ambiguity while a sibling's
    /// connection is busy or has not validated a newer snapshot. A cache miss
    /// never establishes absence and this result never authenticates a source.
    pub(super) fn with_known_binding<T>(
        &self,
        lookup: MetadataLookup<T>,
        known: impl FnOnce(&PositiveBindings) -> Option<T>,
    ) -> MetadataLookup<T> {
        if matches!(&lookup, MetadataLookup::Unavailable(_)) {
            // Independent of both DB and lease mutexes. Even a suspended
            // source can retain known historical ambiguity, while every
            // successful authentication still requires its own checked Store.
            let bindings = self.supervision_lease.positive_bindings.load_full();
            if let Some(binding) = known(&bindings) {
                return MetadataLookup::Present(binding);
            }
        }
        lookup
    }

    pub(super) fn try_read_metadata<T>(
        &self,
        read: impl FnOnce(&Connection) -> anyhow::Result<Option<T>>,
    ) -> MetadataLookup<T> {
        let result = (|| -> anyhow::Result<Option<T>> {
            let mut db = self
                .db
                .try_lock()
                .map_err(|_| anyhow::anyhow!("identity metadata connection unavailable"))?;
            let lease = self
                .supervision_lease
                .state
                .try_lock()
                .map_err(|_| anyhow::anyhow!("identity metadata origin unavailable"))?;
            anyhow::ensure!(
                self.healthy() && !lease.invalidated,
                "identity metadata origin requires recovery"
            );
            // This connection belongs to the Store and remains exclusively
            // held until its normal timeout is restored. Neither SQLite locks
            // nor another live Store handle may turn discovery into a waiter.
            let prior_timeout_ms: i64 =
                db.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
            let prior_timeout_ms =
                u64::try_from(prior_timeout_ms).context("invalid negative SQLite busy timeout")?;
            db.busy_timeout(std::time::Duration::ZERO)?;
            let snapshot = (|| -> anyhow::Result<Option<T>> {
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
                self.backing.verify(&tx)?;
                // The prefix query establishes the read snapshot before its
                // validation markers are compared. No write lock is requested.
                lease.verify(&self.backing, &tx)?;
                let version: i64 = tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
                anyhow::ensure!(
                    version == self.validated_data_version.load(Ordering::Acquire)
                        && tx.total_changes()
                            == self.validated_total_changes.load(Ordering::Acquire),
                    "identity metadata snapshot has not been validated"
                );
                validate_schema(&tx, false)?;
                let value = read(&tx)?;
                self.backing.verify(&tx)?;
                lease.verify(&self.backing, &tx)?;
                tx.rollback()?;
                Ok(value)
            })();
            // Restore on both success and error. Discovery never clears a
            // latch or quarantines an origin after transient resource trouble.
            db.busy_timeout(std::time::Duration::from_millis(prior_timeout_ms))?;
            snapshot
        })();
        match result {
            Ok(Some(value)) => MetadataLookup::Present(value),
            Ok(None) => MetadataLookup::Absent,
            Err(error) => MetadataLookup::Unavailable(error),
        }
    }
}
