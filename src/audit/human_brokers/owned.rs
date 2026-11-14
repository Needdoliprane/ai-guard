//! Owned (formats 14/15) human broker registry projection: a closed,
//! whitelisted Administration view read under the caller's retained source
//! access with the modern checked read protocol. Its shape differs from the
//! legacy registry page, so it is served on its own path and names its own
//! `profile`; it never answers the legacy page's path or parser.
//!
//! The mandatory read audit (same record, kind and human read budget as the
//! legacy projection) is appended in the read transaction before any page is
//! returned, including for an empty page; an audit refusal returns no DTO.
//! Exposed: broker and incarnation identifiers, heartbeat/grant state, grant
//! revision, revocation, expiry, lease and declared authority runtime kinds,
//! heartbeat reception and lease expiry, and a checkpoint summary (reception,
//! expiry, freshness, coverage, reference count, `has_more`, provenance).
//! Never exposed: job IDs, checkpoint or audit hashes, operation IDs,
//! credential IDs or digests, commands, paths, raw grants or secrets.
//! A page is never a tenant total; heartbeat and checkpoint freshness are
//! independent; an unknown checkpoint state is never reported as absent; no
//! worker or broker link is inferred from a label.
use super::*;
use crate::{
    audit::broker_registry::owned_checkpoint::{
        OwnedCheckpointLookup, OwnedCheckpointWindow, owned_checkpoint_lookup,
    },
    broker_registry::{
        BrokerAuthorityV1, BrokerCheckpointCoverageV1, BrokerCheckpointFreshnessV1,
        BrokerClaimProvenanceV1, BrokerReceptionProvenanceV1, BrokerStateV1,
        MAX_BROKER_CHECKPOINT_JOBS, format_time, parse_time, registry_uuid, revision,
    },
    redaction::Redactor,
};

/// Wire profile of this page, distinct from the legacy registry page (the
/// API seals its cursor under the same profile name).
const OWNED_PROFILE: &str = "human_broker_registry_owned_v1";
const SOURCE: &str = "tenant_broker_registry";
const CONSISTENCY: &str = "live_keyset";
const RUNTIME_INVENTORY: &str = "unknown";
/// Items describe this page only, never a complete tenant count.
const COUNT_SCOPE: &str = "page";

/// Checkpoint state of one broker in the owned projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OwnedHumanCheckpointStatusV1 {
    /// A durable publication was found; its freshness is reported separately.
    Received,
    /// The broker's complete operation history holds no publication.
    Absent,
    /// The bounded lookup ended first: unknown, never zero activity.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedHumanBrokerGrantV1 {
    pub(crate) revision: String,
    pub(crate) revoked: bool,
    pub(crate) granted_at: String,
    pub(crate) not_after: String,
    pub(crate) revoked_at: Option<String>,
    pub(crate) lease_seconds: u32,
    pub(crate) authorities: Vec<HumanBrokerAuthorityV1>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedHumanBrokerHeartbeatV1 {
    /// Count of committed heartbeat receptions for this broker (decimal).
    pub(crate) revision: String,
    /// Declared liveness whose reception was observed; `None` if never seen.
    pub(crate) last: Option<HumanBrokerHeartbeatV1>,
    /// The last reception belongs to the current grant's incarnation.
    pub(crate) current_incarnation: bool,
}

