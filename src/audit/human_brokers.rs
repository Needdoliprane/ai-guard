//! Closed schema7 capability retained by schema8; atomic, budgeted registry reads.
use super::*;
use crate::{broker_registry::BrokerV1, identity::*};

pub(super) const GENESIS_KIND: &str = "control.identity.brokers_access.genesis";
pub(super) const READ_KIND: &str = "control.identity.brokers.access";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    protocol: String,
    control_namespace_uuid: String,
    identity_genesis_seq: i64,
    identity_genesis_hash: String,
    authorization_revision: u64,
    previous_seq: i64,
    previous_hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HumanBrokerReadAuditV1 {
    pub version: u32,
    pub request_id: String,
    pub principal: PrincipalRefV1,
    pub membership_id: String,
    pub membership_revision: u64,
    pub mode: ContextModeV1,
    pub authorization_revision: u64,
    pub result_count: usize,
}
pub(super) fn validate_read(record: &HumanBrokerReadAuditV1) -> anyhow::Result<()> {
    record.principal.validate()?;
    crate::broker_registry::registry_uuid(&record.request_id)?;
    crate::broker_registry::registry_uuid(&record.membership_id)?;
    anyhow::ensure!(
        record.version == 1
            && record.principal.is_human()
            && record.mode == ContextModeV1::Administration
            && record.membership_revision > 0
            && i64::try_from(record.membership_revision).is_ok()
            && i64::try_from(record.authorization_revision).is_ok()
            && record.result_count <= HUMAN_MAX_PAGE_ITEMS,
        "invalid human broker read audit"
    );
    Ok(())
}
pub(super) fn is_human_read(kind: &str) -> bool {
    matches!(kind, "control.identity.access" | READ_KIND)
}
/// Classification depends on the authenticated event kind and validated closed
/// payload. JSON cannot nominate an audit capacity class or reserve.
pub(super) fn read_principal(event: &Event) -> anyhow::Result<PrincipalRefV1> {
    match event.kind.as_str() {
        READ_KIND => {
            let record: HumanBrokerReadAuditV1 = serde_json::from_value(event.data.clone())?;
            validate_read(&record)?;
            Ok(record.principal)
        }
        "control.identity.access" => {
            let record: human::HumanReadAuditV1 = serde_json::from_value(event.data.clone())?;
            human::validate_read(&record)?;
            Ok(record.principal)
        }
        _ => anyhow::bail!("not a human read event"),
    }
}
pub(super) fn require_mutation_capability(
    input: &IdentityMutationV1,
    enabled: bool,
) -> anyhow::Result<()> {
    if !enabled
        && matches!(&input.change, IdentityChangeV1::Membership { membership, .. } if membership.capabilities.contains(&HumanCapabilityV1::ReadBrokerRegistry))
    {
        return Err(IdentityFailureV1::MigrationRequired.into());
    }
    Ok(())
}
pub(super) fn genesis(db: &Connection) -> anyhow::Result<(StoredEvent, u64)> {
    let records = {
        let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2")?;
        statement
            .query_map([GENESIS_KIND], row_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    anyhow::ensure!(
        records.len() == 1,
        "invalid human broker capability marker count"
    );
    let stored = records
        .into_iter()
        .next()
        .context("missing human broker capability marker")?;
    let marker: Genesis = serde_json::from_value(stored.event.data.clone())?;
    let (identity_seq, identity_hash): (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM identity_meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let last_capability: i64 = db.query_row("SELECT MAX(seq) FROM events WHERE kind IN ('control.identity.human_access.genesis','control.workload.genesis','control.broker_registry.genesis','control.relay.genesis')", [], |r| r.get(0))?;
    anyhow::ensure!(
        marker.version == 7
            && marker.protocol == "human_broker_registry_read_v1"
            && marker.control_namespace_uuid == namespace
            && marker.identity_genesis_seq == identity_seq
            && marker.identity_genesis_hash == identity_hash
            && stored.seq > identity_seq
            && stored.seq > last_capability
            && marker.previous_seq == stored.seq - 1
            && marker.previous_hash == stored.prev_hash
            && i64::try_from(marker.authorization_revision).is_ok()
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "observed",
        "invalid human broker capability marker"
    );
    Ok((stored, marker.authorization_revision))
}
pub(super) fn validate_capability(db: &Connection, version: i64) -> anyhow::Result<()> {
    if [7, 8].contains(&version) {
        genesis(db)?;
    } else {
        let forbidden: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE kind IN (?1,?2))",
            params![GENESIS_KIND, READ_KIND],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            !forbidden,
            "human broker capability requires schema7 or schema8"
        );
    }
    Ok(())
}
pub(crate) struct HumanBrokerStorePageV1 {
    pub page: HumanBrokerRegistryPageV1,
    pub after_broker_id: Option<String>,
}
/// Immutable response anchors supplied only by the selected server context.
/// This carries no registry handle, bearer or authorization authority.
pub(crate) struct HumanBrokerFrameV1 {
    context_id: String,
    authorization_version: String,
}
impl HumanBrokerFrameV1 {
    pub(crate) fn new(context_id: &str, authorization_version: &str) -> anyhow::Result<Self> {
        for value in [context_id, authorization_version] {
            anyhow::ensure!(
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human broker frame anchor"
            );
        }
        Ok(Self {
            context_id: context_id.into(),
            authorization_version: authorization_version.into(),
        })
    }
}
impl Store {
    pub(super) fn initialize_human_brokers(&self, tx: &Transaction<'_>) -> anyhow::Result<()> {
        let (identity_genesis_seq, identity_genesis_hash): (i64, String) = tx.query_row(
            "SELECT genesis_seq,genesis_hash FROM identity_meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let control_namespace_uuid = tx.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let (previous_seq, previous_hash) = audit_head(tx)?;
        let mut event = Event::new(GENESIS_KIND);
        event.data = serde_json::to_value(Genesis {
            version: 7,
            protocol: "human_broker_registry_read_v1".into(),
            control_namespace_uuid,
            identity_genesis_seq,
            identity_genesis_hash,
            authorization_revision: identity::authorization_revision(tx)?,
            previous_seq,
            previous_hash,
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "human broker capability requires redaction"
        );
        self.append_identity_in_transaction(tx, event, payload)?;
        Ok(())
    }
    pub(crate) fn human_brokers(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanBrokerFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanBrokerStorePageV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit),
            "invalid human broker page"
        );
        if let Some(after) = after {
            crate::broker_registry::registry_uuid(after)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if ![7, 8].contains(&version) {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        let current = identity::revalidate_scope(&tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ReadBrokerRegistry)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        let redactor = self.redactor.union(transient);
        let evaluated_at = crate::broker_registry::format_time(chrono::Utc::now());
        let mut items = Vec::new();
        let mut position = after.map(str::to_owned);
        let mut has_more = false;
        let mut bytes = 2usize;
        {
            let mut statement = tx.prepare("SELECT payload_json FROM broker_registry_brokers WHERE broker_id>?1 ORDER BY broker_id LIMIT ?2")?;
            let rows = statement.query_map(
                params![after.unwrap_or(""), i64::try_from(limit + 1)?],
                |r| r.get::<_, String>(0),
            )?;
            for row in rows {
                if items.len() == limit {
                    has_more = true;
                    break;
                }
                let broker: BrokerV1 = serde_json::from_str(&row?)?;
                let item = project(broker, &evaluated_at, &redactor)?;
                let size = serde_json::to_vec(&item)?
                    .len()
                    .checked_add(1)
                    .context("human broker page size overflow")?;
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
        let record = HumanBrokerReadAuditV1 {
            version: 1,
            request_id: request_id.clone(),
            principal: current.principal().clone(),
            membership_id: current.membership_id().into(),
            membership_revision: current.membership_revision(),
            mode: current.mode(),
            authorization_revision: identity::authorization_revision(&tx)?,
            result_count: items.len(),
        };
        validate_read(&record)?;
        let mut event = Event::new(READ_KIND);
        event.data = serde_json::to_value(record)?;
        let original = serde_json::to_value(&event)?;
        anyhow::ensure!(
            redactor.value(&original) == original,
            "human broker audit identity requires redaction"
        );
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "human broker audit identity requires redaction"
        );
        let stored = self.append_human_in_transaction(&tx, event, payload, current.principal())?;
        let receipt = HumanReadReceiptV1 {
            request_id,
            audit_event_id: stored.event.id,
            received_at: stored.received_at,
        };
        let page = HumanBrokerRegistryPageV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            source: "tenant_broker_registry".into(),
            consistency: "live_keyset".into(),
            evaluated_at,
            runtime_inventory: "unknown".into(),
            items,
            has_more,
            next_cursor: has_more.then(|| "0".repeat(HUMAN_CURSOR_HEX_BYTES)),
            read_receipt: receipt,
        };
        // Validate keys, fixed metadata, anchors, rows and receipt together with
        // the exact-size cursor placeholder before committing the read receipt.
        self.check_human_broker_output(transient, &page)?;
        self.commit_checked(tx)?;
        Ok(HumanBrokerStorePageV1 {
            page,
            after_broker_id: position,
        })
    }
    /// The same immutable Store dictionary guards both the transaction's full
    /// placeholder frame and the final sealed response. No extra audit is made.
    pub(crate) fn check_human_broker_output(
        &self,
        transient: &Redactor,
        page: &HumanBrokerRegistryPageV1,
    ) -> anyhow::Result<usize> {
        HumanBrokerFrameV1::new(&page.context_id, &page.authorization_version)?;
        anyhow::ensure!(
            page.version == 1
                && page.source == "tenant_broker_registry"
                && page.consistency == "live_keyset"
                && page.runtime_inventory == "unknown"
                && page.items.len() <= HUMAN_MAX_PAGE_ITEMS
                && page.has_more == page.next_cursor.is_some(),
            "invalid human broker frame"
        );
        if let Some(cursor) = &page.next_cursor {
            anyhow::ensure!(
                cursor.len() == HUMAN_CURSOR_HEX_BYTES
                    && cursor
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human broker cursor frame"
            );
        }
        crate::broker_registry::parse_time(&page.evaluated_at)?;
        crate::broker_registry::registry_uuid(&page.read_receipt.request_id)?;
        crate::broker_registry::registry_uuid(&page.read_receipt.audit_event_id)?;
        crate::broker_registry::parse_time(&page.read_receipt.received_at)?;
        for item in &page.items {
            item.validate()?;
        }
        let value = serde_json::to_value(page)?;
        anyhow::ensure!(
            self.redactor.union(transient).value(&value) == value,
            "human broker frame requires redaction"
        );
        let size = serde_json::to_vec(page)?.len();
        anyhow::ensure!(
            size <= HUMAN_MAX_PAGE_BYTES,
            "human broker frame exceeds page byte limit"
        );
        Ok(size)
    }
}
fn project(broker: BrokerV1, now: &str, redactor: &Redactor) -> anyhow::Result<HumanBrokerItemV1> {
    let state = broker.view_at(now)?.state;
    let authorities = broker
        .authorities
        .into_iter()
        .map(|authority| {
            let masked = redactor.text(&authority.authority_id) != authority.authority_id;
            HumanBrokerAuthorityV1 {
                authority_id: (!masked).then_some(authority.authority_id),
                runtime_kind: authority.runtime_kind,
                authority_id_redacted: masked,
            }
        })
        .collect();
    let item = HumanBrokerItemV1 {
        broker_id: broker.broker_id,
        incarnation_id: broker.incarnation_id,
        grant_revision: broker.grant_revision.to_string(),
        heartbeat_revision: broker.heartbeat_revision.to_string(),
        state,
        authorities,
        granted_at: broker.granted_at,
        not_after: broker.not_after,
        revoked_at: broker.revoked_at,
        last_heartbeat: broker
            .last_heartbeat
            .map(|heartbeat| HumanBrokerHeartbeatV1 {
                incarnation_id: heartbeat.incarnation_id,
                sequence: heartbeat.sequence.to_string(),
                received_at: heartbeat.received_at,
                lease_expires_at: heartbeat.lease_expires_at,
                heartbeat_provenance: heartbeat.heartbeat_provenance,
                reception_provenance: heartbeat.reception_provenance,
            }),
    };
    item.validate()?;
    // Required identifiers/timestamps cannot be replaced by redaction markers.
    // Only authority IDs have an explicit null/redacted representation.
    let original = serde_json::to_value(&item)?;
    anyhow::ensure!(
        redactor.value(&original) == original,
        "human broker metadata requires redaction"
    );
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker_registry::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn human_projection_preserves_exact_large_decimals_all_lease_states_and_old_incarnation() {
        let mut broker = BrokerV1 {
            version: 1,
            broker_id: uuid::Uuid::new_v4().to_string(),
            grant_revision: i64::MAX as u64,
            heartbeat_revision: i64::MAX as u64,
            credential_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "declared-kubernetes-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 30,
            not_after: "2026-11-04T12:01:00.000Z".into(),
            revoked: false,
            granted_at: "2026-11-04T12:00:00.000Z".into(),
            revoked_at: None,
            last_heartbeat: None,
        };
        let redactor = Redactor::new(vec![]);
        assert_eq!(
            project(broker.clone(), "2026-11-04T12:00:15.000Z", &redactor)
                .unwrap()
                .state,
            BrokerStateV1::Unseen
        );
        broker.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
            incarnation_id: broker.incarnation_id.clone(),
            sequence: i64::MAX as u64,
            received_at: "2026-11-04T12:00:05.000Z".into(),
            lease_expires_at: "2026-11-04T12:00:35.000Z".into(),
            heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
            reception_provenance: BrokerReceptionProvenanceV1::Observed,
        });
        for (now, state) in [
            ("2026-11-04T11:59:59.000Z", BrokerStateV1::ClockUncertain),
            ("2026-11-04T12:00:15.000Z", BrokerStateV1::Current),
            ("2026-11-04T12:00:40.000Z", BrokerStateV1::Stale),
            ("2026-11-04T12:01:00.000Z", BrokerStateV1::Expired),
        ] {
            let item = project(broker.clone(), now, &redactor).unwrap();
            assert_eq!(item.state, state);
            assert_eq!(item.grant_revision, i64::MAX.to_string());
            assert_eq!(item.heartbeat_revision, i64::MAX.to_string());
            assert_eq!(item.last_heartbeat.unwrap().sequence, i64::MAX.to_string());
        }
        let old = broker.incarnation_id.clone();
        broker.incarnation_id = uuid::Uuid::new_v4().to_string();
        let item = project(broker.clone(), "2026-11-04T12:00:15.000Z", &redactor).unwrap();
        assert_eq!(item.state, BrokerStateV1::Unseen);
        assert_eq!(item.last_heartbeat.unwrap().incarnation_id, old);
        broker.revoked = true;
        broker.revoked_at = Some("2026-11-04T12:00:10.000Z".into());
        assert_eq!(
            project(broker, "2026-11-04T12:00:15.000Z", &redactor)
                .unwrap()
                .state,
            BrokerStateV1::Revoked
        );
    }

    #[test]
    fn sqlite_full_during_required_read_audit_returns_no_page_and_keeps_last_durable_head() {
        let files = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = files.path().join("audit.sqlite");
        let store =
            Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
        store
            .migrate_control_v3(&path.with_extension("before3"))
            .unwrap();
        store
            .migrate_identity_v4(&path.with_extension("before4"))
            .unwrap();
        store
            .migrate_human_access_v5(&path.with_extension("before5"))
            .unwrap();
        store
            .migrate_workload_v6(&path.with_extension("before6"))
            .unwrap();
        store
            .migrate_human_brokers_v7(&path.with_extension("before7"))
            .unwrap();
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: uuid::Uuid::new_v4().to_string(),
            account_id: uuid::Uuid::new_v4().to_string(),
        };
        let member = MembershipV1 {
            version: 1,
            membership_id: uuid::Uuid::new_v4().to_string(),
            principal: principal.clone(),
            tenant_id: "tenant-a".into(),
            organization_scope: "organization-a".into(),
            revision: 1,
            active: true,
            capabilities: std::collections::BTreeSet::from([HumanCapabilityV1::ReadBrokerRegistry]),
        };
        for change in [
            IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Named SQLite reader".into(),
                    revision: 1,
                    active: true,
                },
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: member.clone(),
            },
        ] {
            store
                .mutate_identity(&IdentityMutationV1 {
                    version: 1,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    actor: PrincipalRefV1::LegacyControl {
                        credential_id: "fixture-operator".into(),
                        role: LegacyControlRoleV1::Runner,
                    },
                    change,
                })
                .unwrap();
        }
        let scope = store
            .authorize_human_context(
                &principal,
                &member.membership_id,
                1,
                ContextModeV1::Administration,
            )
            .unwrap();
        {
            let db = store.db.lock().unwrap();
            let pages: i64 = db
                .pragma_query_value(None, "page_count", |r| r.get(0))
                .unwrap();
            db.pragma_update(None, "max_page_count", pages + 1).unwrap();
        }
        let frame = HumanBrokerFrameV1::new(&"a".repeat(64), &"b".repeat(64)).unwrap();
        let mut observed_full = false;
        for _ in 0..128 {
            let before = store.head().unwrap();
            match store.human_brokers(&scope, &frame, None, 25, &Redactor::new(vec![])) {
                Ok(page) => {
                    assert!(page.page.items.is_empty());
                    assert_eq!(store.head().unwrap(), before + 1);
                }
                Err(error) => {
                    assert!(
                        matches!(error.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(code,_)) if code.code == rusqlite::ErrorCode::DiskFull),
                        "expected actual SQLite FULL, got {error}"
                    );
                    assert_eq!(store.head().unwrap(), before);
                    observed_full = true;
                    break;
                }
            }
        }
        assert!(
            observed_full,
            "bounded SQLite page cap did not reach a real allocation refusal"
        );
        assert!(store.integrity().unwrap().valid);
    }
}
