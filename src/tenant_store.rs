//! The tenant's selected audit backend. The gateway is built against this
//! enum so the SQLite Store and the MySQL Store share one routing seam.
//!
//! A MySQL tenant exposes only the families ported to it. Every other family
//! is refused with a typed error before any lock, token read or audit write.
//! The refusal never falls back to another backend.
use crate::{
    audit::{EventQuery, Integrity, Store},
    model::{Event, StoredEvent},
    redaction::Redactor,
};
use anyhow::Result;
use serde_json::Value;
use std::{fmt, sync::Arc};

#[cfg(feature = "mysql-storage")]
pub use mysql::MysqlTenant;

/// The selected backend has not ported this operation family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageCapabilityUnavailable(pub &'static str);

impl fmt::Display for StorageCapabilityUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the configured storage backend does not provide this capability ({})",
            self.0
        )
    }
}
impl std::error::Error for StorageCapabilityUnavailable {}

#[derive(Clone)]
pub enum TenantStore {
    Sqlite(Arc<Store>),
    #[cfg(feature = "mysql-storage")]
    Mysql(Arc<MysqlTenant>),
}

impl From<Arc<Store>> for TenantStore {
    fn from(store: Arc<Store>) -> Self {
        Self::Sqlite(store)
    }
}

impl TenantStore {
    /// SQLite-only families. A MySQL tenant refuses before any effect.
    pub fn sqlite(&self) -> Result<&Arc<Store>, StorageCapabilityUnavailable> {
        match self {
            Self::Sqlite(store) => Ok(store),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(_) => Err(StorageCapabilityUnavailable("sqlite_family")),
        }
    }

