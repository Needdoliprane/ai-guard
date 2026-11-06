//! A read-only federation projection from one checked tenant snapshot.
use super::*;
use crate::control::{RuntimeKindV1, WorkerV1};

fn empty_association(status: AuthorityBrokerStatusV1) -> AuthorityBrokerAssociationV1 {
    AuthorityBrokerAssociationV1 {
        status,
        broker: None,
        runtime_kind: None,
        runtime_matches: None,
        in_current_grant: None,
    }
}

pub(in crate::audit) fn association(
    db: &Connection,
    fact: Option<(&str, RuntimeKindV1)>,
    evaluated_at: &str,
    registry_available: bool,
) -> anyhow::Result<AuthorityBrokerAssociationV1> {
    if !registry_available {
        return Ok(empty_association(
            AuthorityBrokerStatusV1::RegistryUnavailable,
        ));
    }
    let Some((authority, runtime)) = fact.filter(|(authority, _)| !authority.is_empty()) else {
        return Ok(empty_association(AuthorityBrokerStatusV1::AuthorityUnknown));
    };
    let owner = db
        .query_row(
            "SELECT broker_id,runtime_kind FROM broker_registry_authorities WHERE authority_id=?1",
            [authority],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((broker_id, kind)) = owner else {
        return Ok(empty_association(AuthorityBrokerStatusV1::Unregistered));
    };
    let kind = match kind.as_str() {
        "docker" => BrokerRuntimeKindV1::Docker,
        "podman" => BrokerRuntimeKindV1::Podman,
        "kubernetes" => BrokerRuntimeKindV1::Kubernetes,
        _ => anyhow::bail!("invalid registered runtime kind"),
    };
    let runtime = match runtime {
        RuntimeKindV1::Docker => BrokerRuntimeKindV1::Docker,
        RuntimeKindV1::Podman => BrokerRuntimeKindV1::Podman,
    };
    let broker = load_broker(db, &broker_id)?.context("historical authority owner is missing")?;
    let state = broker.view_at(evaluated_at)?.state;
    let in_current_grant = broker
        .authorities
        .iter()
        .any(|entry| entry.authority_id == authority && entry.runtime_kind == kind);
    // Old-incarnation heartbeats do not describe the current incarnation's lease.
    let heartbeat = broker
        .last_heartbeat
        .as_ref()
        .filter(|heartbeat| heartbeat.incarnation_id == broker.incarnation_id);
    let last_received_at = heartbeat.map(|heartbeat| heartbeat.received_at.clone());
    let lease_expires_at = heartbeat.map(|heartbeat| heartbeat.lease_expires_at.clone());
    Ok(AuthorityBrokerAssociationV1 {
        status: AuthorityBrokerStatusV1::Registered,
        runtime_kind: Some(kind),
        runtime_matches: Some(kind == runtime),
        in_current_grant: Some(in_current_grant),
        broker: Some(FederationBrokerV1 {
            broker_id: broker.broker_id,
            current_incarnation_id: broker.incarnation_id,
            state,
            grant_revision: broker.grant_revision.to_string(),
            heartbeat_revision: broker.heartbeat_revision.to_string(),
            last_received_at,
            lease_expires_at,
        }),
    })
}

impl Store {
    /// Historical label ownership can postdate a worker fact. These associations
    /// do not identify the broker which performed a launch or an inspection.
    pub fn federation_workers_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<FederationWorkerPageV1> {
        require((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            registry_uuid(cursor)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let schema: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let registry_available = match schema {
            2..=5 => false,
            6..=10 => true,
            _ => return Err(crate::control::ControlFailure::MigrationRequired.into()),
        };
        let evaluated_at = format_time(Utc::now());
        let mut page = FederationWorkerPageV1 {
            version: 1,
            source: "tenant_worker_broker_registry".into(),
            consistency: "snapshot_per_page".into(),
            evaluated_at,
            runtime_inventory: "unknown".into(),
            registry_status: if registry_available {
                "available"
            } else {
                "migration_required"
            }
            .into(),
            items: Vec::new(),
            // Private size placeholder, replaced before publication. Reserve
            // the exact encoded cursor width even when the final page has none.
            next_cursor: Some("00000000-0000-0000-0000-000000000000".into()),
        };
        let mut bytes = serde_json::to_vec(&page)?.len();
        let mut has_more = false;
        {
            let mut statement = tx.prepare(
                "SELECT snapshot_json FROM workers WHERE worker_id>?1 ORDER BY worker_id LIMIT ?2",
            )?;
            let mut rows = statement.query(params![cursor.unwrap_or(""), i64::from(limit) + 1])?;
            while let Some(row) = rows.next()? {
                if page.items.len() == limit as usize {
                    has_more = true;
                    break;
                }
                let worker: WorkerV1 = serde_json::from_str(&row.get::<_, String>(0)?)?;
                let launch_broker = association(
                    &tx,
                    Some((&worker.launch.authority_id, worker.launch.runtime_kind)),
                    &page.evaluated_at,
                    registry_available,
                )?;
                let observation_broker = association(
                    &tx,
                    worker.last_observation.as_ref().map(|observation| {
                        (
                            observation.inspection.authority_id.as_str(),
                            observation.inspection.runtime_kind,
                        )
                    }),
                    &page.evaluated_at,
                    registry_available,
                )?;
                let item = FederationWorkerV1 {
                    worker,
                    launch_broker,
                    observation_broker,
                };
                let next_bytes = bytes
                    .checked_add(serde_json::to_vec(&item)?.len())
                    .and_then(|count| count.checked_add(usize::from(!page.items.is_empty())))
                    .context("federation page size overflow")?;
                if next_bytes > MAX_FEDERATION_PAGE_BYTES {
                    anyhow::ensure!(
                        !page.items.is_empty(),
                        "worker exceeds federation page byte limit"
                    );
                    has_more = true;
                    break;
                }
                bytes = next_bytes;
                page.items.push(item);
            }
        }
        page.next_cursor = if has_more {
            Some(
                page.items
                    .last()
                    .context("federation page cannot advance")?
                    .worker
                    .worker_id
                    .clone(),
            )
        } else {
            None
        };
        let value = serde_json::to_value(&page)?;
        anyhow::ensure!(
            self.redactor.value_unclipped(&value) == value,
            "federation page requires redaction"
        );
        anyhow::ensure!(
            serde_json::to_vec(&page)?.len() <= MAX_FEDERATION_PAGE_BYTES,
            "federation page exceeds byte limit"
        );
        self.commit_checked(tx)?;
        Ok(page)
    }
}
