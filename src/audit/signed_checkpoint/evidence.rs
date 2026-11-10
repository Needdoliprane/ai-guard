//! Read-only evidence of a tenant audit database on one consistent snapshot.
//!
//! This reader reuses the validators of the gateway startup and of
//! `GET /api/v1/integrity` (`validate_schema`, `Store::validate_metadata`,
//! `Store::check_integrity`, `control::validate`) on a `READ_ONLY` connection.
//! It never opens a `Store`: a `Store` open holds a write transaction and the
//! initialization lock during its whole integrity scan, which would make a
//! concurrent gateway start or admission fail. Here the initialization lock is
//! released as soon as the read-only connection is open and bound to the
//! checked file, before any scan. The main database file is never written; a
//! read-only connection may still create empty `-wal`/`-shm` sidecars and the
//! reserved initialization-lock file next to it.

use super::{
    format::ZERO_HASH,
    report::{Code, Failure},
};
use crate::{
    audit::{Store, audit_head, control, storage::StoreStorage, validate_schema},
    tenancy::validate_tenant_id,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// Facts read from the audit database, all from the same snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvidence {
    pub tenant_id: String,
    /// The real control namespace created with the control genesis event.
    pub namespace_uuid: String,
    pub audit_schema_version: u32,
    pub event_count: u64,
    pub head_hash: String,
    pub control_genesis_seq: u64,
    pub control_genesis_hash: String,
    /// Chain hash at each requested position that exists in this snapshot.
    /// A requested position beyond the head has no entry.
    pub hashes: BTreeMap<u64, String>,
}

fn unavailable(error: anyhow::Error) -> Failure {
    Failure::new(
        Code::AuditUnavailable,
        "the audit database cannot be read under the private storage policy",
    )
    .cause(&error)
}

/// SQLite engine failures that say nothing about the content are operational.
/// Every other validator failure is a refusal of the audit content.
fn classify(error: anyhow::Error, refusal: &str) -> Failure {
    let operational = error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(failure, _)) if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy
                    | rusqlite::ErrorCode::DatabaseLocked
                    | rusqlite::ErrorCode::SystemIoFailure
                    | rusqlite::ErrorCode::CannotOpen
                    | rusqlite::ErrorCode::PermissionDenied
                    | rusqlite::ErrorCode::OutOfMemory
                    | rusqlite::ErrorCode::ReadOnly
            )
        )
    });
    if operational {
        unavailable(error)
    } else {
        Failure::new(Code::AuditChainInvalid, refusal).cause(&error)
    }
}

/// Read the evidence needed to sign or verify a checkpoint. `positions` are
/// event counts whose chain hash is wanted.
pub(crate) fn read_audit_evidence(
    path: &Path,
    tenant_id: &str,
    positions: &[u64],
) -> Result<AuditEvidence, Failure> {
    read_with_observer(path, tenant_id, positions, &mut || {})
}