    /// Same selected backing instance, for lane and lease accounting.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Sqlite(a), Self::Sqlite(b)) => Arc::ptr_eq(a, b),
            #[cfg(feature = "mysql-storage")]
            (Self::Mysql(a), Self::Mysql(b)) => Arc::ptr_eq(a, b),
            #[cfg(feature = "mysql-storage")]
            _ => false,
        }
    }

    pub(crate) fn owned_redaction_covers(&self, redactor: &Redactor) -> bool {
        match self {
            Self::Sqlite(store) => store.owned_redaction_covers(redactor),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.covers_redaction(redactor),
        }
    }

    /// The MySQL tenant, for operations ported only to that backend's own
    /// transaction model.
    #[cfg(feature = "mysql-storage")]
    pub fn mysql(&self) -> Option<&Arc<MysqlTenant>> {
        match self {
            Self::Mysql(store) => Some(store),
            Self::Sqlite(_) => None,
        }
    }

    /// A neighboring store can deny issuance with a known binding. An absent
    /// result requires both identity projections to have been checked.
    pub(crate) fn neighboring_credential_present(
        &self,
        digest: &[u8; 32],
        #[cfg(feature = "mysql-storage")] mysql_deadline: tokio::time::Instant,
    ) -> Result<bool> {
        match self {
            Self::Sqlite(store) => {
                let workload = store.try_workload_digest_binding(digest).into_result()?;
                let human = store.try_local_credential_binding(digest).into_result()?;
                Ok(workload.is_some() || human.is_some())
            }
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.neighboring_credential_present(digest, mysql_deadline),
        }
    }

    /// Append with the authenticated technical credential bound to the write
    /// transaction on MySQL. SQLite keeps its existing authorization seam.
    pub fn append_guarded(&self, event: Event, guard: Option<[u8; 32]>) -> Result<StoredEvent> {
        match self {
            Self::Sqlite(store) => store.append(event),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.append_guarded(event, guard),
        }
    }

    /// Append a Legacy runner's batch to its selected store. MySQL rechecks
    /// the verified runner digest inside the batch transaction.
    pub(crate) fn append_batch_guarded(
        &self,
        events: Vec<Event>,
        guard: [u8; 32],
    ) -> Result<Vec<StoredEvent>> {
        match self {
            Self::Sqlite(store) => {
                let _ = guard;
                store.append_batch(events)
            }
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => {
                store.block_guarded(Some(guard), store.store().append_batch(events))
            }
        }
    }

    /// Capabilities proven against the selected backend's own verified state.
    pub fn control_capabilities(&self) -> Result<crate::control::ControlCapabilitiesV1> {
        match self {
            Self::Sqlite(store) => {
                let state = store.supervision_state()?;
                let capabilities = crate::control::ControlCapabilitiesV1::from_sqlite_state(
                    store.tenant_id(),
                    &state,
                );
                capabilities.validate()?;
                Ok(capabilities)
            }
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.control_capabilities(),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "sqlite",
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(_) => "mysql",
        }
    }

    pub fn tenant_id(&self) -> &str {
        match self {
            Self::Sqlite(store) => store.tenant_id(),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.tenant_id(),
        }
    }

    pub fn healthy(&self) -> bool {
        match self {
            Self::Sqlite(store) => store.healthy(),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.healthy(),
        }
    }

    pub(crate) fn redactor_union(&self, additional: &Redactor) -> Redactor {
        match self {
            Self::Sqlite(store) => store.redactor_union(additional),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.redactor_union(additional),
        }
    }

    /// Attach the gateway's masking union. A MySQL tenant must still be
    /// unshared and unverified, so its codec cannot change under a reader.
    pub fn bind_redaction(self, additional: &Redactor) -> Result<Self> {
        Ok(match self {
            Self::Sqlite(store) => Self::Sqlite(store.with_additional_redaction(additional)?),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => {
                Self::Mysql(MysqlTenant::with_additional_redaction(store, additional)?)
            }
        })
    }

    /// Seed each MySQL tenant's startup verification with the gateway's entire
    /// technical credential index, including credentials of neighboring tenants.
    #[cfg(feature = "mysql-storage")]
    pub(crate) fn bind_control_collision_candidates(&mut self, digests: &[[u8; 32]]) -> Result<()> {
        if let Self::Mysql(store) = self {
            Arc::get_mut(store)
                .ok_or_else(|| {
                    anyhow::anyhow!("MySQL collision candidates require an unshared Store")
                })?
                .set_control_collision_candidates(digests)?;
        }
        Ok(())
    }

    /// A per-request sibling with a wider masking union. A MySQL tenant keeps
    /// its codec and refuses a union it does not already cover.
    pub fn with_additional_redaction(&self, additional: &Redactor) -> Result<Self> {
        Ok(match self {
            Self::Sqlite(store) => Self::Sqlite(store.with_additional_redaction(additional)?),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => {
                anyhow::ensure!(
                    store.covers_redaction(additional),
                    "MySQL masking is fixed when the tenant Store is built"
                );
                Self::Mysql(Arc::clone(store))
            }
        })
    }

    pub fn head(&self) -> Result<i64> {
        match self {
            Self::Sqlite(store) => store.head(),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.head(),
        }
    }

    pub fn events(&self, query: &EventQuery) -> Result<Vec<StoredEvent>> {
        match self {
            Self::Sqlite(store) => store.events(query),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.events(query),
        }
    }

    pub fn integrity(&self) -> Result<Integrity> {
        match self {
            Self::Sqlite(store) => store.integrity(),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.integrity(),
        }
    }

    pub(crate) fn audit_retention_ledger(
        &self,
    ) -> Result<Option<crate::audit::retention::RetentionLedgerV1>> {
        match self {
            Self::Sqlite(store) => store.audit_retention_ledger().map(Some),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(_) => Ok(None),
        }
    }

    /// Verify the bound audit and the authority required to serve requests.
    pub(crate) fn verify_gateway_startup(&self) -> Result<()> {
        match self {
            Self::Sqlite(store) => {
                anyhow::ensure!(store.integrity()?.valid, "audit integrity failure")
            }
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => {
                store
                    .block(store.store().verify_control_projections())
                    .map_err(|error| error.context("MySQL startup audit verification failed"))?;
            }
        }
        Ok(())
    }

    pub fn append(&self, event: Event) -> Result<StoredEvent> {
        match self {
            Self::Sqlite(store) => store.append(event),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.append(event),
        }
    }

    pub fn entities(&self, sandboxes: bool) -> Result<Vec<Value>> {
        match self {
            Self::Sqlite(store) => store.entities(sandboxes),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.entities(sandboxes),
        }
    }

    pub fn overview(&self) -> Result<Value> {
        match self {
            Self::Sqlite(store) => store.overview(),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(store) => store.overview(),
        }
    }
}

#[cfg(feature = "mysql-storage")]
mod mysql {
    use super::*;
    use crate::mysql_storage::audit::{MysqlAuditStore, RequiredAuditVerificationFailed};
    use std::{
        future::Future,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };
    use tokio::runtime::Handle;

    #[derive(Default)]
    struct ControlVerification {
        failures: u64,
        unavailable: bool,
    }

    impl ControlVerification {
        fn record_failure(&mut self) {
            self.failures = self.failures.saturating_add(1);
            self.unavailable = true;
        }

        fn recover(&mut self, failures_at_start: u64) {
            if self.failures == failures_at_start && self.failures != u64::MAX {
                // A check already in flight cannot clear a newer failure.
                self.unavailable = false;
            }
        }
    }

