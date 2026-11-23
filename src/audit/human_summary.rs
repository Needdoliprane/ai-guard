//! Store side of the per-context 360° summary: one checked snapshot
//! transaction of one tenant, recorded under the human read action `summary`.
//!
//! Every count is computed here over the control's own durable registries or
//! the verified journal, inside the transaction that revalidates the context.
//! A section whose capability the context lacks is `not_granted`; nothing is
//! inferred from it. A runtime inventory is never claimed.
use super::*;
use crate::{
    broker_registry::BrokerStateV1,
    control::{QuotaPolicyV1, RuntimeKindV1, WorkerStateV1, WorkerV1},
    identity::*,
};
use rusqlite::named_params;
use std::collections::BTreeMap;

/// Row budgets that keep one summary inside the read lane's cost envelope.
const MAX_WORKER_ROWS: i64 = 100_000;
const MAX_JOURNAL_ROWS: i64 = 20_000;
const MAX_ALLOCATION_ROWS: i64 = 100_000;

/// Immutable response anchors supplied only by the selected server context.
pub(crate) struct HumanSummaryFrameV1 {
    context_id: String,
    authorization_version: String,
}
impl HumanSummaryFrameV1 {
    pub(crate) fn new(context_id: &str, authorization_version: &str) -> anyhow::Result<Self> {
        for value in [context_id, authorization_version] {
            anyhow::ensure!(
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human summary frame anchor"
            );
        }
        Ok(Self {
            context_id: context_id.into(),
            authorization_version: authorization_version.into(),
        })
    }
}

fn count(value: u64) -> String {
    value.to_string()
}
fn counts(entries: impl IntoIterator<Item = (String, u64)>) -> SummaryCountsV1 {
    let mut map = BTreeMap::new();
    for (key, value) in entries {
        *map.entry(key).or_insert(0u64) += value;
    }
    map.into_iter().map(|(k, v)| (k, count(v))).collect()
}
fn label<T: Serialize>(value: &T) -> anyhow::Result<String> {
    match serde_json::to_value(value)? {
        Value::String(text) => Ok(text),
        _ => anyhow::bail!("summary label is not a closed word"),
    }
}

