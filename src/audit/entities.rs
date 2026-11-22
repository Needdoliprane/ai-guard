//! Shared projection of the legacy dashboard entities from ordered audit facts.
use crate::{
    declared_agents::DeclaredAgentProjection,
    model::{Event, Outcome},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) struct Projection {
    sandboxes: bool,
    items: BTreeMap<String, Value>,
    declared: DeclaredAgentProjection,
}

/// Resumable state of a projection, persisted by audit retention snapshots.
/// Its field layout is part of `RetentionSnapshotV1` version 1.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectionStateV1 {
    pub(crate) items: BTreeMap<String, Value>,
    pub(crate) declared: DeclaredAgentProjection,
}

impl Projection {
    pub(crate) fn new(sandboxes: bool) -> Self {
        Self {
            sandboxes,
            items: BTreeMap::new(),
            declared: DeclaredAgentProjection::default(),
        }
    }

    pub(crate) fn from_state(sandboxes: bool, state: ProjectionStateV1) -> Self {
        Self {
            sandboxes,
            items: state.items,
            declared: state.declared,
        }
    }

    pub(crate) fn state(&self) -> ProjectionStateV1 {
        ProjectionStateV1 {
            items: self.items.clone(),
            declared: self.declared.clone(),
        }
    }

    pub(crate) fn observe(&mut self, event: &Event) {
        let sandboxes = self.sandboxes;
        let id = if sandboxes {
            event.sandbox_id.as_ref()
        } else {
            event.agent_id.as_ref()
        };
        let Some(id) = id else { return };
        if !sandboxes {
            self.declared.observe(event);
            if event.kind == "agent.declared"
                && event.provenance == "declared"
                && (event.data.get("provider_agent_id").is_some()
                    || event.data.get("declared_agent_id").is_some())
            {
                return;
            }
        }

        let item = self.items.entry(id.clone()).or_insert_with(|| {
            if sandboxes {
                json!({
                    "id": id,
                    "engine": null,
                    "image": null,
                    "profile": null,
                    "status": "unknown",
                    "project_id": event.project_id,
                    "session_id": event.session_id,
                    "network": null,
                    "started_at": null,
                    "resources": null,
                    "mounts": []
                })
            } else {
                json!({
                    "id": id,
                    "provider": null,
                    "name": id,
                    "status": "unknown",
                    "parent_id": event.parent_agent_id,
                    "project_id": event.project_id,
                    "sandbox_id": event.sandbox_id,
                    "model": null,
                    "session_id": event.session_id,
                    "started_at": null,
                    "last_activity": null,
                    "task": null,
                    "compatibility": event.coverage,
                    "provenance": event.provenance
                })
            }
        });
        if event.provenance == "observed" {
            let state = match event.kind.as_str() {
                "agent.started" | "command.started" => Some("running"),
                "agent.completed" | "agent.stopped" | "command.completed" => Some("completed"),
                "agent.failed" | "command.failed" | "sandbox.failed" => Some("error"),
                "runtime.execution.error" => Some("unknown"),
                "sandbox.created" | "session.started" => Some("ready"),
                "sandbox.stopped" => Some("stopped"),
                "security.decision"
                    if event
                        .decision
                        .as_ref()
                        .is_some_and(|decision| decision.outcome == Outcome::Deny) =>
                {
                    Some("blocked")
                }
                _ => None,
            };
            if let Some(status) = state {
                item["status"] = json!(status);
            }
            if matches!(
                event.kind.as_str(),
                "command.started" | "agent.started" | "sandbox.created"
            ) {
                item["started_at"] = json!(event.timestamp);
            }
            if event.kind == "sandbox.metrics" {
                item["resources"] = event.data.clone();
            }
        }
        if !sandboxes {
            item["last_activity"] = json!(event.timestamp);
        }
        let fields: &[&str] = if sandboxes {
            &["engine", "image", "profile", "network", "mounts"]
        } else {
            &["provider", "name", "model", "task"]
        };
        if item.get("field_provenance").is_none() {
            item["field_provenance"] = json!({});
        }
        if event.provenance == "observed" && !sandboxes {
            item["compatibility"] = json!(event.coverage);
        }
        for key in fields {
            if let Some(value) = event.data.get(*key).filter(|value| !value.is_null()) {
                let prior_observed = item["field_provenance"][*key] == "observed";
                if (sandboxes || prior_observed) && event.provenance != "observed" {
                    continue;
                }
                item[*key] = value.clone();
                item["field_provenance"][*key] = json!(event.provenance);
            }
        }
    }

    pub(crate) fn finish(self) -> Vec<Value> {
        let mut items: Vec<Value> = self.items.into_values().collect();
        if !self.sandboxes {
            items.extend(self.declared.finish());
        }
        items
    }
}
