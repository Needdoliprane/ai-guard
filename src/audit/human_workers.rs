//! Explicit schema10 human worker metadata; one authorization/projection/audit transaction.
use super::*;
use crate::{control::WorkerV1, identity::*};

pub(super) const GENESIS_KIND: &str = "control.identity.workers_access.genesis";
pub(super) const READ_KIND: &str = "control.identity.workers.access";
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
pub(super) struct HumanWorkerReadAuditV1 {
    pub version: u32,
    pub request_id: String,
    pub principal: PrincipalRefV1,
    pub membership_id: String,
    pub membership_revision: u64,
    pub mode: ContextModeV1,
    pub authorization_revision: u64,
    pub result_count: usize,
}
pub(super) fn validate_read(record: &HumanWorkerReadAuditV1) -> anyhow::Result<()> {
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
        "invalid human worker read audit"
    );
    Ok(())
}
pub(super) fn require_mutation_capability(
    input: &IdentityMutationV1,
    enabled: bool,
) -> anyhow::Result<()> {
    if !enabled
        && matches!(&input.change, IdentityChangeV1::Membership { membership, .. } if membership.capabilities.contains(&HumanCapabilityV1::ReadWorkerRegistry))
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
        "invalid human worker capability marker count"
    );
    let stored = records
        .into_iter()
        .next()
        .context("missing human worker capability marker")?;
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
    let last_capability: i64 = db.query_row("SELECT MAX(seq) FROM events WHERE kind IN ('control.identity.human_access.genesis','control.workload.genesis','control.broker_registry.genesis','control.relay.genesis','control.identity.brokers_access.genesis','control.kubernetes.create.policy.genesis')", [], |r| r.get(0))?;
    anyhow::ensure!(
        marker.version == 10
            && marker.protocol == "human_worker_registry_read_v1"
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
        "invalid human worker capability marker"
    );
    Ok((stored, marker.authorization_revision))
}
pub(super) fn validate_capability(db: &Connection, version: i64) -> anyhow::Result<()> {
    if matches!(version, 10 | 11 | 12 | 13) {
        genesis(db)?;
    } else {
        let forbidden: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE kind IN (?1,?2))",
            params![GENESIS_KIND, READ_KIND],
            |r| r.get(0),
        )?;
        anyhow::ensure!(!forbidden, "human worker capability requires schema10");
    }
    Ok(())
}
pub(crate) struct HumanWorkerStorePageV1 {
    pub page: HumanWorkerRegistryPageV1,
    pub after_worker_id: Option<String>,
}
/// Immutable response anchors supplied only by the selected server context.
/// This carries no registry handle, bearer or authorization authority.
pub(crate) struct HumanWorkerFrameV1 {
    context_id: String,
    authorization_version: String,
}
impl HumanWorkerFrameV1 {
    pub(crate) fn new(context_id: &str, authorization_version: &str) -> anyhow::Result<Self> {
        for value in [context_id, authorization_version] {
            anyhow::ensure!(
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human worker frame anchor"
            );
        }
        Ok(Self {
            context_id: context_id.into(),
            authorization_version: authorization_version.into(),
        })
    }
}
impl Store {
    pub(super) fn initialize_human_workers(&self, tx: &Transaction<'_>) -> anyhow::Result<()> {
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
            version: 10,
            protocol: "human_worker_registry_read_v1".into(),
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
            "human worker capability requires redaction"
        );
        self.append_identity_in_transaction(tx, event, payload)?;
        Ok(())
    }
    pub(crate) fn human_workers(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanWorkerFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanWorkerStorePageV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit),
            "invalid human worker page"
        );
        if let Some(after) = after {
            crate::control::uuid(after)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let current = identity::revalidate_scope(&tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ReadWorkerRegistry)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        if !matches!(version, 10 | 11 | 12 | 13) {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        let redactor = self.redactor.union(transient);
        let evaluated_at = crate::broker_registry::format_time(chrono::Utc::now());
        let mut items = Vec::new();
        let mut position = after.map(str::to_owned);
        let mut has_more = false;
        let mut bytes = 2usize;
        {
            let mut statement = tx.prepare(
                "SELECT snapshot_json FROM workers WHERE worker_id>?1 ORDER BY worker_id LIMIT ?2",
            )?;
            let rows = statement.query_map(
                params![after.unwrap_or(""), i64::try_from(limit + 1)?],
                |r| r.get::<_, String>(0),
            )?;
            for row in rows {
                if items.len() == limit {
                    has_more = true;
                    break;
                }
                let worker: WorkerV1 = serde_json::from_str(&row?)?;
                let item = project(&tx, worker, &evaluated_at, &redactor)?;
                let size = serde_json::to_vec(&item)?
                    .len()
                    .checked_add(1)
                    .context("human worker page size overflow")?;
                if bytes
                    .checked_add(size)
                    .is_none_or(|total| total > HUMAN_MAX_PAGE_BYTES - HUMAN_PAGE_ENVELOPE_RESERVE)
                {
                    anyhow::ensure!(!items.is_empty(), "worker metadata exceeds page byte limit");
                    has_more = true;
                    break;
                }
                bytes += size;
                position = Some(item.worker_id.clone());
                items.push(item);
            }
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let record = HumanWorkerReadAuditV1 {
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
            "human worker audit identity requires redaction"
        );
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "human worker audit identity requires redaction"
        );
        let stored = self.append_human_in_transaction(&tx, event, payload, current.principal())?;
        let receipt = HumanReadReceiptV1 {
            request_id,
            audit_event_id: stored.event.id,
            received_at: stored.received_at,
        };
        let page = HumanWorkerRegistryPageV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            source: "tenant_worker_broker_registry".into(),
            consistency: "snapshot_per_page".into(),
            evaluated_at,
            runtime_inventory: "unknown".into(),
            registry_status: "available".into(),
            items,
            has_more,
            next_cursor: has_more.then(|| "0".repeat(HUMAN_CURSOR_HEX_BYTES)),
            read_receipt: receipt,
        };
        // Validate keys, fixed metadata, anchors, rows and receipt together with
        // the exact-size cursor placeholder before committing the read receipt.
        self.check_human_worker_output(transient, &page)?;
        self.commit_checked(tx)?;
        Ok(HumanWorkerStorePageV1 {
            page,
            after_worker_id: position,
        })
    }
    /// The same immutable Store dictionary guards both the transaction's full
    /// placeholder frame and the final sealed response. No extra audit is made.
    pub(crate) fn check_human_worker_output(
        &self,
        transient: &Redactor,
        page: &HumanWorkerRegistryPageV1,
    ) -> anyhow::Result<usize> {
        HumanWorkerFrameV1::new(&page.context_id, &page.authorization_version)?;
        anyhow::ensure!(
            page.version == 1
                && page.source == "tenant_worker_broker_registry"
                && page.consistency == "snapshot_per_page"
                && page.runtime_inventory == "unknown"
                && page.registry_status == "available"
                && page.items.len() <= HUMAN_MAX_PAGE_ITEMS
                && page.has_more == page.next_cursor.is_some(),
            "invalid human worker frame"
        );
        if let Some(cursor) = &page.next_cursor {
            anyhow::ensure!(
                cursor.len() == HUMAN_CURSOR_HEX_BYTES
                    && cursor
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human worker cursor frame"
            );
        }
        crate::broker_registry::parse_time(&page.evaluated_at)?;
        crate::broker_registry::registry_uuid(&page.read_receipt.request_id)?;
        crate::broker_registry::registry_uuid(&page.read_receipt.audit_event_id)?;
        crate::broker_registry::parse_time(&page.read_receipt.received_at)?;
        for item in &page.items {
            validate_item(item)?;
        }
        let value = serde_json::to_value(page)?;
        anyhow::ensure!(
            self.redactor.union(transient).value(&value) == value,
            "human worker frame requires redaction"
        );
        let size = serde_json::to_vec(page)?.len();
        anyhow::ensure!(
            size <= HUMAN_MAX_PAGE_BYTES,
            "human worker frame exceeds page byte limit"
        );
        Ok(size)
    }
}

