//! Closed, ordered list of identity-chain markers installed in place on an
//! adopted owned source (formats 14 and 15) by the journaled offline
//! maintenance. Every marker still pending is appended in one held writer
//! transaction that follows one verified source backup; the store format,
//! anchor and active closure never change. Each marker keeps its own payload
//! and validation; this list grants no permission, context, read or mutation.
use super::egress::{SourceBackupStepV1, VerifiedSourceBackup, physical_for};
use super::review_authority::{self as authority, AuthorityReceiptV1};
use super::*;
use crate::human_authority::maintenance::HumanMaintenanceSourceToken;
use sha2::{Digest, Sha256};

/// One in-place identity marker. A later family (for example G04's session
/// control) adds its own variant, its place in `ORDERED` and the same
/// per-step contract: `installed(db)` and `install(store, tx, migration_id)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum InPlaceIdentityMarkerV1 {
    #[serde(rename = "human_rights_v2")]
    HumanRightsV2,
    #[serde(rename = "session_prepare_v1")]
    SessionPrepareV1,
}

impl InPlaceIdentityMarkerV1 {
    /// Installation order. A marker may require every earlier one.
    pub(crate) const ORDERED: [Self; 2] = [Self::HumanRightsV2, Self::SessionPrepareV1];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::HumanRightsV2 => "human_rights_v2",
            Self::SessionPrepareV1 => "session_prepare_v1",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::HumanRightsV2 => super::human_rights::GENESIS_KIND,
            Self::SessionPrepareV1 => super::session_prepare::GENESIS_KIND,
        }
    }

    /// Whether this store's identity chain carries the validated marker.
    pub(crate) fn installed(self, db: &Connection) -> anyhow::Result<bool> {
        match self {
            Self::HumanRightsV2 => super::human_rights::installed(db),
            Self::SessionPrepareV1 => super::session_prepare::installed(db),
        }
    }

    /// Create the marker's own schema objects inside the held transaction,
    /// before its genesis event is appended. The rights marker has none.
    fn install_schema(self, tx: &Transaction<'_>) -> anyhow::Result<()> {
        match self {
            Self::HumanRightsV2 => Ok(()),
            Self::SessionPrepareV1 => super::session_prepare::install_schema(tx),
        }
    }

    /// Append this marker inside the caller's held writer transaction, with
    /// its own payload, and return its receipt. Pending until commit.
    pub(crate) fn install(
        self,
        store: &Store,
        tx: &Transaction<'_>,
        migration_id: uuid::Uuid,
    ) -> anyhow::Result<AuthorityReceiptV1> {
        let mut event = match self {
            Self::HumanRightsV2 => super::human_rights::marker_event(tx)?,
            Self::SessionPrepareV1 => super::session_prepare::marker_event(tx)?,
        };
        anyhow::ensure!(event.kind == self.kind(), "identity marker kind differs");
        self.install_schema(tx)?;
        let clock = workload::capture_control_time(tx)?;
        event.id = self.event_id(migration_id);
        event.timestamp = crate::broker_registry::format_time(clock.at());
        let original = serde_json::to_value(&event)?;
        let (event, payload) = store.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "identity marker requires redaction"
        );
        let stored = store.append_authorized_identity_in_transaction(tx, event, payload, &clock)?;
        workload::validate_fresh_control_time(tx, &clock)?;
        anyhow::ensure!(self.installed(tx)?, "identity marker does not validate");
        Ok(AuthorityReceiptV1 {
            seq: stored.seq,
            hash: stored.hash,
        })
    }

    /// Binds a marker to the maintenance that appended it: a UUIDv8 of
    /// SHA-256(domain, migration id, marker name). Distinct per marker, so
    /// one transaction can append several. Not part of the marker payload.
    pub(crate) fn event_id(self, migration_id: uuid::Uuid) -> String {
        let digest = Sha256::new()
            .chain_update(b"ai-guard:in-place-identity-marker:v1\0")
            .chain_update(migration_id.as_bytes())
            .chain_update(self.name().as_bytes())
            .finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        uuid::Builder::from_custom_bytes(bytes)
            .into_uuid()
            .to_string()
    }

    fn stored(self, db: &Connection) -> anyhow::Result<StoredEvent> {
        Ok(db.query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1",
            [self.kind()],
            row_event,
        )?)
    }
}

