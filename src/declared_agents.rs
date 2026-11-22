//! Projects explicit agent declarations without replacing the runner's identity.
//!
//! This is a view of untrusted telemetry, not evidence that a subprocess exists.
//! Parents resolve only to other declarations in the same execution namespace.

use crate::model::Event;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ID_BYTES: usize = 256;

/// Serializable so that audit retention can persist and resume the fold.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredAgentProjection {
    agents: BTreeMap<String, DeclaredAgent>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Namespace {
    session_id: String,
    sandbox_id: String,
    context_agent_id: String,
    source: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredAgent {
    namespace: Namespace,
    project_id: Option<String>,
    provider_agent_id: String,
    provider_parent_agent_id: Option<String>,
    parent_warning: Option<String>,
    provider: Option<String>,
    name: String,
    model: Option<String>,
    task: Option<String>,
    status: String,
    reported_state: Option<String>,
    started_at: Option<String>,
    last_activity: String,
}

enum ParentUpdate {
    Unspecified,
    Root,
    Link(String),
    Invalid,
}

impl DeclaredAgentProjection {
    /// Consume events in audit order. Unknown, observed or incomplete records
    /// cannot manufacture a declared agent. Missing update fields retain their
    /// previous values; an explicit null parent clears the relationship.
    pub fn observe(&mut self, event: &Event) {
        if event.kind != "agent.declared" || event.provenance != "declared" {
            return;
        }
        let Some(data) = event.data.as_object() else {
            return;
        };
        let Some(provider_agent_id) = agent_id(data) else {
            return;
        };
        let (Some(session_id), Some(sandbox_id), Some(context_agent_id)) = (
            event.session_id.as_deref().filter(|id| valid_id(id)),
            event.sandbox_id.as_deref().filter(|id| valid_id(id)),
            event.agent_id.as_deref().filter(|id| valid_id(id)),
        ) else {
            return;
        };
        if !valid_text(&event.source, 80) {
            return;
        }
        let namespace = Namespace {
            session_id: session_id.into(),
            sandbox_id: sandbox_id.into(),
            context_agent_id: context_agent_id.into(),
            source: event.source.clone(),
        };
        let id = stable_id(&namespace, &provider_agent_id);
        let agent = self.agents.entry(id).or_insert_with(|| DeclaredAgent {
            namespace,
            project_id: event.project_id.clone(),
            name: provider_agent_id.clone(),
            provider_agent_id,
            provider_parent_agent_id: None,
            parent_warning: None,
            provider: None,
            model: None,
            task: None,
            status: "unknown".into(),
            reported_state: None,
            started_at: None,
            last_activity: event.timestamp.clone(),
        });

        match parent_update(data) {
            ParentUpdate::Unspecified => {}
            ParentUpdate::Root => {
                agent.provider_parent_agent_id = None;
                agent.parent_warning = None;
            }
            ParentUpdate::Link(parent) => {
                agent.provider_parent_agent_id = Some(parent);
                agent.parent_warning = None;
            }
            ParentUpdate::Invalid => {
                agent.provider_parent_agent_id = None;
                agent.parent_warning =
                    Some("Identifiant parent invalide ou contradictoire.".into());
            }
        }

        for (key, target, max_bytes) in [
            ("provider", &mut agent.provider, 256),
            ("model", &mut agent.model, 256),
            ("task", &mut agent.task, 4096),
        ] {
            if data.get(key).is_some_and(Value::is_null) {
                *target = None;
            } else if let Some(value) = data.get(key).and_then(Value::as_str) {
                let safe = if key == "task" {
                    !value.is_empty()
                        && value.len() <= max_bytes
                        && !value
                            .chars()
                            .any(|c| c.is_control() && c != '\n' && c != '\t')
                } else {
                    valid_text(value, max_bytes)
                };
                if safe {
                    *target = Some(value.into());
                }
            }
        }
        if let Some(name) = data.get("name").and_then(Value::as_str)
            && valid_text(name, 256)
        {
            agent.name = name.into();
        }
        if let Some(state) = data.get("state") {
            let state = state.as_str().filter(|state| valid_text(state, 64));
            agent.reported_state = state.map(str::to_owned);
            agent.status = match state {
                Some("started") => "running",
                Some("completed") => "completed",
                Some("failed") => "error",
                Some("stopped") => "stopped",
                _ => "unknown",
            }
            .into();
            if state == Some("started") && agent.started_at.is_none() {
                agent.started_at = Some(event.timestamp.clone());
            }
        }
        agent.last_activity = event.timestamp.clone();
    }

    /// Resolve explicit parent IDs only after all declarations have arrived,
    /// allowing a child to be reported before its parent. Cycle edges are
    /// removed rather than exposed as a recursive tree to the frontend.
    pub fn finish(self) -> Vec<Value> {
        let mut parents = BTreeMap::new();
        for (id, agent) in &self.agents {
            if let Some(parent) = &agent.provider_parent_agent_id {
                let parent_id = stable_id(&agent.namespace, parent);
                if self.agents.contains_key(&parent_id) {
                    parents.insert(id.clone(), parent_id);
                }
            }
        }
        let cycles = cycle_members(&parents);
        self.agents
            .into_iter()
            .map(|(id, agent)| {
                let is_cycle = cycles.contains(&id);
                let parent_id = parents.get(&id).filter(|_| !is_cycle);
                let unresolved = agent
                    .provider_parent_agent_id
                    .as_ref()
                    .filter(|_| !parents.contains_key(&id));
                let warning = if is_cycle {
                    Some("Cycle dans les liens déclarés ; lien parent retiré.")
                } else if unresolved.is_some() {
                    Some("Parent déclaré absent de ce contexte d'exécution.")
                } else {
                    agent.parent_warning.as_deref()
                };
                json!({
                    "id": id,
                    "parent_id": parent_id,
                    "project_id": agent.project_id,
                    "session_id": agent.namespace.session_id,
                    "sandbox_id": agent.namespace.sandbox_id,
                    "context_agent_id": agent.namespace.context_agent_id,
                    "source": agent.namespace.source,
                    "provider_agent_id": agent.provider_agent_id,
                    "provider_parent_agent_id": agent.provider_parent_agent_id,
                    "provider": agent.provider,
                    "name": agent.name,
                    "model": agent.model,
                    "task": agent.task,
                    "status": agent.status,
                    "reported_state": agent.reported_state,
                    "started_at": agent.started_at,
                    "last_activity": agent.last_activity,
                    "provenance": "declared",
                    "compatibility": "observed",
                    "parent_unresolved": unresolved,
                    "hierarchy_warning": warning,
                    "field_provenance": {
                        "provider": "declared",
                        "name": "declared",
                        "model": "declared",
                        "task": "declared",
                        "status": "declared",
                        "parent_id": "declared",
                        "provider_agent_id": "declared",
                        "provider_parent_agent_id": "declared",
                        "started_at": "declared",
                        "last_activity": "declared"
                    }
                })
            })
            .collect()
    }
}

fn valid_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn valid_id(value: &str) -> bool {
    valid_text(value, MAX_ID_BYTES)
}

fn agent_id(data: &Map<String, Value>) -> Option<String> {
    let canonical = data.get("provider_agent_id");
    let alias = data.get("declared_agent_id");
    if canonical.is_some() && alias.is_some() && canonical != alias {
        return None;
    }
    canonical
        .or(alias)
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .map(str::to_owned)
}

fn parent_update(data: &Map<String, Value>) -> ParentUpdate {
    let canonical = data.get("provider_parent_agent_id");
    let alias = data.get("declared_parent_agent_id");
    if canonical.is_some() && alias.is_some() && canonical != alias {
        return ParentUpdate::Invalid;
    }
    match canonical.or(alias) {
        None => ParentUpdate::Unspecified,
        Some(Value::Null) => ParentUpdate::Root,
        Some(Value::String(id)) if valid_id(id) => ParentUpdate::Link(id.clone()),
        Some(_) => ParentUpdate::Invalid,
    }
}

fn stable_id(namespace: &Namespace, provider_agent_id: &str) -> String {
    // A JSON tuple preserves boundaries even if identifiers contain punctuation.
    // Metadata, timestamps and ordering are deliberately absent from identity.
    let tuple = json!([
        namespace.session_id,
        namespace.sandbox_id,
        namespace.context_agent_id,
        namespace.source,
        provider_agent_id
    ]);
    format!(
        "declared-{}",
        hex::encode(Sha256::digest(tuple.to_string().as_bytes()))
    )
}

fn cycle_members(parents: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut completed = BTreeSet::new();
    let mut cycles = BTreeSet::new();
    for start in parents.keys() {
        if completed.contains(start.as_str()) {
            continue;
        }
        let mut positions = BTreeMap::new();
        let mut path: Vec<&str> = Vec::new();
        let mut cursor = start.as_str();
        while !completed.contains(cursor) {
            if let Some(index) = positions.get(cursor) {
                cycles.extend(path[*index..].iter().map(|id| (*id).to_owned()));
                break;
            }
            positions.insert(cursor, path.len());
            path.push(cursor);
            let Some(parent) = parents.get(cursor) else {
                break;
            };
            cursor = parent.as_str();
        }
        completed.extend(path);
    }
    cycles
}