fn project(
    db: &Connection,
    worker: WorkerV1,
    now: &str,
    redactor: &Redactor,
) -> anyhow::Result<HumanWorkerItemV1> {
    let launch_broker = broker_registry::federation::association(
        db,
        Some((&worker.launch.authority_id, worker.launch.runtime_kind)),
        now,
        true,
    )?;
    let observation_broker = broker_registry::federation::association(
        db,
        worker.last_observation.as_ref().map(|observation| {
            (
                observation.inspection.authority_id.as_str(),
                observation.inspection.runtime_kind,
            )
        }),
        now,
        true,
    )?;
    let (observed_started_at, observed_started_at_status) =
        project_runtime_time(worker.observed_started_at.as_deref(), true);
    let last_observation = worker
        .last_observation
        .map(|observation| -> anyhow::Result<_> {
            // Observe already validates this control-observation timestamp as RFC3339,
            // without the broker lease clock's positive-epoch restriction.
            let (observed_at, observed_at_status) =
                project_observed_time(&observation.observed_at)?;
            let (started_at, started_at_status) =
                project_runtime_time(observation.started_at.as_deref(), true);
            let (finished_at, finished_at_status) =
                project_runtime_time(observation.finished_at.as_deref(), false);
            Ok(HumanWorkerObservationV1 {
                observed_at,
                observed_at_status,
                status: observation.status,
                running: observation.running,
                started_at,
                started_at_status,
                finished_at,
                finished_at_status,
                exit_code: observation.exit_code.map(|value| value.to_string()),
                oom_killed: observation.oom_killed,
                restart_count: observation.restart_count,
                provenance: "observed".into(),
            })
        })
        .transpose()?;
    let item = HumanWorkerItemV1 {
        worker_id: worker.worker_id,
        request_id: worker.request_id,
        attempt_id: worker.attempt.attempt_id,
        revision: worker.revision.to_string(),
        state: worker.state,
        quota_held: worker.quota_held,
        session_id: worker.launch.session_id,
        session_provenance: "declared".into(),
        runtime_kind: worker.launch.runtime_kind,
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes.to_string(),
        pids_limit: worker.launch.pids_limit,
        timeout_seconds: worker.launch.timeout_seconds,
        network: worker.launch.network,
        observed_started_at,
        observed_started_at_status,
        allocation_inconsistent: worker.allocation_inconsistent,
        cleanup_removed: worker.cleanup_removed,
        last_observation,
        launch_broker,
        observation_broker,
    };
    validate_item(&item)?;
    let value = serde_json::to_value(&item)?;
    anyhow::ensure!(
        redactor.value(&value) == value,
        "human worker metadata requires redaction"
    );
    Ok(item)
}
fn validate_item(item: &HumanWorkerItemV1) -> anyhow::Result<()> {
    for id in [
        &item.worker_id,
        &item.request_id,
        &item.attempt_id,
        &item.session_id,
    ] {
        crate::control::uuid(id)?;
    }
    let revision: u64 = item.revision.parse()?;
    let memory: u64 = item.memory_bytes.parse()?;
    anyhow::ensure!(
        revision > 0
            && revision <= i64::MAX as u64
            && revision.to_string() == item.revision
            && memory > 0
            && memory <= 100 * 1024 * 1024 * 1024
            && memory.to_string() == item.memory_bytes
            && item.cpu_millis > 0
            && item.cpu_millis <= 200_000
            && item.pids_limit > 0
            && item.timeout_seconds > 0
            && item.timeout_seconds <= 86400
            && item.session_provenance == "declared",
        "invalid human worker metadata"
    );
    validate_time_pair(
        item.observed_started_at.as_deref(),
        item.observed_started_at_status,
        true,
    )?;
    if let Some(observation) = &item.last_observation {
        anyhow::ensure!(
            observation.provenance == "observed"
                && observation.observed_at_status != HumanWorkerTimeStatusV1::Absent,
            "invalid human worker observation"
        );
        validate_time_pair(
            observation.observed_at.as_deref(),
            observation.observed_at_status,
            false,
        )?;
        validate_time_pair(
            observation.started_at.as_deref(),
            observation.started_at_status,
            true,
        )?;
        validate_time_pair(
            observation.finished_at.as_deref(),
            observation.finished_at_status,
            false,
        )?;
        if let Some(exit_code) = &observation.exit_code {
            let value: i64 = exit_code.parse()?;
            anyhow::ensure!(
                value.to_string() == *exit_code,
                "invalid human worker exit code"
            );
        }
    }
    Ok(())
}