    /// Synchronous facade over the async MySQL Store. Every call is made from
    /// a blocking Store lane (or from startup), never from an async worker.
    pub struct MysqlTenant {
        store: MysqlAuditStore,
        handle: Handle,
        transport_degraded: AtomicBool,
        control_verification: Mutex<ControlVerification>,
    }

    impl MysqlTenant {
        pub fn new(store: MysqlAuditStore, handle: Handle) -> Self {
            Self {
                store,
                handle,
                transport_degraded: AtomicBool::new(false),
                control_verification: Mutex::default(),
            }
        }

        /// The Store handle for typed async ports.
        pub fn store(&self) -> &MysqlAuditStore {
            &self.store
        }

        pub(super) fn set_control_collision_candidates(
            &mut self,
            digests: &[[u8; 32]],
        ) -> Result<()> {
            self.store.set_control_collision_candidates(digests)
        }

        fn wait<T>(&self, future: impl Future<Output = T>) -> T {
            if Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| self.handle.block_on(future))
            } else {
                self.handle.block_on(future)
            }
        }

        pub fn block<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
            let result = self.wait(future);
            // This records transport availability. Selected audit proof has
            // its own health state.
            match &result {
                Ok(_) => self.transport_degraded.store(false, Ordering::Release),
                Err(error) if crate::mysql_storage::is_transport_failure(error) => {
                    self.transport_degraded.store(true, Ordering::Release)
                }
                Err(_) => {}
            }
            result
        }

        pub fn tenant_id(&self) -> &str {
            self.store.tenant_id()
        }
        pub fn healthy(&self) -> bool {
            self.control_verification.lock().is_ok_and(|state| {
                !state.unavailable && !self.transport_degraded.load(Ordering::Acquire)
            })
        }
        pub fn redactor_union(&self, additional: &Redactor) -> Redactor {
            self.store.redactor_union(additional)
        }
        /// Valid only while this handle is unique, before the gateway state
        /// shares it. A verified prefix cannot change its masking codec.
        pub fn with_additional_redaction(
            this: Arc<Self>,
            additional: &Redactor,
        ) -> Result<Arc<Self>> {
            if this.store.redactor_covers(additional) {
                return Ok(this);
            }
            let Self { store, handle, .. } = Arc::try_unwrap(this)
                .map_err(|_| anyhow::anyhow!("MySQL masking is fixed once the Store is shared"))?;
            Ok(Arc::new(Self::new(
                store.with_additional_redaction(additional)?,
                handle,
            )))
        }
        pub fn control_capabilities(&self) -> Result<crate::control::ControlCapabilitiesV1> {
            self.block_verifying_audit(self.store.control_capabilities())
        }
        pub fn covers_redaction(&self, additional: &Redactor) -> bool {
            self.store.redactor_covers(additional)
        }
        pub fn head(&self) -> Result<i64> {
            self.block_verifying_audit(self.store.head())
        }
        pub fn events(&self, query: &EventQuery) -> Result<Vec<StoredEvent>> {
            self.block_verifying_audit(self.store.events(query))
        }
        pub fn integrity(&self) -> Result<Integrity> {
            let result = self.block_verifying_audit(self.store.integrity())?;
            if !result.valid {
                self.lock_control_verification()?.record_failure();
            }
            Ok(result)
        }
        pub fn append(&self, event: Event) -> Result<StoredEvent> {
            self.block(self.store.append(event))
        }
        /// The technical digest is checked against the verified identity and
        /// workload projections of the append's own transaction.
        pub fn append_guarded(&self, event: Event, guard: Option<[u8; 32]>) -> Result<StoredEvent> {
            self.block_guarded(guard, self.store.append(event))
        }
        pub(super) fn block_guarded<T>(
            &self,
            guard: Option<[u8; 32]>,
            operation: impl Future<Output = Result<T>>,
        ) -> Result<T> {
            match guard {
                Some(digest) => self.block_verifying_audit(
                    crate::mysql_storage::audit::with_control_guard(digest, operation),
                ),
                None => self.block(operation),
            }
        }
        fn block_verifying_audit<T>(
            &self,
            operation: impl Future<Output = Result<T>>,
        ) -> Result<T> {
            let result = self.block(operation);
            if result
                .as_ref()
                .is_err_and(|error| error.is::<RequiredAuditVerificationFailed>())
            {
                self.lock_control_verification()?.record_failure();
            }
            result
        }
        fn lock_control_verification(
            &self,
        ) -> Result<std::sync::MutexGuard<'_, ControlVerification>> {
            self.control_verification
                .lock()
                .map_err(|_| anyhow::anyhow!("MySQL control verification state unavailable"))
        }
        /// One verified snapshot answers human and workload presence for the
        /// selected tenant. A collision is a verified answer. An unavailable
        /// proof remains visible until a fresh selected check verifies both
        /// projections, independently of transport and peer observations.
        pub fn control_presence(
            &self,
            digest: &[u8; 32],
        ) -> Result<Option<crate::mysql_storage::audit::ControlCollision>> {
            let failures_at_start = self.lock_control_verification()?.failures;
            let result = self.block(self.store.verify_control_credential_presence(digest));
            let mut state = self.lock_control_verification()?;
            if result.is_ok() {
                state.recover(failures_at_start);
            } else {
                state.record_failure();
            }
            result
        }

        /// This peer metadata lookup has its own budget and never changes the
        /// liveness latch used by the selected tenant's operations.
        pub(super) fn neighboring_credential_present(
            &self,
            digest: &[u8; 32],
            deadline: tokio::time::Instant,
        ) -> Result<bool> {
            self.wait(async {
                anyhow::ensure!(
                    tokio::time::Instant::now() < deadline,
                    "MySQL credential lookup budget exhausted"
                );
                let presence = tokio::time::timeout_at(
                    deadline,
                    self.store.verify_control_credential_presence(digest),
                )
                .await
                .map_err(|_| anyhow::anyhow!("MySQL credential lookup budget exhausted"))??;
                Ok(presence.is_some())
            })
        }

        /// A positive from the last fully verified read; opens no connection.
        pub fn known_control_collision(
            &self,
            digest: &[u8; 32],
        ) -> crate::mysql_storage::audit::KnownControlCollision {
            self.store.known_control_collision(digest)
        }
        pub fn entities(&self, sandboxes: bool) -> Result<Vec<Value>> {
            self.block_verifying_audit(self.store.entities(sandboxes))
        }
        pub fn overview(&self) -> Result<Value> {
            self.block_verifying_audit(self.store.overview())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::ControlVerification;

        #[test]
        fn a_success_already_in_flight_cannot_clear_a_new_failure() {
            let mut state = ControlVerification::default();
            state.record_failure();
            let stale = state.failures;
            state.record_failure();
            state.recover(stale);
            assert!(state.unavailable);
            state.recover(state.failures);
            assert!(!state.unavailable);
        }
    }
}

