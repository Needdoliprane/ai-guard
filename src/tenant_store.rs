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
    use crate::mysql_storage::audit::MysqlAuditStore;
    use std::{
        future::Future,
        sync::atomic::{AtomicBool, Ordering},
    };
    use tokio::runtime::Handle;

    /// Synchronous facade over the async MySQL Store. Every call is made from
    /// a blocking Store lane (or from startup), never from an async worker.
    pub struct MysqlTenant {
        store: MysqlAuditStore,
        handle: Handle,
        degraded: AtomicBool,
    }

    impl MysqlTenant {
        pub fn new(store: MysqlAuditStore, handle: Handle) -> Self {
            Self {
                store,
                handle,
                degraded: AtomicBool::new(false),
            }
        }

        /// The Store handle for typed async ports.
        pub fn store(&self) -> &MysqlAuditStore {
            &self.store
        }

        pub fn block<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
            let result = if Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| self.handle.block_on(future))
            } else {
                self.handle.block_on(future)
            };
            // Only a confirmed transport failure latches a degraded state. A
            // refusal from the Store is an answer, not an outage.
            match &result {
                Ok(_) => self.degraded.store(false, Ordering::Release),
                Err(error) if crate::mysql_storage::is_transport_failure(error) => {
                    self.degraded.store(true, Ordering::Release)
                }
                Err(_) => {}
            }
            result
        }

        pub fn tenant_id(&self) -> &str {
            self.store.tenant_id()
        }
        pub fn healthy(&self) -> bool {
            !self.degraded.load(Ordering::Acquire)
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
            let Self {
                store,
                handle,
                degraded: _,
            } = Arc::try_unwrap(this)
                .map_err(|_| anyhow::anyhow!("MySQL masking is fixed once the Store is shared"))?;
            Ok(Arc::new(Self::new(
                store.with_additional_redaction(additional)?,
                handle,
            )))
        }
        pub fn covers_redaction(&self, additional: &Redactor) -> bool {
            self.store.redactor_covers(additional)
        }
        pub fn head(&self) -> Result<i64> {
            self.block(self.store.head())
        }
        pub fn events(&self, query: &EventQuery) -> Result<Vec<StoredEvent>> {
            self.block(self.store.events(query))
        }
        pub fn integrity(&self) -> Result<Integrity> {
            self.block(self.store.integrity())
        }
        pub fn append(&self, event: Event) -> Result<StoredEvent> {
            self.block(self.store.append(event))
        }
        pub fn entities(&self, sandboxes: bool) -> Result<Vec<Value>> {
            self.block(self.store.entities(sandboxes))
        }
        pub fn overview(&self) -> Result<Value> {
            self.block(self.store.overview())
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