impl Store {
    pub(crate) fn human_summary(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanSummaryFrameV1,
        window: SummaryWindowV1,
        quota: &QuotaPolicyV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanSummaryV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let summary =
            self.human_summary_checked(&tx, scope, frame, window, quota, transient, false)?;
        self.commit_checked(tx)?;
        Ok(summary)
    }
    /// The same summary in the business read epoch of the retained 14/15 source.
    pub(crate) fn modern_human_summary(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanSummaryFrameV1,
        window: SummaryWindowV1,
        quota: &QuotaPolicyV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanSummaryV1> {
        self.with_modern_read(access, |tx| {
            self.human_summary_checked(tx, scope, frame, window, quota, transient, true)
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn human_summary_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanSummaryFrameV1,
        window: SummaryWindowV1,
        quota: &QuotaPolicyV1,
        transient: &Redactor,
        owned: bool,
    ) -> anyhow::Result<HumanSummaryV1> {
        identity::require_human_read(tx)?;
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let administration = current.mode() == ContextModeV1::Administration;
        let has = |capability| current.has_capability(capability);
        let metadata = has(HumanCapabilityV1::ReadMetadata)
            || (administration && has(HumanCapabilityV1::ReadTenantMetadata));
        let workers = administration && has(HumanCapabilityV1::ReadWorkerRegistry);
        let brokers = administration && has(HumanCapabilityV1::ReadBrokerRegistry);
        let now = chrono::Utc::now();
        let evaluated_at = crate::broker_registry::format_time(now);
        let from = window
            .duration()
            .map(|span| crate::broker_registry::format_time(now - span));
        let registry = if owned {
            matches!(version, 14 | 15)
        } else {
            (6..=13).contains(&version)
        };

        let sections = SummarySectionsV1 {
            resources: if metadata {
                resources(tx, &current)?
            } else {
                SummarySectionV1::NotGranted
            },
            decisions: if metadata {
                decisions(tx, &current, from.as_deref(), &evaluated_at)?
            } else {
                SummarySectionV1::NotGranted
            },
            workers: if !workers {
                SummarySectionV1::NotGranted
            } else if !registry {
                unavailable(SummaryUnavailableV1::MigrationRequired)
            } else {
                worker_groups(tx, &evaluated_at)?
            },
            brokers: if !brokers {
                SummarySectionV1::NotGranted
            } else if !registry {
                unavailable(SummaryUnavailableV1::MigrationRequired)
            } else {
                broker_states(tx, &evaluated_at)?
            },
            quota: if let Some(section) = quota_usage_gate(workers, registry) {
                section
            } else {
                let usage = super::control::quota_usage(tx, quota)?;
                SummarySectionV1::Ready {
                    coverage: SummaryCoverageV1::CompleteRegistry,
                    data: SummaryQuotaV1 {
                        max_active_workers: usage.policy.max_active_workers.to_string(),
                        max_cpu_millis: usage.policy.max_cpu_millis.to_string(),
                        max_memory_bytes: usage.policy.max_memory_bytes.to_string(),
                        held_workers: usage.held_workers.to_string(),
                        held_cpu_millis: usage.held_cpu_millis.to_string(),
                        held_memory_bytes: usage.held_memory_bytes.to_string(),
                    },
                }
            },
            usage: match quota_usage_gate(workers, registry) {
                Some(section) => section,
                None => usage(tx, from.as_deref(), &evaluated_at)?,
            },
            attention: if !(workers || brokers) {
                SummarySectionV1::NotGranted
            } else if !registry {
                unavailable(SummaryUnavailableV1::MigrationRequired)
            } else if owned && !brokers {
                unavailable(SummaryUnavailableV1::ProfileUnavailable)
            } else {
                // On the owned profile only the broker facts are read.
                attention(tx, &evaluated_at, workers && !owned, brokers)?
            },
        };
        let ready = [
            matches!(sections.resources, SummarySectionV1::Ready { .. }),
            matches!(sections.decisions, SummarySectionV1::Ready { .. }),
            matches!(sections.workers, SummarySectionV1::Ready { .. }),
            matches!(sections.brokers, SummarySectionV1::Ready { .. }),
            matches!(sections.quota, SummarySectionV1::Ready { .. }),
            matches!(sections.usage, SummarySectionV1::Ready { .. }),
            matches!(sections.attention, SummarySectionV1::Ready { .. }),
        ]
        .into_iter()
        .filter(|ready| *ready)
        .count();
        let read_receipt = self.record_human_read_checked(
            tx,
            current.principal(),
            Some(&current),
            "summary",
            ready,
            transient,
        )?;
        let summary = HumanSummaryV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            mode: current.mode(),
            evaluated_at: evaluated_at.clone(),
            period: SummaryPeriodV1 {
                window,
                from,
                to: evaluated_at,
            },
            consistency: "transaction_snapshot".into(),
            sections,
            read_receipt,
        };
        let value = serde_json::to_value(&summary)?;
        anyhow::ensure!(
            self.redactor.union(transient).value(&value) == value,
            "human summary requires redaction"
        );
        anyhow::ensure!(
            serde_json::to_vec(&summary)?.len() <= HUMAN_MAX_PAGE_BYTES,
            "human summary exceeds its byte bound"
        );
        Ok(summary)
    }
}

fn unavailable<T>(code: SummaryUnavailableV1) -> SummarySectionV1<T> {
    SummarySectionV1::Unavailable { code }
}

/// Quota and usage require registry access in an admitted store format.
fn quota_usage_gate<T>(granted: bool, registry: bool) -> Option<SummarySectionV1<T>> {
    if !granted {
        Some(SummarySectionV1::NotGranted)
    } else if !registry {
        Some(unavailable(SummaryUnavailableV1::MigrationRequired))
    } else {
        None
    }
}

fn resources(
    tx: &Connection,
    scope: &AuthorizedScopeV1,
) -> anyhow::Result<SummarySectionV1<SummaryResourcesV1>> {
    let sql = format!(
        "SELECT r.resource_kind,COUNT(*) FROM identity_resources r WHERE r.active=1 AND json_extract(r.payload_json,'$.organization_scope')=:organization AND {} GROUP BY r.resource_kind",
        super::human::allowed_sql(scope, ResourcePermissionV1::Metadata)
    );
    let key = scope.principal().key()?;
    let mut statement = tx.prepare(&sql)?;
    let rows = statement
        .query_map(
            named_params! {":organization": scope.organization_scope(), ":principal": key},
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_kind: SummaryCountsV1 = ["session", "agent", "sandbox", "project"]
        .into_iter()
        .map(|kind| (kind.to_owned(), count(0)))
        .collect();
    for (kind, value) in rows {
        anyhow::ensure!(
            by_kind.contains_key(&kind),
            "unknown authorized resource kind"
        );
        by_kind.insert(kind, count(u64::try_from(value)?));
    }
    Ok(SummarySectionV1::Ready {
        coverage: SummaryCoverageV1::AuthorizedSelection,
        data: SummaryResourcesV1 { by_kind },
    })
}

/// Gateway decisions on authorized resources, newest first, within the
/// period and the journal row budget. A budget cut is reported, not hidden.
fn decisions(
    tx: &Connection,
    scope: &AuthorizedScopeV1,
    from: Option<&str>,
    to: &str,
) -> anyhow::Result<SummarySectionV1<SummaryDecisionsV1>> {
    let sql = format!(
        "SELECT e.decision,e.received_at FROM events e JOIN identity_resources r ON r.resource_kind=CASE WHEN json_extract(e.payload,'$.session_id') IS NOT NULL THEN 'session' WHEN e.sandbox_id IS NOT NULL THEN 'sandbox' WHEN e.agent_id IS NOT NULL THEN 'agent' ELSE 'project' END AND r.resource_id=COALESCE(json_extract(e.payload,'$.session_id'),e.sandbox_id,e.agent_id,e.project_id) WHERE e.decision IS NOT NULL AND e.kind NOT GLOB 'control.identity.*' AND r.active=1 AND json_extract(r.payload_json,'$.organization_scope')=:organization AND e.received_at<=:to AND {} ORDER BY e.seq DESC LIMIT :limit",
        super::human::allowed_sql(scope, ResourcePermissionV1::Metadata)
    );
    let key = scope.principal().key()?;
    let mut statement = tx.prepare(&sql)?;
    let rows = statement
        .query_map(
            named_params! {
                ":organization": scope.organization_scope(),
                ":principal": key,
                ":to": to,
                ":limit": MAX_JOURNAL_ROWS + 1,
            },
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let budget_reached = rows.len() as i64 > MAX_JOURNAL_ROWS;
    let mut by_outcome: BTreeMap<String, u64> = ["allow", "deny", "review"]
        .into_iter()
        .map(|outcome| (outcome.to_owned(), 0))
        .collect();
    let mut oldest = None;
    let mut reached_from = false;
    for (decision, received_at) in rows.into_iter().take(MAX_JOURNAL_ROWS as usize) {
        if from.is_some_and(|from| received_at.as_str() < from) {
            reached_from = true;
            break;
        }
        oldest = Some(received_at);
        if let Some(decision) = decision {
            let slot = by_outcome
                .get_mut(&decision)
                .ok_or_else(|| anyhow::anyhow!("unknown recorded decision"))?;
            *slot += 1;
        }
    }
    let complete = reached_from || !budget_reached;
    Ok(SummarySectionV1::Ready {
        coverage: SummaryCoverageV1::VerifiedJournal,
        data: SummaryDecisionsV1 {
            by_outcome: by_outcome.into_iter().map(|(k, v)| (k, count(v))).collect(),
            complete,
            oldest_counted_at: if complete { None } else { oldest },
        },
    })
}

fn worker_rows(tx: &Connection) -> anyhow::Result<Option<Vec<WorkerV1>>> {
    let rows: i64 = tx.query_row("SELECT COUNT(*) FROM workers", [], |r| r.get(0))?;
    if rows > MAX_WORKER_ROWS {
        return Ok(None);
    }
    let mut statement = tx.prepare("SELECT snapshot_json FROM workers ORDER BY worker_id")?;
    let workers = statement
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str(&row?)?))
        .collect::<anyhow::Result<Vec<WorkerV1>>>()?;
    Ok(Some(workers))
}

fn worker_groups(
    tx: &Connection,
    evaluated_at: &str,
) -> anyhow::Result<SummarySectionV1<SummaryWorkersV1>> {
    let Some(workers) = worker_rows(tx)? else {
        return Ok(unavailable(SummaryUnavailableV1::CapacityExceeded));
    };
    let mut by_state = Vec::new();
    let mut held = 0u64;
    let mut groups: BTreeMap<(String, String), (Vec<(String, u64)>, u64)> = BTreeMap::new();
    for worker in &workers {
        let state = label(&worker.state)?;
        by_state.push((state.clone(), 1));
        held += u64::from(worker.quota_held);
        let runtime = label(&worker.launch.runtime_kind)?;
        let group = groups
            .entry((worker.launch.authority_id.clone(), runtime))
            .or_default();
        group.0.push((state, 1));
        group.1 += u64::from(worker.quota_held);
    }
    let mut projected = Vec::new();
    for ((authority, runtime), (states, quota_held)) in groups {
        let runtime_kind: RuntimeKindV1 = serde_json::from_value(Value::String(runtime.clone()))?;
        let association = super::broker_registry::federation::association(
            tx,
            Some((&authority, runtime_kind)),
            evaluated_at,
            true,
        )?;
        projected.push(SummaryWorkerGroupV1 {
            association: label(&association.status)?,
            runtime_kind: runtime,
            broker: match association.broker {
                Some(broker) => Some(SummaryBrokerRefV1 {
                    broker_id: broker.broker_id,
                    state: label(&broker.state)?,
                    last_received_at: broker.last_received_at,
                    lease_expires_at: broker.lease_expires_at,
                }),
                None => None,
            },
            by_state: counts(states),
            quota_held: count(quota_held),
        });
    }
    // Brokers first by identifier, then the groups no registered broker owns.
    projected.sort_by(|a, b| {
        (
            a.broker.is_none(),
            a.broker.as_ref().map(|b| b.broker_id.clone()),
        )
            .cmp(&(
                b.broker.is_none(),
                b.broker.as_ref().map(|b| b.broker_id.clone()),
            ))
    });
    let groups_complete = projected.len() <= SUMMARY_MAX_GROUPS;
    projected.truncate(SUMMARY_MAX_GROUPS);
    Ok(SummarySectionV1::Ready {
        coverage: SummaryCoverageV1::CompleteRegistry,
        data: SummaryWorkersV1 {
            by_state: counts(by_state),
            quota_held: count(held),
            groups: projected,
            groups_complete,
            runtime_inventory: "unknown".into(),
        },
    })
}

fn broker_states(
    tx: &Connection,
    evaluated_at: &str,
) -> anyhow::Result<SummarySectionV1<SummaryBrokersV1>> {
    let states = super::broker_registry::states_at(tx, evaluated_at)?;
    let mut by_state: Vec<(String, u64)> = [
        BrokerStateV1::Unseen,
        BrokerStateV1::Current,
        BrokerStateV1::Stale,
        BrokerStateV1::Expired,
        BrokerStateV1::Revoked,
        BrokerStateV1::ClockUncertain,
    ]
    .iter()
    .map(|state| Ok((label(state)?, 0)))
    .collect::<anyhow::Result<_>>()?;
    for (_, state, _) in &states {
        by_state.push((label(state)?, 1));
    }
    Ok(SummarySectionV1::Ready {
        coverage: SummaryCoverageV1::CompleteRegistry,
        data: SummaryBrokersV1 {
            by_state: counts(by_state),
        },
    })
}

/// Terminal allocations whose finish lies in the period. Decimal sums stay
/// exact (u128) and refuse overflow instead of rounding.
fn usage(
    tx: &Connection,
    from: Option<&str>,
    to: &str,
) -> anyhow::Result<SummarySectionV1<SummaryUsageV1>> {
    let rows: i64 = tx.query_row("SELECT COUNT(*) FROM worker_allocations", [], |r| r.get(0))?;
    if rows > MAX_ALLOCATION_ROWS {
        return Ok(unavailable(SummaryUnavailableV1::CapacityExceeded));
    }
    let mut statement = tx.prepare("SELECT payload_json FROM worker_allocations")?;
    let allocations = statement
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str(&row?)?))
        .collect::<anyhow::Result<Vec<crate::control::AllocationV1>>>()?;
    let (mut n, mut duration, mut cpu, mut memory) = (0u64, 0u128, 0u128, 0u128);
    for allocation in allocations {
        anyhow::ensure!(
            allocation.method_version == 1 && !allocation.billable,
            "unknown allocation method"
        );
        let finished = allocation.finished_at.as_str();
        if from.is_some_and(|from| finished < from) || finished > to {
            continue;
        }
        let add = |sum: u128, value: &str| -> anyhow::Result<u128> {
            sum.checked_add(value.parse::<u128>()?)
                .ok_or_else(|| anyhow::anyhow!("allocation sum overflow"))
        };
        n += 1;
        duration = add(duration, &allocation.duration_ns)?;
        cpu = add(cpu, &allocation.cpu_millicore_nanoseconds)?;
        memory = add(memory, &allocation.memory_byte_nanoseconds)?;
    }
    Ok(SummarySectionV1::Ready {
        coverage: SummaryCoverageV1::CompleteRegistry,
        data: SummaryUsageV1 {
            allocations: count(n),
            duration_ns: duration.to_string(),
            cpu_millicore_nanoseconds: cpu.to_string(),
            memory_byte_nanoseconds: memory.to_string(),
            method_version: 1,
            billable: false,
        },
    })
}