/// Failure classification needs only the Store's liveness, whichever backend.
pub trait StoreHealth {
    fn store_healthy(&self) -> bool;
}
impl StoreHealth for Store {
    fn store_healthy(&self) -> bool {
        self.healthy()
    }
}
impl StoreHealth for TenantStore {
    fn store_healthy(&self) -> bool {
        self.healthy()
    }
}

#[cfg(all(test, feature = "mysql-storage"))]
mod tests {
    use super::*;
    use crate::mysql_storage::{
        MysqlCredentials, MysqlEndpoint, MysqlProbeError, audit::MysqlAuditStore,
        is_transport_failure,
    };

    fn mysql_store(redactor: Redactor) -> TenantStore {
        // No connection is opened: construction only validates the profile.
        let store = MysqlAuditStore::new(
            MysqlEndpoint {
                host: "db.example.invalid",
                port: 3306,
                database: "audit_a",
                username: "writer_a",
            },
            MysqlCredentials {
                password: "fixture-password",
                ca_pem: b"-----BEGIN CERTIFICATE-----\n",
                client_identity: None,
            },
            "tenant-a",
            uuid::Uuid::from_u128(1),
            redactor,
            100,
        )
        .unwrap();
        TenantStore::Mysql(Arc::new(MysqlTenant::new(
            store,
            tokio::runtime::Handle::current(),
        )))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_mysql_tenant_refuses_sqlite_families_with_a_typed_error() {
        let store = mysql_store(Redactor::new(vec![]));
        assert_eq!(store.backend_name(), "mysql");
        assert!(store.mysql().is_some());
        let refusal = store.sqlite().err().expect("MySQL has no SQLite Store");
        assert_eq!(refusal, StorageCapabilityUnavailable("sqlite_family"));
        let wrapped = anyhow::Error::new(refusal);
        assert!(
            wrapped
                .downcast_ref::<StorageCapabilityUnavailable>()
                .is_some()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_mysql_tenant_keeps_its_masking_codec_and_refuses_a_wider_union() {
        let store = mysql_store(Redactor::new(vec!["fixture-secret-a".into()]));
        let covered = Redactor::new(vec!["fixture-secret-a".into()]);
        assert!(store.with_additional_redaction(&covered).is_ok());
        let wider = Redactor::new(vec!["fixture-secret-b".into()]);
        assert!(store.with_additional_redaction(&wider).is_err());
        assert!(!store.owned_redaction_covers(&wider));
    }

    #[test]
    fn only_a_transport_or_timeout_failure_is_an_outage() {
        for error in [
            MysqlProbeError::TransportUnavailable,
            MysqlProbeError::TimedOut,
        ] {
            assert!(is_transport_failure(&anyhow::Error::new(error)));
        }
        for error in [
            MysqlProbeError::InvalidConfiguration,
            MysqlProbeError::ServerRefused(1045),
            MysqlProbeError::SessionMismatch,
        ] {
            assert!(!is_transport_failure(&anyhow::Error::new(error)));
        }
        assert!(!is_transport_failure(&anyhow::anyhow!("other")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_health_latch_follows_outages_and_ignores_store_refusals() {
        let store = mysql_store(Redactor::new(vec![]));
        let TenantStore::Mysql(tenant) = &store else {
            unreachable!()
        };
        assert!(store.healthy());
        let refused =
            tenant.block(async { Err::<(), _>(MysqlProbeError::ServerRefused(1045).into()) });
        assert!(
            refused.is_err() && store.healthy(),
            "a refusal is an answer"
        );
        let outage =
            tenant.block(async { Err::<(), _>(MysqlProbeError::TransportUnavailable.into()) });
        assert!(outage.is_err() && !store.healthy());
        let refused =
            tenant.block(async { Err::<(), _>(MysqlProbeError::ServerRefused(1045).into()) });
        assert!(
            refused.is_err() && !store.healthy(),
            "a refusal does not clear an outage"
        );
        assert!(tenant.block(async { Ok(()) }).is_ok() && store.healthy());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn guarded_write_verification_failure_stays_visible_after_other_results() {
        use crate::mysql_storage::audit::{
            BatchConflict, CommitUncertain, ControlCollision, ControlGuardRefused,
            RequiredAuditVerificationFailed,
        };

        let store = mysql_store(Redactor::new(vec![]));
        let tenant = store.mysql().unwrap();
        let guard = Some([0; 32]);
        for refusal in [
            anyhow::Error::new(ControlGuardRefused(ControlCollision::Human)),
            anyhow::Error::new(BatchConflict),
            anyhow::Error::new(CommitUncertain),
        ] {
            assert!(
                tenant
                    .block_guarded(guard, async { Err::<(), _>(refusal) })
                    .is_err()
            );
            assert!(store.healthy());
        }

        let cause = MysqlProbeError::SessionMismatch;
        let tagged = RequiredAuditVerificationFailed::tag(cause.into(), true);
        assert_eq!(tagged.to_string(), cause.to_string());
        assert_eq!(tagged.downcast_ref::<MysqlProbeError>(), Some(&cause));
        assert!(!is_transport_failure(&tagged));
        assert!(
            tenant
                .block_guarded(guard, async { Err::<(), _>(tagged) })
                .is_err()
        );
        assert!(!store.healthy());

        let timed_out =
            RequiredAuditVerificationFailed::tag(MysqlProbeError::TimedOut.into(), true);
        assert!(is_transport_failure(&timed_out));
        assert!(tenant.block(async { Ok(()) }).is_ok());
        assert!(tenant.block_guarded(guard, async { Ok(()) }).is_ok());
        assert!(!store.healthy());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_timeout_before_proof_requires_more_than_transport_recovery() {
        use crate::mysql_storage::audit::RequiredAuditVerificationFailed;

        let store = mysql_store(Redactor::new(vec![]));
        let tenant = store.mysql().unwrap();
        let guard = Some([0; 32]);
        let timeout = MysqlProbeError::TimedOut;

        let after_proof = RequiredAuditVerificationFailed::tag(timeout.into(), false);
        assert!(is_transport_failure(&after_proof));
        assert!(
            tenant
                .block_guarded(guard, async { Err::<(), _>(after_proof) })
                .is_err()
        );
        assert!(!store.healthy());
        assert!(tenant.block(async { Ok(()) }).is_ok() && store.healthy());

        let before_proof = RequiredAuditVerificationFailed::tag(timeout.into(), true);
        assert!(is_transport_failure(&before_proof));
        assert!(
            tenant
                .block_guarded(guard, async { Err::<(), _>(before_proof) })
                .is_err()
        );
        assert!(!store.healthy());
        assert!(tenant.block(async { Ok(()) }).is_ok());
        assert!(!store.healthy());
    }
}