/// `after_snapshot` runs once the read snapshot is established and before the
/// scan. Production passes a no-op; a test uses it to interleave a live writer.
pub(super) fn read_with_observer(
    path: &Path,
    tenant_id: &str,
    positions: &[u64],
    after_snapshot: &mut dyn FnMut(),
) -> Result<AuditEvidence, Failure> {
    validate_tenant_id(tenant_id).map_err(|error| {
        Failure::new(Code::InvalidArgument, "the tenant identifier is not valid").cause(&error)
    })?;
    let (backing, initialization_guard) = StoreStorage::open_existing(path).map_err(unavailable)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut db = Connection::open_with_flags(backing.path(), flags)
        .map_err(|error| unavailable(error.into()))?;
    backing.verify(&db).map_err(unavailable)?;
    // The connection is now bound to the checked inode. Keeping the
    // initialization lock any longer would block gateway starts and sibling
    // opens for the whole scan.
    drop(initialization_guard);
    db.busy_timeout(Duration::from_secs(5))
        .and_then(|()| db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;"))
        .map_err(|error| unavailable(error.into()))?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| unavailable(error.into()))?;
    // The first read fixes the snapshot used by every check below. It is also
    // the first time the file content is read: a file that is not a SQLite
    // database is refused as content, not reported as a busy database.
    let schema_objects: i64 = tx
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
        .map_err(|error| {
            classify(
                error.into(),
                "the audit database file is not a readable SQLite database",
            )
        })?;
    after_snapshot();
    let user_version: i64 = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| {
            classify(
                error.into(),
                "the audit schema version of the database is unreadable",
            )
        })?;
    if user_version <= 0 {
        // An emptied or truncated file is an empty SQLite database, and a
        // legacy database carries no version either: neither is "a schema
        // newer than this build", and against a signed position the first is
        // the most complete truncation there is.
        return Err(Failure::new(
            Code::AuditSchemaUnversioned,
            "the audit database carries no schema version: it is empty or truncated, or it is a legacy database that the gateway has not upgraded yet",
        )
        .with("user_version", user_version)
        .with("schema_objects", schema_objects));
    }
    if let Err(error) = validate_schema(&tx, false) {
        // With a positive version, this exact message means a version above
        // the ones this build knows: an operational limit of the build, not
        // evidence of an altered chain.
        let failure = if error.to_string() == "unsupported audit schema version" {
            Failure::new(
                Code::AuditSchemaUnsupported,
                "this build does not support the audit schema version of the database",
            )
        } else {
            classify(
                error,
                "the audit schema is not the exact schema shipped for its version",
            )
        };
        return Err(failure.with("user_version", user_version));
    }
    if user_version < 2 {
        return Err(Failure::new(
            Code::AuditNamespaceUnavailable,
            "an audit schema without a control namespace cannot be bound to a checkpoint",
        )
        .with("user_version", user_version));
    }
    let bound: Option<String> = tx
        .query_row(
            "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| classify(error.into(), "the tenant binding is unreadable"))?;
    match bound {
        Some(bound) if bound == tenant_id => {}
        Some(bound) => {
            return Err(Failure::new(
                Code::ScopeMismatch,
                "the audit database is bound to another tenant",
            )
            .with("field", "tenant_id")
            .with("between", vec!["request", "database"])
            .with("expected", tenant_id)
            .with("actual", bound));
        }
        None => {
            return Err(Failure::new(
                Code::AuditChainInvalid,
                "the audit database has no tenant binding",
            ));
        }
    }
    Store::validate_metadata(&tx, tenant_id)
        .map_err(|error| classify(error, "the tenant binding or byte quota is inconsistent"))?;
    let integrity = Store::check_integrity(&tx)
        .map_err(|error| classify(error, "the audit chain could not be verified"))?;
    if !integrity.valid {
        return Err(Failure::new(
            Code::AuditChainInvalid,
            "the existing integrity reader refuses the audit chain",
        )
        .with("integrity_error", integrity.error)
        .with("valid_prefix_event_count", integrity.event_count));
    }
    control::validate(&tx)
        .map_err(|error| classify(error, "control projections do not match the audit chain"))?;
    let (namespace_uuid, genesis_seq, control_genesis_hash): (String, i64, String) = tx
        .query_row(
            "SELECT namespace_uuid,genesis_seq,genesis_hash FROM control_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| classify(error.into(), "the control namespace is unreadable"))?;
    let (head_seq, head_hash) =
        audit_head(&tx).map_err(|error| classify(error, "the audit head is unreadable"))?;
    let converted = (
        u64::try_from(head_seq).ok(),
        u64::try_from(genesis_seq).ok(),
        u32::try_from(user_version).ok(),
    );
    let (Some(event_count), Some(control_genesis_seq), Some(audit_schema_version)) = converted
    else {
        return Err(Failure::new(
            Code::AuditChainInvalid,
            "the audit database reports a negative position or version",
        ));
    };
    if event_count != integrity.event_count || head_hash != integrity.head_hash {
        return Err(Failure::new(
            Code::AuditChainInvalid,
            "the audit head differs from the verified chain within one snapshot",
        ));
    }
    let mut hashes = BTreeMap::new();
    for &position in positions {
        if position == 0 {
            hashes.insert(0, ZERO_HASH.to_owned());
        } else if position <= event_count {
            let sequence = i64::try_from(position).map_err(|_| {
                Failure::new(
                    Code::InvalidArgument,
                    "a requested audit position is out of range",
                )
            })?;
            let hash: Option<String> = tx
                .query_row("SELECT hash FROM events WHERE seq=?1", [sequence], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| classify(error.into(), "an audit event hash is unreadable"))?;
            if let Some(hash) = hash {
                hashes.insert(position, hash);
            }
        }
    }
    backing.verify(&tx).map_err(unavailable)?;
    drop(tx);
    Ok(AuditEvidence {
        tenant_id: tenant_id.to_owned(),
        namespace_uuid,
        audit_schema_version,
        event_count,
        head_hash,
        control_genesis_seq,
        control_genesis_hash,
        hashes,
    })
}