/// One committed marker of the maintenance, as listed in its journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct IdentityMarkerReceiptV1 {
    pub marker: InPlaceIdentityMarkerV1,
    pub seq: i64,
    pub hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdentityMarkersFailure {
    /// Nothing is pending; refused before any journal record or backup.
    AlreadyInstalled,
    /// Durable markers, prefix or proof differ from this maintenance.
    Conflict,
}
impl std::fmt::Display for IdentityMarkersFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AlreadyInstalled => {
                "every in-place identity marker is already installed on this store; nothing was changed"
            }
            Self::Conflict => "in-place identity marker state differs from this maintenance",
        })
    }
}
impl std::error::Error for IdentityMarkersFailure {}

fn require(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(IdentityMarkersFailure::Conflict.into())
    }
}

/// The held in-place writer on the exact adopted source: unchanged format and
/// the anchor of the retained prior active manifest.
fn source(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<()> {
    physical_for(store, tx, token, SourceBackupStepV1::InPlace)?;
    let d = token.descriptor();
    let expected = token
        .in_place_anchor()?
        .ok_or(authority::ReviewAuthorityFailure::Required)?;
    let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let snapshot = authority::replay(tx)?;
    require(
        format == i64::from(d.required_format)
            && snapshot.anchor.installation_id == d.installation_id
            && snapshot.anchor.source_id == d.source_id
            && snapshot.anchor.kind == d.kind
            && snapshot.anchor.tenant_id == store.tenant_id()
            && snapshot.anchor.durable_namespace_id == d.durable_namespace_id
            && snapshot.anchor_receipt.seq == expected.seq
            && snapshot.anchor_receipt.hash == expected.hash,
    )
}

/// Markers this maintenance committed: contiguous, in `ORDERED` order, each
/// carrying its bound event id. `None` when it committed none.
fn committed(
    db: &Connection,
    migration_id: uuid::Uuid,
) -> anyhow::Result<Option<(AuthorityReceiptV1, Vec<IdentityMarkerReceiptV1>)>> {
    let mut bound: Vec<(InPlaceIdentityMarkerV1, StoredEvent)> = Vec::new();
    let mut foreign = Vec::new();
    for marker in InPlaceIdentityMarkerV1::ORDERED {
        if !marker.installed(db)? {
            continue;
        }
        let stored = marker.stored(db)?;
        if stored.event.id == marker.event_id(migration_id) {
            bound.push((marker, stored));
        } else {
            foreign.push(stored.seq);
        }
    }
    let Some((_, first)) = bound.first() else {
        return Ok(None);
    };
    let prefix = AuthorityReceiptV1 {
        seq: first.seq - 1,
        hash: first.prev_hash.clone(),
    };
    let mut previous = (prefix.seq, prefix.hash.clone());
    let mut receipts = Vec::new();
    for (marker, stored) in &bound {
        require(stored.seq == previous.0 + 1 && stored.prev_hash == previous.1)?;
        previous = (stored.seq, stored.hash.clone());
        receipts.push(IdentityMarkerReceiptV1 {
            marker: *marker,
            seq: stored.seq,
            hash: stored.hash.clone(),
        });
    }
    // A marker installed by another maintenance predates this one's prefix.
    require(foreign.iter().all(|seq| *seq <= prefix.seq))?;
    Ok(Some((prefix, receipts)))
}

/// Prefix and marker list of the in-place maintenance `token`. Before its
/// install: the current head and every marker the store still lacks, refused
/// with `AlreadyInstalled` when none is pending. After it committed: the
/// prefix its first marker follows and the markers it appended.
pub(super) fn maintenance_prefix(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    before_install: bool,
) -> anyhow::Result<(AuthorityReceiptV1, Vec<InPlaceIdentityMarkerV1>)> {
    source(store, tx, token)?;
    let result = match committed(tx, token.migration_id())? {
        Some((prefix, receipts)) => {
            require(!before_install)?;
            (prefix, receipts.iter().map(|r| r.marker).collect())
        }
        None => {
            let mut pending = Vec::new();
            for marker in InPlaceIdentityMarkerV1::ORDERED {
                if !marker.installed(tx)? {
                    pending.push(marker);
                }
            }
            if pending.is_empty() {
                return Err(IdentityMarkersFailure::AlreadyInstalled.into());
            }
            let (seq, hash) = audit_head(tx)?;
            (AuthorityReceiptV1 { seq, hash }, pending)
        }
    };
    physical_for(store, tx, token, SourceBackupStepV1::InPlace)?;
    Ok(result)
}

/// Whether the in-place maintenance `token` already committed its markers.
pub(super) fn is_committed(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<bool> {
    source(store, tx, token)?;
    Ok(committed(tx, token.migration_id())?.is_some())
}

fn read_committed(
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedSourceBackup<'_>,
    markers: &[InPlaceIdentityMarkerV1],
) -> anyhow::Result<Vec<IdentityMarkerReceiptV1>> {
    let (prefix, receipts) =
        committed(tx, token.migration_id())?.ok_or(IdentityMarkersFailure::Conflict)?;
    require(
        (prefix.seq, prefix.hash.as_str())
            == (backup.source_head().0, backup.source_head().1.as_str())
            && receipts
                .iter()
                .map(|r| r.marker)
                .eq(markers.iter().copied()),
    )?;
    Ok(receipts)
}

/// Install every listed pending marker, in order, inside the held writer
/// transaction that follows the verified backup. An already committed run
/// returns its receipts without a second marker. Pending until commit.
pub(super) fn install_in_place(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedSourceBackup<'_>,
    markers: &[InPlaceIdentityMarkerV1],
) -> anyhow::Result<Vec<IdentityMarkerReceiptV1>> {
    super::egress::verify_in_place_backup(store, tx, token, backup)?;
    if committed(tx, token.migration_id())?.is_some() {
        let receipts = read_committed(tx, token, backup, markers)?;
        source(store, tx, token)?;
        return Ok(receipts);
    }
    let (prefix, pending) = maintenance_prefix(store, tx, token, true)?;
    require(
        (prefix.seq, prefix.hash.as_str())
            == (backup.source_head().0, backup.source_head().1.as_str())
            && pending == markers,
    )?;
    let mut receipts = Vec::new();
    for marker in markers {
        let receipt = marker.install(store, tx, token.migration_id())?;
        receipts.push(IdentityMarkerReceiptV1 {
            marker: *marker,
            seq: receipt.seq,
            hash: receipt.hash,
        });
    }
    validate_schema(tx, false)?;
    control::validate(tx)?;
    Store::validate_metadata(tx, store.tenant_id())?;
    require(Store::check_integrity(tx)?.valid)?;
    require(read_committed(tx, token, backup, markers)? == receipts)?;
    source(store, tx, token)?;
    Ok(receipts)
}

/// Read the committed receipts on a reopened source. Appends nothing.
pub(super) fn confirm_in_place(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedSourceBackup<'_>,
    markers: &[InPlaceIdentityMarkerV1],
) -> anyhow::Result<Vec<IdentityMarkerReceiptV1>> {
    super::egress::verify_in_place_backup(store, tx, token, backup)?;
    let receipts = read_committed(tx, token, backup, markers)?;
    source(store, tx, token)?;
    Ok(receipts)
}