/// Publish only RFC3339's four-digit UTC year domain. Conversion must roundtrip
/// the same instant and nanoseconds; an otherwise valid source is never clipped.
fn canonical_utc(source: &chrono::DateTime<chrono::FixedOffset>) -> Option<String> {
    let utc = source.with_timezone(&chrono::Utc);
    let rendered = utc.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let (year, _) = rendered.split_once('-')?;
    if year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let roundtrip = chrono::DateTime::parse_from_rfc3339(&rendered).ok()?;
    (roundtrip.with_timezone(&chrono::Utc) == utc
        && roundtrip.timestamp_subsec_nanos() == utc.timestamp_subsec_nanos())
    .then_some(rendered)
}
/// The source field is mandatory. Invalid raw RFC3339 remains a storage error;
/// only a valid source whose UTC representation is outside the published domain
/// becomes an explicit unknown value.
fn project_observed_time(raw: &str) -> anyhow::Result<(Option<String>, HumanWorkerTimeStatusV1)> {
    let source = chrono::DateTime::parse_from_rfc3339(raw)
        .context("invalid persisted worker observation time")?;
    Ok(match canonical_utc(&source) {
        Some(value) => (Some(value), HumanWorkerTimeStatusV1::Known),
        None => (None, HumanWorkerTimeStatusV1::Unknown),
    })
}
/// Keep validity separate from observation provenance. The historical start
/// classifier treats nonpositive runtime starts as unset/unobserved sentinels;
/// the control observation and finish timestamps have no separate epoch gate.
fn project_runtime_time(
    raw: Option<&str>,
    credible_start: bool,
) -> (Option<String>, HumanWorkerTimeStatusV1) {
    let Some(raw) = raw else {
        return (None, HumanWorkerTimeStatusV1::Absent);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(raw).ok();
    match parsed
        .filter(|time| !credible_start || time.timestamp() > 0)
        .and_then(|time| canonical_utc(&time))
    {
        Some(value) => (Some(value), HumanWorkerTimeStatusV1::Known),
        None => (None, HumanWorkerTimeStatusV1::Unknown),
    }
}
fn validate_time_pair(
    value: Option<&str>,
    status: HumanWorkerTimeStatusV1,
    credible_start: bool,
) -> anyhow::Result<()> {
    match status {
        HumanWorkerTimeStatusV1::Known => {
            let value = value.context("known worker time is missing")?;
            anyhow::ensure!(
                project_runtime_time(Some(value), credible_start)
                    == (Some(value.into()), HumanWorkerTimeStatusV1::Known),
                "worker time is not canonical UTC or contradicts the historical start interpretation"
            );
        }
        HumanWorkerTimeStatusV1::Absent | HumanWorkerTimeStatusV1::Unknown => {
            anyhow::ensure!(
                value.is_none(),
                "unknown or absent worker time carries a value"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
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
        store
            .migrate_kubernetes_create_v8(&path.with_extension("before8"))
            .unwrap();
        store
            .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
            .unwrap();
        store
            .migrate_human_workers_v10(&path.with_extension("before10"))
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
            capabilities: std::collections::BTreeSet::from([HumanCapabilityV1::ReadWorkerRegistry]),
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
        let frame = HumanWorkerFrameV1::new(&"a".repeat(64), &"b".repeat(64)).unwrap();
        let mut observed_full = false;
        for _ in 0..128 {
            let before = store.head().unwrap();
            match store.human_workers(&scope, &frame, None, 25, &Redactor::new(vec![])) {
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

    #[test]
    fn runtime_time_status_preserves_historical_start_and_finish_interpretation() {
        use HumanWorkerTimeStatusV1::{Absent, Known, Unknown};
        assert_eq!(project_runtime_time(None, true), (None, Absent));
        assert_eq!(
            project_runtime_time(Some("unavailable"), true),
            (None, Unknown)
        );
        assert_eq!(
            project_runtime_time(Some("1970-01-01T00:00:00Z"), true),
            (None, Unknown)
        );
        assert_eq!(
            project_runtime_time(Some("1970-01-01T00:00:00Z"), false),
            (Some("1970-01-01T00:00:00.000000000Z".into()), Known)
        );
        assert_eq!(
            project_observed_time("1970-01-01T01:00:00+01:00")
                .unwrap()
                .0
                .as_deref(),
            Some("1970-01-01T00:00:00.000000000Z")
        );
        assert_eq!(
            project_runtime_time(Some("2026-10-29T11:00:00.000000001+01:00"), true),
            (Some("2026-10-29T10:00:00.000000001Z".into()), Known)
        );
    }

    #[test]
    fn a_more_frame_with_nil_legacy_worker_ids_is_valid_without_widening_broker_ids() {
        let files = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Store::open_for_tenant(
            &files.path().join("frame.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap();
        let nil = uuid::Uuid::nil().to_string();
        let association = || crate::broker_registry::AuthorityBrokerAssociationV1 {
            status: crate::broker_registry::AuthorityBrokerStatusV1::AuthorityUnknown,
            broker: None,
            runtime_kind: None,
            runtime_matches: None,
            in_current_grant: None,
        };
        let item = HumanWorkerItemV1 {
            worker_id: nil.clone(),
            request_id: nil.clone(),
            attempt_id: nil.clone(),
            revision: "1".into(),
            state: crate::control::WorkerStateV1::Reserved,
            quota_held: true,
            session_id: nil.clone(),
            session_provenance: "declared".into(),
            runtime_kind: crate::control::RuntimeKindV1::Docker,
            cpu_millis: 2000,
            memory_bytes: "1073741824".into(),
            pids_limit: 256,
            timeout_seconds: 60,
            network: crate::control::NetworkV1::None,
            observed_started_at: None,
            observed_started_at_status: HumanWorkerTimeStatusV1::Absent,
            allocation_inconsistent: false,
            cleanup_removed: None,
            last_observation: None,
            launch_broker: association(),
            observation_broker: association(),
        };
        let page = HumanWorkerRegistryPageV1 {
            version: 1,
            context_id: "a".repeat(64),
            authorization_version: "b".repeat(64),
            source: "tenant_worker_broker_registry".into(),
            consistency: "snapshot_per_page".into(),
            evaluated_at: "2026-11-06T12:00:00.000Z".into(),
            runtime_inventory: "unknown".into(),
            registry_status: "available".into(),
            items: vec![item],
            has_more: true,
            next_cursor: Some("0".repeat(HUMAN_CURSOR_HEX_BYTES)),
            read_receipt: HumanReadReceiptV1 {
                request_id: uuid::Uuid::new_v4().to_string(),
                audit_event_id: uuid::Uuid::new_v4().to_string(),
                received_at: "2026-11-06T12:00:00.000Z".into(),
            },
        };
        assert!(
            store
                .check_human_worker_output(&Redactor::new(vec![]), &page)
                .is_ok()
        );
        assert!(
            crate::broker_registry::registry_uuid(&nil).is_err(),
            "broker IDs keep their separate nonnil contract"
        );
    }

    #[test]
    fn out_of_domain_utc_observation_is_unknown_in_a_valid_human_dto() {
        let nil = uuid::Uuid::nil().to_string();
        let association = serde_json::json!({
            "status":"authority_unknown", "broker":null, "runtime_kind":null,
            "runtime_matches":null, "in_current_grant":null
        });
        let mut failures = Vec::new();
        for (label, raw) in [
            ("upper", "9999-12-31T23:59:59-01:00"),
            ("lower", "0000-01-01T00:00:00+01:00"),
        ] {
            // This is the historical Observe.observed_at admission parser.
            // Both four-digit source dates are valid; their UTC rendering
            // crosses the published four-digit RFC3339 domain.
            let source = chrono::DateTime::parse_from_rfc3339(raw)
                .expect("the historical Observe RFC3339 parser accepts the source");
            let utc = source
                .with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            let (projected, status) = project_runtime_time(Some(raw), false);
            let item: HumanWorkerItemV1 = serde_json::from_value(serde_json::json!({
                "worker_id":nil, "request_id":nil, "attempt_id":nil, "revision":"1",
                "state":"in_doubt", "quota_held":true, "session_id":nil,
                "session_provenance":"declared", "runtime_kind":"docker",
                "cpu_millis":2000, "memory_bytes":"1073741824",
                "pids_limit":256, "timeout_seconds":60, "network":"none",
                "observed_started_at":null, "observed_started_at_status":"absent",
                "allocation_inconsistent":false, "cleanup_removed":null,
                "last_observation":{
                    "observed_at":projected, "observed_at_status":status,
                    "status":"unknown", "running":null,
                    "started_at":null, "started_at_status":"absent",
                    "finished_at":null, "finished_at_status":"absent",
                    "exit_code":null, "oom_killed":null, "restart_count":null,
                    "provenance":"observed"
                },
                "launch_broker":association, "observation_broker":association
            }))
            .expect("the actual closed human DTO must represent this projected observation");
            let validation = validate_item(&item);
            eprintln!(
                "human_workers_utc_bound case={label} source_rfc3339=valid raw={raw} utc={utc} time_status={status:?} dto_valid={}",
                validation.is_ok()
            );
            if validation.is_err() {
                failures.push(format!(
                    "{label}: a valid historical source causes human DTO validation failure"
                ));
            }
            let wire = serde_json::to_value(&item).unwrap();
            if !wire["last_observation"]["observed_at"].is_null()
                || wire["last_observation"]["observed_at_status"] != "unknown"
            {
                failures.push(format!(
                    "{label}: unrepresentable UTC must be null+unknown, without clipping"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }

    #[test]
    fn required_observation_time_refuses_invalid_source_and_preserves_epoch_and_nanoseconds() {
        assert!(
            project_observed_time("unavailable").is_err(),
            "invalid mandatory raw RFC3339 remains an error, never an unknown source"
        );
        for raw in [
            "1970-01-01T00:00:00Z",
            "2026-11-06T13:00:00.000000001+01:00",
        ] {
            let source = chrono::DateTime::parse_from_rfc3339(raw).unwrap();
            let (value, status) = project_observed_time(raw).unwrap();
            assert_eq!(status, HumanWorkerTimeStatusV1::Known);
            let roundtrip =
                chrono::DateTime::parse_from_rfc3339(value.as_deref().unwrap()).unwrap();
            assert_eq!(roundtrip, source);
            assert_eq!(
                roundtrip.timestamp_subsec_nanos(),
                source.timestamp_subsec_nanos()
            );
        }
    }
}
