//! Borrowed human-source epochs on the actual Store transaction. No owner,
//! epoch, marker or readiness can be reconstructed from descriptive data here.
use super::*;
use crate::human_authority::{
    HumanAuthorityFailure, HumanSourcesReadEpoch, HumanSourcesWriteEpoch, OwnedSourceToken,
};
use review_authority::ReviewAuthorityFailure;

pub(crate) enum HumanSourceAccess<'e> {
    Read {
        epoch: &'e HumanSourcesReadEpoch,
        source: &'e OwnedSourceToken,
    },
    Write {
        epoch: &'e HumanSourcesWriteEpoch,
        source: &'e OwnedSourceToken,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreOperationClass {
    Business,
    IdentityAuthority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewAccessFailure {
    WriteEpochRequired,
    Busy,
}
impl std::fmt::Display for ReviewAccessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WriteEpochRequired => "identity authority mutation requires a write epoch",
            Self::Busy => "human source transaction is busy",
        })
    }
}
impl std::error::Error for ReviewAccessFailure {}

impl<'e> HumanSourceAccess<'e> {
    pub(crate) fn read(
        epoch: &'e HumanSourcesReadEpoch,
        source: &'e OwnedSourceToken,
    ) -> anyhow::Result<Self> {
        epoch.validate_source(source)?;
        Ok(Self::Read { epoch, source })
    }

    pub(crate) fn write(
        epoch: &'e HumanSourcesWriteEpoch,
        source: &'e OwnedSourceToken,
    ) -> anyhow::Result<Self> {
        epoch.validate_source(source)?;
        Ok(Self::Write { epoch, source })
    }

    fn source(&self) -> &OwnedSourceToken {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => source,
        }
    }

    pub(super) fn check(&self, class: StoreOperationClass) -> anyhow::Result<()> {
        match self {
            Self::Read { epoch, source } => {
                if class == StoreOperationClass::IdentityAuthority {
                    return Err(ReviewAccessFailure::WriteEpochRequired.into());
                }
                epoch.validate_source(source)?;
            }
            Self::Write { epoch, source } => epoch.validate_source(source)?,
        }
        Ok(())
    }

    pub(super) fn validate_connection(
        &self,
        store: &Store,
        db: &Connection,
        class: StoreOperationClass,
    ) -> anyhow::Result<()> {
        self.check(class)?;
        let retained = store
            .review_authority
            .as_ref()
            .ok_or(ReviewAuthorityFailure::Required)?;
        if !retained.source_token().same_owner_and_source(self.source()) {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        // Exact connection, physical source, marker receipt and incarnation
        // replay. revalidate_authority never acquires a recursive epoch.
        store.revalidate_review_source_on_connection(db, self.source())?;
        self.check(class)
    }
}

pub(super) fn require_legacy(db: &Connection) -> anyhow::Result<()> {
    let format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(1..=13).contains(&format) {
        return Err(ReviewAuthorityFailure::Required.into());
    }
    Ok(())
}

pub(super) fn validate_context(
    store: &Store,
    db: &Connection,
    access: Option<&HumanSourceAccess<'_>>,
    class: StoreOperationClass,
) -> anyhow::Result<()> {
    match access {
        Some(access) => access.validate_connection(store, db, class),
        None => require_legacy(db),
    }
}

// These are operation refusals, not assertions of a damaged durable origin.
// Owner/source validators retain their own narrowly typed permanent latch.
pub(super) fn admission_refusal(error: &anyhow::Error, scoped: bool) -> bool {
    error.downcast_ref::<ReviewAccessFailure>().is_some()
        || error.downcast_ref::<HumanAuthorityFailure>().is_some()
        || matches!(
            error.downcast_ref::<ReviewAuthorityFailure>(),
            Some(ReviewAuthorityFailure::Required | ReviewAuthorityFailure::ForeignTransaction
                | ReviewAuthorityFailure::Unavailable)
        )
        || (scoped
            && error.chain().any(|cause| {
                cause.downcast_ref::<rusqlite::Error>().is_some_and(|error| {
                    matches!(error, rusqlite::Error::SqliteFailure(code, _)
                        if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
                })
            }))
}

impl CheckedTransaction<'_> {
    fn require_live_access(&self, class: StoreOperationClass) -> anyhow::Result<()> {
        validate_context(self.origin, self.connection, self.source_access, class)?;
        anyhow::ensure!(
            self.origin.healthy() && !self.lease.invalidated,
            "supervision origin invalidated during transaction"
        );
        self.origin.backing.verify(&self.transaction)?;
        self.lease.verify(&self.origin.backing, &self.transaction)?;
        validate_context(self.origin, self.connection, self.source_access, class)
    }

    /// A business read/write and its mandatory audit may use Read or Write.
    /// This never upgrades that transaction to identity-authority mutation.
    pub(crate) fn require_business_access(&self) -> anyhow::Result<()> {
        self.require_live_access(StoreOperationClass::Business)
    }

    /// Sensitive identity helpers must call this before replay or idempotence;
    /// the Deref to Transaction alone never proves authority to mutate identity.
    pub(crate) fn require_identity_authority(&self) -> anyhow::Result<()> {
        if self.source_access.is_some()
            && self.operation_class != StoreOperationClass::IdentityAuthority
        {
            return Err(ReviewAccessFailure::WriteEpochRequired.into());
        }
        self.require_live_access(StoreOperationClass::IdentityAuthority)
    }
}

impl Store {
    pub(super) fn lock_access_lease(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
    ) -> anyhow::Result<MutexGuard<'_, SupervisionLease>> {
        if access.is_none() {
            return self.lock_supervision_lease();
        }
        match self.supervision_lease.state.try_lock() {
            Ok(lease) => Ok(lease),
            Err(std::sync::TryLockError::WouldBlock) => Err(ReviewAccessFailure::Busy.into()),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.unhealthy.store(true, Ordering::Release);
                self.supervision_lease
                    .invalidated
                    .store(true, Ordering::Release);
                Err(ReviewAuthorityFailure::Unavailable.into())
            }
        }
    }

    pub(super) fn lock_access_capacity(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
    ) -> anyhow::Result<MutexGuard<'_, capacity::Cache>> {
        if access.is_none() {
            return self
                .supervision_lease
                .capacity
                .lock()
                .map_err(|_| anyhow::anyhow!("audit capacity cache lock poisoned"));
        }
        self.supervision_lease
            .capacity
            .try_lock()
            .map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => ReviewAccessFailure::Busy.into(),
                std::sync::TryLockError::Poisoned(_) => {
                    anyhow::anyhow!("audit capacity cache lock poisoned")
                }
            })
    }
}

#[cfg(test)]
mod tests;
