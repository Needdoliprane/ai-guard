//! AuthLedger5 source facts. Deserializable records describe history; only a
//! retained real token and its live epoch permit access to that history.
use super::*;
use crate::human_authority::{
    HumanSourceKindV1, HumanSourcesReadEpoch, HumanSourcesWriteEpoch, OwnedSourceToken,
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

pub(super) const PROTOCOL: &str = "human_source_authority_v1";
pub(super) const SCHEMA: (&str, &str) = (
    "authentication_source_authority",
    "CREATE TABLE authentication_source_authority(singleton INTEGER PRIMARY KEY CHECK(singleton=1),snapshot_json TEXT NOT NULL,snapshot_hash TEXT NOT NULL,wall_floor TEXT NOT NULL)",
);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthenticationSourceFailure {
    Required,
    Mismatch,
    Busy,
    ClockBackwards,
}
impl std::fmt::Display for AuthenticationSourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Required => "authentication source authority required",
            Self::Mismatch => "authentication source authority differs from durable source",
            Self::Busy => "authentication source is busy",
            Self::ClockBackwards => "authentication source clock is below its durable floor",
        })
    }
}
impl std::error::Error for AuthenticationSourceFailure {}
pub(super) fn exact(value: bool) -> Result<()> {
    ensure!(value, AuthenticationSourceFailure::Mismatch);
    Ok(())
}
fn id(value: Uuid) -> Result<()> {
    exact(!value.is_nil())
}
fn hash(value: &str) -> Result<()> {
    exact(
        value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
    )
}
fn receipt(value: &AuthenticationReceiptV1, namespace: Uuid) -> Result<()> {
    exact(
        value.namespace_id == namespace && value.sequence > 0 && value.sequence <= i64::MAX as u64,
    )?;
    hash(&value.hash)
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceAnchor {
    pub version: u32,
    pub protocol: String,
    pub from_format: u32,
    pub format: u32,
    pub migration_id: Uuid,
    pub installation_id: Uuid,
    pub source_id: Uuid,
    pub durable_namespace_id: Uuid,
    pub kind: HumanSourceKindV1,
    pub previous_sequence: u64,
    pub previous_hash: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceIncarnation {
    pub version: u32,
    pub protocol: String,
    pub operation_id: Uuid,
    pub anchor: AuthenticationReceiptV1,
    pub closure_id: Uuid,
    pub closure_revision: u64,
    pub closure_digest: String,
    pub control_config_public_digest: String,
    pub owner_incarnation: Uuid,
    pub revision: u64,
    pub previous_incarnation: Option<AuthenticationReceiptV1>,
    pub previous_sequence: u64,
    pub previous_hash: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub anchor: SourceAnchor,
    pub anchor_receipt: AuthenticationReceiptV1,
    pub incarnation: Option<(SourceIncarnation, AuthenticationReceiptV1)>,
}
impl Snapshot {
    pub fn apply(
        current: &mut Option<Self>,
        entry: &format::Entry,
        got: &AuthenticationReceiptV1,
        previous: &str,
        installation: Uuid,
    ) -> Result<()> {
        match entry {
            format::Entry::SourceAnchor { anchor } => {
                exact(
                    current.is_none()
                        && anchor.version == 1
                        && anchor.protocol == PROTOCOL
                        && anchor.from_format == 4
                        && anchor.format == 5
                        && anchor.kind == HumanSourceKindV1::AuthenticationLedger
                        && anchor.installation_id == installation
                        && anchor.durable_namespace_id == got.namespace_id
                        && anchor.previous_sequence.checked_add(1) == Some(got.sequence)
                        && anchor.previous_hash == previous,
                )?;
                for value in [
                    anchor.migration_id,
                    anchor.installation_id,
                    anchor.source_id,
                    anchor.durable_namespace_id,
                ] {
                    id(value)?;
                }
                hash(&anchor.previous_hash)?;
                receipt(got, anchor.durable_namespace_id)?;
                *current = Some(Self {
                    anchor: anchor.clone(),
                    anchor_receipt: got.clone(),
                    incarnation: None,
                });
            }
            format::Entry::SourceIncarnation { incarnation: next } => {
                let state = current
                    .as_mut()
                    .ok_or(AuthenticationSourceFailure::Mismatch)?;
                let (revision, prior) = state
                    .incarnation
                    .as_ref()
                    .map_or((0, None), |(value, got)| (value.revision, Some(got)));
                exact(
                    next.version == 1
                        && next.protocol == PROTOCOL
                        && next.anchor == state.anchor_receipt
                        && next.closure_revision > 0
                        && next.closure_revision <= i64::MAX as u64
                        && next.revision <= i64::MAX as u64
                        && revision.checked_add(1) == Some(next.revision)
                        && next.previous_incarnation.as_ref() == prior
                        && next.previous_sequence.checked_add(1) == Some(got.sequence)
                        && next.previous_hash == previous,
                )?;
                for value in [next.operation_id, next.closure_id, next.owner_incarnation] {
                    id(value)?;
                }
                for value in [
                    &next.closure_digest,
                    &next.control_config_public_digest,
                    &next.previous_hash,
                ] {
                    hash(value)?;
                }
                receipt(&next.anchor, got.namespace_id)?;
                state.incarnation = Some((next.clone(), got.clone()));
            }
            _ => return Err(AuthenticationSourceFailure::Mismatch.into()),
        }
        Ok(())
    }
    pub fn write(&self, db: &Connection, wall_floor: &str, redactor: &Redactor) -> Result<()> {
        let json = format::checked_json(self, redactor)?;
        let digest = hex::encode(Sha256::digest(json.as_bytes()));
        exact(db.execute("INSERT INTO authentication_source_authority(singleton,snapshot_json,snapshot_hash,wall_floor) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET snapshot_json=excluded.snapshot_json,snapshot_hash=excluded.snapshot_hash,wall_floor=excluded.wall_floor", params![json, digest, wall_floor])? == 1)
    }
    pub fn verify_projection(
        &self,
        db: &Connection,
        floor: &str,
        redactor: &Redactor,
    ) -> Result<()> {
        let count: i64 = db.query_row(
            "SELECT count(*) FROM authentication_source_authority",
            [],
            |r| r.get(0),
        )?;
        let sizes: (i64,i64,i64) = db.query_row("SELECT length(CAST(snapshot_json AS BLOB)),length(CAST(snapshot_hash AS BLOB)),length(CAST(wall_floor AS BLOB)) FROM authentication_source_authority WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        exact(
            count == 1
                && sizes.0 > 0
                && sizes.0 <= 32_768
                && sizes.1 == 64
                && (20..=40).contains(&sizes.2),
        )?;
        let expected = format::checked_json(self, redactor)?;
        let (json, digest, stored_floor): (String,String,String) = db.query_row("SELECT snapshot_json,snapshot_hash,wall_floor FROM authentication_source_authority WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        exact(
            json == expected
                && digest == hex::encode(Sha256::digest(expected.as_bytes()))
                && stored_floor == floor,
        )
    }
    pub fn match_anchor(&self, source: &OwnedSourceToken) -> Result<()> {
        let d = source.descriptor();
        exact(
            self.anchor.source_id == source.source_id()
                && self.anchor.installation_id == d.installation_id
                && self.anchor.durable_namespace_id == d.durable_namespace_id
                && self.anchor.kind == d.kind
                && self.anchor_receipt.sequence == d.authority_marker_receipt.seq as u64
                && self.anchor_receipt.hash == d.authority_marker_receipt.hash,
        )
    }
    pub fn match_current(&self, source: &OwnedSourceToken) -> Result<()> {
        self.match_anchor(source)?;
        let (i, _) = self
            .incarnation
            .as_ref()
            .ok_or(AuthenticationSourceFailure::Required)?;
        let c = source.closure();
        exact(
            i.owner_incarnation == source.incarnation()
                && i.closure_id == c.closure_id
                && i.closure_revision == c.closure_revision
                && i.closure_digest == c.closure_digest
                && i.control_config_public_digest == c.configuration.public_config_digest,
        )
    }
}

/// No Legacy variant and no deserialization of an access capability.
pub(crate) enum AuthenticationSourceAccess<'e> {
    Read {
        epoch: &'e HumanSourcesReadEpoch,
        source: &'e OwnedSourceToken,
    },
    Write {
        epoch: &'e HumanSourcesWriteEpoch,
        source: &'e OwnedSourceToken,
    },
}
impl<'e> AuthenticationSourceAccess<'e> {
    pub(crate) fn read(
        epoch: &'e HumanSourcesReadEpoch,
        source: &'e OwnedSourceToken,
    ) -> Result<Self> {
        epoch.validate_source(source)?;
        Ok(Self::Read { epoch, source })
    }
    pub(crate) fn write(
        epoch: &'e HumanSourcesWriteEpoch,
        source: &'e OwnedSourceToken,
    ) -> Result<Self> {
        epoch.validate_source(source)?;
        Ok(Self::Write { epoch, source })
    }
    pub(super) fn source(&self) -> &OwnedSourceToken {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => source,
        }
    }
    pub(super) fn check(&self, write: bool) -> Result<()> {
        match self {
            Self::Read { epoch, source } => {
                ensure!(!write, AuthenticationSourceFailure::Required);
                epoch.validate_source(source)?;
            }
            Self::Write { epoch, source } => epoch.validate_source(source)?,
        }
        Ok(())
    }
}
pub(super) fn physical(
    backing: &crate::audit::storage::StoreStorage,
    db: &Connection,
    source: &OwnedSourceToken,
) -> Result<()> {
    source.revalidate_authority()?;
    let d = source.descriptor();
    let (path, identity) = backing.identity_key();
    exact(
        d.kind == HumanSourceKindV1::AuthenticationLedger
            && d.tenant_id.is_none()
            && d.required_format == 5
            && path == d.storage_binding.path()
            && identity.physical_identity()
                == (
                    d.storage_binding.source_device,
                    d.storage_binding.source_inode,
                ),
    )?;
    crate::audit::storage::verify_connection_main(backing, db)?;
    source.revalidate_authority().map_err(Into::into)
}
pub(super) fn require_legacy(db: &Connection) -> Result<()> {
    let v: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        matches!(
            v,
            format::LEGACY_VERSION
                | format::VERIFIED_INITIAL_VERSION
                | format::VERSION
                | format::TENANT_CAPACITY_VERSION
        ),
        AuthenticationSourceFailure::Required
    );
    Ok(())
}
pub(super) fn clock(floor: &str) -> Result<String> {
    let now = format::timestamp();
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&now)? >= chrono::DateTime::parse_from_rfc3339(floor)?,
        AuthenticationSourceFailure::ClockBackwards
    );
    Ok(now)
}
pub(super) fn admission_refusal(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<AuthenticationSourceFailure>(), Some(AuthenticationSourceFailure::Required | AuthenticationSourceFailure::Busy | AuthenticationSourceFailure::ClockBackwards))
        || error.downcast_ref::<crate::human_authority::HumanAuthorityFailure>().is_some()
        || error.downcast_ref::<crate::audit::review_authority::ReviewAuthorityFailure>().is_some()
        || error.chain().any(|cause| cause.downcast_ref::<rusqlite::Error>().is_some_and(|e|
            matches!(e, rusqlite::Error::SqliteFailure(code, _) if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))))
}

#[cfg(all(test, unix))]
mod tests;