/// Declared, partial checkpoint summary. Every field but `status` is present
/// exactly when `status` is `received`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedHumanBrokerCheckpointV1 {
    pub(crate) status: OwnedHumanCheckpointStatusV1,
    pub(crate) received_at: Option<String>,
    pub(crate) lease_expires_at: Option<String>,
    pub(crate) freshness: Option<BrokerCheckpointFreshnessV1>,
    pub(crate) in_current_grant: Option<bool>,
    pub(crate) coverage: Option<BrokerCheckpointCoverageV1>,
    pub(crate) reference_count: Option<u32>,
    pub(crate) has_more: Option<bool>,
    pub(crate) publication_provenance: Option<BrokerClaimProvenanceV1>,
    pub(crate) reception_provenance: Option<BrokerReceptionProvenanceV1>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedHumanBrokerItemV1 {
    pub(crate) broker_id: String,
    pub(crate) incarnation_id: String,
    /// Grant/heartbeat state at `evaluated_at`, not a runtime inventory.
    pub(crate) state: BrokerStateV1,
    /// The control clock precedes a durable reception of this broker.
    pub(crate) clock_uncertain: bool,
    pub(crate) grant: OwnedHumanBrokerGrantV1,
    pub(crate) heartbeat: OwnedHumanBrokerHeartbeatV1,
    pub(crate) checkpoint: OwnedHumanBrokerCheckpointV1,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedHumanBrokerPageV1 {
    /// Always `human_broker_registry_owned_v1`.
    pub(crate) profile: String,
    pub(crate) version: u32,
    pub(crate) context_id: String,
    pub(crate) authorization_version: String,
    pub(crate) source: String,
    pub(crate) consistency: String,
    pub(crate) evaluated_at: String,
    pub(crate) runtime_inventory: String,
    pub(crate) count_scope: String,
    pub(crate) items: Vec<OwnedHumanBrokerItemV1>,
    pub(crate) has_more: bool,
    pub(crate) next_cursor: Option<String>,
    pub(crate) read_receipt: HumanReadReceiptV1,
}

/// Page plus the server-side keyset position the caller seals in its cursor.
pub(crate) struct OwnedHumanBrokerStorePageV1 {
    pub(crate) page: OwnedHumanBrokerPageV1,
    pub(crate) after_broker_id: Option<String>,
}

/// Heartbeat/grant clock uncertainty and checkpoint clock uncertainty are
/// independent observations; either one makes the item clock-uncertain.
fn clock_uncertain(state: BrokerStateV1, freshness: Option<BrokerCheckpointFreshnessV1>) -> bool {
    state == BrokerStateV1::ClockUncertain
        || freshness == Some(BrokerCheckpointFreshnessV1::ClockUncertain)
}

fn decimal(value: &str, zero: bool) -> anyhow::Result<()> {
    let number: u64 = value.parse()?;
    anyhow::ensure!(
        number.to_string() == value && revision(number).is_ok() && (zero || number > 0),
        "invalid owned broker decimal"
    );
    Ok(())
}

impl OwnedHumanBrokerCheckpointV1 {
    fn validate(&self) -> anyhow::Result<()> {
        let fields = [
            self.received_at.is_some(),
            self.lease_expires_at.is_some(),
            self.freshness.is_some(),
            self.in_current_grant.is_some(),
            self.coverage.is_some(),
            self.reference_count.is_some(),
            self.has_more.is_some(),
            self.publication_provenance.is_some(),
            self.reception_provenance.is_some(),
        ];
        let received = self.status == OwnedHumanCheckpointStatusV1::Received;
        anyhow::ensure!(
            fields.iter().all(|present| *present == received),
            "inconsistent owned checkpoint summary"
        );
        if let (Some(received_at), Some(expires_at)) = (&self.received_at, &self.lease_expires_at) {
            anyhow::ensure!(
                parse_time(expires_at)? >= parse_time(received_at)?,
                "invalid owned checkpoint lease"
            );
        }
        let maximum = u32::try_from(MAX_BROKER_CHECKPOINT_JOBS)?;
        anyhow::ensure!(
            self.reference_count.is_none_or(|count| count <= maximum),
            "invalid owned checkpoint reference count"
        );
        Ok(())
    }
}

impl OwnedHumanBrokerItemV1 {
    fn validate(&self) -> anyhow::Result<()> {
        registry_uuid(&self.broker_id)?;
        registry_uuid(&self.incarnation_id)?;
        let grant = &self.grant;
        decimal(&grant.revision, false)?;
        parse_time(&grant.granted_at)?;
        parse_time(&grant.not_after)?;
        if let Some(time) = &grant.revoked_at {
            parse_time(time)?;
        }
        anyhow::ensure!(
            grant.revoked == grant.revoked_at.is_some()
                && (1..=300).contains(&grant.lease_seconds)
                && (1..=16).contains(&grant.authorities.len()),
            "invalid owned broker grant summary"
        );
        for authority in &grant.authorities {
            match (&authority.authority_id, authority.authority_id_redacted) {
                (None, true) => {}
                (Some(id), false) => BrokerAuthorityV1 {
                    authority_id: id.clone(),
                    runtime_kind: authority.runtime_kind,
                }
                .validate()?,
                _ => anyhow::bail!("inconsistent owned broker authority redaction"),
            }
        }
        decimal(&self.heartbeat.revision, true)?;
        match &self.heartbeat.last {
            Some(last) => {
                registry_uuid(&last.incarnation_id)?;
                decimal(&last.sequence, false)?;
                anyhow::ensure!(
                    parse_time(&last.lease_expires_at)? >= parse_time(&last.received_at)?
                        && self.heartbeat.current_incarnation
                            == (last.incarnation_id == self.incarnation_id)
                        && self.heartbeat.revision != "0",
                    "invalid owned broker heartbeat summary"
                );
            }
            None => anyhow::ensure!(
                !self.heartbeat.current_incarnation && self.heartbeat.revision == "0",
                "invalid owned broker heartbeat summary"
            ),
        }
        self.checkpoint.validate()?;
        anyhow::ensure!(
            self.clock_uncertain == clock_uncertain(self.state, self.checkpoint.freshness),
            "inconsistent owned broker clock state"
        );
        Ok(())
    }
}

/// A declared authority label is shown only when the Store dictionary would
/// not alter it and it does not look like a path; runtime kinds stay visible.
fn authority_summary(authority: BrokerAuthorityV1, redactor: &Redactor) -> HumanBrokerAuthorityV1 {
    let masked = redactor.text(&authority.authority_id) != authority.authority_id
        || authority.authority_id.contains(['/', '\\']);
    HumanBrokerAuthorityV1 {
        authority_id: (!masked).then_some(authority.authority_id),
        runtime_kind: authority.runtime_kind,
        authority_id_redacted: masked,
    }
}

fn checkpoint_summary(
    lookup: OwnedCheckpointLookup,
) -> anyhow::Result<OwnedHumanBrokerCheckpointV1> {
    let empty = |status| OwnedHumanBrokerCheckpointV1 {
        status,
        received_at: None,
        lease_expires_at: None,
        freshness: None,
        in_current_grant: None,
        coverage: None,
        reference_count: None,
        has_more: None,
        publication_provenance: None,
        reception_provenance: None,
    };
    Ok(match lookup {
        OwnedCheckpointLookup::Absent => empty(OwnedHumanCheckpointStatusV1::Absent),
        OwnedCheckpointLookup::Unknown => empty(OwnedHumanCheckpointStatusV1::Unknown),
        OwnedCheckpointLookup::Found(observation) => {
            let observation = *observation;
            let snapshot = &observation.checkpoint.snapshot;
            OwnedHumanBrokerCheckpointV1 {
                status: OwnedHumanCheckpointStatusV1::Received,
                reference_count: Some(u32::try_from(snapshot.jobs.len())?),
                has_more: Some(snapshot.has_more),
                received_at: Some(observation.received_at),
                lease_expires_at: Some(observation.lease_expires_at),
                freshness: Some(observation.freshness),
                in_current_grant: Some(observation.in_current_grant),
                coverage: Some(observation.coverage),
                publication_provenance: Some(observation.publication_provenance),
                reception_provenance: Some(observation.reception_provenance),
            }
        }
    })
}

fn project_owned(
    db: &Connection,
    broker: BrokerV1,
    evaluated_at: &str,
    redactor: &Redactor,
    checkpoints: &mut Option<OwnedCheckpointWindow>,
) -> anyhow::Result<OwnedHumanBrokerItemV1> {
    let state = broker.view_at(evaluated_at)?.state;
    let lookup = owned_checkpoint_lookup(db, &broker, evaluated_at, checkpoints)?;
    let checkpoint = checkpoint_summary(lookup)?;
    let uncertain = clock_uncertain(state, checkpoint.freshness);
    let current_incarnation = broker
        .last_heartbeat
        .as_ref()
        .is_some_and(|heartbeat| heartbeat.incarnation_id == broker.incarnation_id);
    let item = OwnedHumanBrokerItemV1 {
        broker_id: broker.broker_id,
        incarnation_id: broker.incarnation_id,
        state,
        clock_uncertain: uncertain,
        grant: OwnedHumanBrokerGrantV1 {
            revision: broker.grant_revision.to_string(),
            revoked: broker.revoked,
            granted_at: broker.granted_at,
            not_after: broker.not_after,
            revoked_at: broker.revoked_at,
            lease_seconds: broker.lease_seconds,
            authorities: broker
                .authorities
                .into_iter()
                .map(|authority| authority_summary(authority, redactor))
                .collect(),
        },
        heartbeat: OwnedHumanBrokerHeartbeatV1 {
            revision: broker.heartbeat_revision.to_string(),
            last: broker
                .last_heartbeat
                .map(|heartbeat| HumanBrokerHeartbeatV1 {
                    incarnation_id: heartbeat.incarnation_id,
                    sequence: heartbeat.sequence.to_string(),
                    received_at: heartbeat.received_at,
                    lease_expires_at: heartbeat.lease_expires_at,
                    heartbeat_provenance: heartbeat.heartbeat_provenance,
                    reception_provenance: heartbeat.reception_provenance,
                }),
            current_incarnation,
        },
        checkpoint,
    };
    item.validate()?;
    // Identifiers and timestamps cannot be replaced by redaction markers;
    // only authority labels have an explicit redacted representation.
    let original = serde_json::to_value(&item)?;
    anyhow::ensure!(
        redactor.value(&original) == original,
        "owned broker metadata requires redaction"
    );
    Ok(item)
}

impl Store {
    /// Owned human broker page: Administration context with the
    /// `ReadBrokerRegistry` capability, revalidated in the read transaction
    /// of the caller's retained source; bounded keyset pagination; mandatory
    /// read audit before publication, even for an empty page.
    pub(crate) fn owned_human_brokers(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanBrokerFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<OwnedHumanBrokerStorePageV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit),
            "invalid owned human broker page"
        );
        if let Some(after) = after {
            registry_uuid(after)?;
        }
        self.with_modern_read(access, |tx| {
            self.owned_human_brokers_checked(tx, scope, frame, after, limit, transient)
        })
    }

    fn owned_human_brokers_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanBrokerFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<OwnedHumanBrokerStorePageV1> {
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !matches!(version, 14 | 15) || tx.source_access.is_none() {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        super::genesis(tx)?;
        super::super::identity::require_human_read(tx)?;
        let current = super::super::identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ReadBrokerRegistry)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        let redactor = self.redactor.union(transient);
        let evaluated_at = format_time(chrono::Utc::now());
        let mut items = Vec::new();
        let mut position = after.map(str::to_owned);
        let mut has_more = false;
        let mut bytes = 2usize;
        // The tenant's checkpoint window is read at most once per page, only
        // for a broker whose own recent operations hold no publication.
        let mut checkpoints = None;
        {
            let mut statement = tx.prepare("SELECT payload_json FROM broker_registry_brokers WHERE broker_id>?1 ORDER BY broker_id LIMIT ?2")?;
            let rows = statement.query_map(
                params![after.unwrap_or(""), i64::try_from(limit + 1)?],
                |row| row.get::<_, String>(0),
            )?;
            for row in rows {
                if items.len() == limit {
                    has_more = true;
                    break;
                }
                let broker: BrokerV1 = serde_json::from_str(&row?)?;
                let item = project_owned(tx, broker, &evaluated_at, &redactor, &mut checkpoints)?;
                let size = serde_json::to_vec(&item)?
                    .len()
                    .checked_add(1)
                    .context("owned human broker page size overflow")?;
                if bytes
                    .checked_add(size)
                    .is_none_or(|total| total > HUMAN_MAX_PAGE_BYTES - HUMAN_PAGE_ENVELOPE_RESERVE)
                {
                    anyhow::ensure!(!items.is_empty(), "broker metadata exceeds page byte limit");
                    has_more = true;
                    break;
                }
                bytes += size;
                position = Some(item.broker_id.clone());
                items.push(item);
            }
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let record = super::HumanBrokerReadAuditV1 {
            version: 1,
            request_id: request_id.clone(),
            principal: current.principal().clone(),
            membership_id: current.membership_id().into(),
            membership_revision: current.membership_revision(),
            mode: current.mode(),
            authorization_revision: super::super::identity::authorization_revision(tx)?,
            result_count: items.len(),
        };
        super::validate_read(&record)?;
        let mut event = Event::new(super::READ_KIND);
        event.data = serde_json::to_value(record)?;
        let original = serde_json::to_value(&event)?;
        anyhow::ensure!(
            redactor.value(&original) == original,
            "owned human broker audit identity requires redaction"
        );
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "owned human broker audit identity requires redaction"
        );
        let stored = self.append_human_in_transaction(tx, event, payload, current.principal())?;
        let page = OwnedHumanBrokerPageV1 {
            profile: OWNED_PROFILE.into(),
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            source: SOURCE.into(),
            consistency: CONSISTENCY.into(),
            evaluated_at,
            runtime_inventory: RUNTIME_INVENTORY.into(),
            count_scope: COUNT_SCOPE.into(),
            items,
            has_more,
            next_cursor: has_more.then(|| "0".repeat(HUMAN_CURSOR_HEX_BYTES)),
            read_receipt: HumanReadReceiptV1 {
                request_id,
                audit_event_id: stored.event.id,
                received_at: stored.received_at,
            },
        };
        // Validate fixed metadata, anchors, rows and receipt with the
        // exact-size cursor placeholder before the read receipt commits.
        self.check_owned_human_broker_output(transient, &page)?;
        Ok(OwnedHumanBrokerStorePageV1 {
            page,
            after_broker_id: position,
        })
    }

    /// The same Store dictionary guards the transaction's placeholder frame
    /// and the final sealed response. No extra audit is made here.
    pub(crate) fn check_owned_human_broker_output(
        &self,
        transient: &Redactor,
        page: &OwnedHumanBrokerPageV1,
    ) -> anyhow::Result<usize> {
        HumanBrokerFrameV1::new(&page.context_id, &page.authorization_version)?;
        anyhow::ensure!(
            page.profile == OWNED_PROFILE
                && page.version == 1
                && page.source == SOURCE
                && page.consistency == CONSISTENCY
                && page.runtime_inventory == RUNTIME_INVENTORY
                && page.count_scope == COUNT_SCOPE
                && page.items.len() <= HUMAN_MAX_PAGE_ITEMS
                && page.has_more == page.next_cursor.is_some(),
            "invalid owned human broker frame"
        );
        if let Some(cursor) = &page.next_cursor {
            anyhow::ensure!(
                cursor.len() == HUMAN_CURSOR_HEX_BYTES
                    && cursor
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid owned human broker cursor frame"
            );
        }
        parse_time(&page.evaluated_at)?;
        registry_uuid(&page.read_receipt.request_id)?;
        registry_uuid(&page.read_receipt.audit_event_id)?;
        parse_time(&page.read_receipt.received_at)?;
        for item in &page.items {
            item.validate()?;
        }
        let value = serde_json::to_value(page)?;
        anyhow::ensure!(
            self.redactor.union(transient).value(&value) == value,
            "owned human broker frame requires redaction"
        );
        let size = serde_json::to_vec(page)?.len();
        anyhow::ensure!(
            size <= HUMAN_MAX_PAGE_BYTES,
            "owned human broker frame exceeds page byte limit"
        );
        Ok(size)
    }
}