/// Points of attention derived only from durable control states.
fn attention(
    tx: &Connection,
    evaluated_at: &str,
    workers: bool,
    brokers: bool,
) -> anyhow::Result<SummarySectionV1<SummaryAttentionV1>> {
    let mut items = Vec::new();
    if workers {
        let Some(rows) = worker_rows(tx)? else {
            return Ok(unavailable(SummaryUnavailableV1::CapacityExceeded));
        };
        for worker in rows {
            let at = worker
                .last_observation
                .as_ref()
                .map(|observation| observation.observed_at.clone());
            let mut push = |kind: &str| {
                items.push(SummaryAttentionItemV1 {
                    kind: kind.into(),
                    subject_kind: "worker".into(),
                    subject_id: worker.worker_id.clone(),
                    at: at.clone(),
                })
            };
            if worker.state == WorkerStateV1::InDoubt {
                push("worker_in_doubt");
            }
            if worker.allocation_inconsistent {
                push("worker_allocation_inconsistent");
            }
            if worker
                .last_observation
                .as_ref()
                .is_some_and(|observation| observation.oom_killed == Some(true))
            {
                push("worker_oom_killed");
            }
        }
    }
    if brokers {
        for (broker_id, state, received_at) in super::broker_registry::states_at(tx, evaluated_at)?
        {
            let kind = match state {
                BrokerStateV1::Current => continue,
                BrokerStateV1::Unseen => "broker_heartbeat_unseen",
                BrokerStateV1::Stale => "broker_lease_stale",
                BrokerStateV1::Expired => "broker_grant_expired",
                BrokerStateV1::Revoked => "broker_revoked",
                BrokerStateV1::ClockUncertain => "broker_clock_uncertain",
            };
            items.push(SummaryAttentionItemV1 {
                kind: kind.into(),
                subject_kind: "broker".into(),
                subject_id: broker_id,
                at: received_at,
            });
        }
    }
    let by_kind = counts(items.iter().map(|item| (item.kind.clone(), 1)));
    // Newest recorded fact first; facts without an instant last, then by id.
    items.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| a.subject_id.cmp(&b.subject_id))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    let has_more = items.len() > SUMMARY_MAX_ATTENTION;
    items.truncate(SUMMARY_MAX_ATTENTION);
    Ok(SummarySectionV1::Ready {
        coverage: SummaryCoverageV1::CompleteRegistry,
        data: SummaryAttentionV1 {
            by_kind,
            items,
            has_more,
        },
    })
}
