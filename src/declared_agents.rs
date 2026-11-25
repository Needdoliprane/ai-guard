//! Projects explicit agent declarations without replacing the runner's identity.
//!
//! This is a view of untrusted telemetry, not evidence that a subprocess exists.
//! Parents resolve only to other declarations in the same execution namespace.

use crate::model::Event;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ID_BYTES: usize = 256;
const CLAUDE_SOURCE: &str = "claude-adapter";

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

impl Namespace {
    fn from_event(event: &Event) -> Option<Self> {
        if !valid_text(&event.source, 80) {
            return None;
        }
        Some(Self {
            session_id: event
                .session_id
                .as_deref()
                .filter(|id| valid_id(id))?
                .into(),
            sandbox_id: event
                .sandbox_id
                .as_deref()
                .filter(|id| valid_id(id))?
                .into(),
            context_agent_id: event.agent_id.as_deref().filter(|id| valid_id(id))?.into(),
            source: event.source.clone(),
        })
    }
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

/// Evidence collected only from the authorized read's live event suffix.
#[derive(Default)]
pub(crate) struct ClaudeReadParents {
    namespaces: BTreeMap<String, DelegationClaims>,
    children: BTreeMap<String, ReadParent>,
}

#[derive(Default)]
struct DelegationClaims {
    // None retains a conflict across repeated claims.
    emitters: BTreeMap<String, Option<String>>,
    calls_by_child: BTreeMap<String, Option<String>>,
    children_by_call: BTreeMap<String, Option<String>>,
}

enum ReadParent {
    Candidate,
    ExplicitRoot,
}

impl ClaudeReadParents {
    fn observe_parent(&mut self, id: &str, first_declaration: bool, update: &ParentUpdate) {
        match update {
            ParentUpdate::Unspecified if first_declaration => {
                // A checkpoint child does not distinguish omission from null.
                self.children.insert(id.into(), ReadParent::Candidate);
            }
            ParentUpdate::Unspecified => {}
            ParentUpdate::Root => {
                self.children.insert(id.into(), ReadParent::ExplicitRoot);
            }
            ParentUpdate::Link(_) | ParentUpdate::Invalid => {
                self.children.remove(id);
            }
        }
    }

    fn observe_delegation(&mut self, event: &Event, data: &Map<String, Value>) {
        let child =
            join_id(data, "provider_agent_id").zip(join_id(data, "provider_parent_tool_use_id"));
        let call = if event.kind == "delegation.declared"
            && matches!(
                data.get("delegation_tool").and_then(Value::as_str),
                Some("Agent" | "Task")
            ) {
            let emitter = if data.contains_key("provider_agent_id") {
                join_id(data, "provider_agent_id")
            } else if data
                .get("provider_parent_tool_use_id")
                .is_none_or(Value::is_null)
            {
                join_id(data, "provider_session_id")
            } else {
                None
            };
            join_id(data, "provider_tool_use_id").zip(emitter)
        } else {
            None
        };
        if (call.is_some() || child.is_some())
            && let Some(namespace) = Namespace::from_event(event)
        {
            let claims = self
                .namespaces
                .entry(stable_id(&namespace, ""))
                .or_default();
            if let Some((call, emitter)) = call {
                record_unique(&mut claims.emitters, call, emitter);
            }
            if let Some((child, call)) = child {
                record_unique(&mut claims.calls_by_child, child, call);
                record_unique(&mut claims.children_by_call, call, child);
            }
        }
    }

    fn inferred_parent(&self, id: &str, agent: &DeclaredAgent) -> Option<String> {
        if !matches!(self.children.get(id), Some(ReadParent::Candidate)) {
            return None;
        }
        let claims = self.namespaces.get(&stable_id(&agent.namespace, ""))?;
        let call = claims
            .calls_by_child
            .get(&agent.provider_agent_id)?
            .as_deref()?;
        if claims.children_by_call.get(call)?.as_deref()? != agent.provider_agent_id.as_str() {
            return None;
        }
        let emitter = claims.emitters.get(call)?.as_deref()?;
        Some(stable_id(&agent.namespace, emitter))
    }
}

impl DeclaredAgentProjection {
    /// Consume events in audit order. Unknown, observed or incomplete records
    /// cannot manufacture a declared agent. Missing update fields retain their
    /// previous values; an explicit null parent clears the relationship.
    pub fn observe(&mut self, event: &Event) {
        self.observe_with_claude_parents(event, None);
    }

    pub(crate) fn observe_with_claude_parents(
        &mut self,
        event: &Event,
        read: Option<&mut ClaudeReadParents>,
    ) {
        let read = read.filter(|_| event.source == CLAUDE_SOURCE);
        if event.provenance != "declared" || (event.kind != "agent.declared" && read.is_none()) {
            return;
        }
        let Some(data) = event.data.as_object() else {
            return;
        };
        if event.kind != "agent.declared" {
            if let Some(read) = read {
                read.observe_delegation(event, data);
            }
            return;
        }
        let Some(provider_agent_id) = agent_id(data) else {
            return;
        };
        let Some(namespace) = Namespace::from_event(event) else {
            return;
        };
        let id = stable_id(&namespace, &provider_agent_id);
        let parent = parent_update(data);
        if let Some(read) = read {
            read.observe_parent(&id, !self.agents.contains_key(&id), &parent);
            read.observe_delegation(event, data);
        }
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

        match parent {
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
        self.finish_with_claude_parents(None)
    }

    pub(crate) fn finish_with_claude_parents(self, read: Option<ClaudeReadParents>) -> Vec<Value> {
        let mut parents = BTreeMap::new();
        for (id, agent) in &self.agents {
            if let Some(parent) = &agent.provider_parent_agent_id {
                let parent_id = stable_id(&agent.namespace, parent);
                if self.agents.contains_key(&parent_id) {
                    parents.insert(id.clone(), parent_id);
                }
            }
        }
        let explicit_cycles = cycle_members(&parents);
        let mut inferred = BTreeSet::new();
        if let Some(read) = &read {
            for (id, agent) in &self.agents {
                if agent.provider_parent_agent_id.is_none()
                    && let Some(parent_id) = read.inferred_parent(id, agent)
                    && self.agents.contains_key(&parent_id)
                {
                    inferred.insert(id.clone());
                    parents.insert(id.clone(), parent_id);
                }
            }
        }
        // An inferred edge must never displace a valid explicit link.
        let cycles_with_inference = if inferred.is_empty() {
            BTreeSet::new()
        } else {
            cycle_members(&parents)
        };
        self.agents
            .into_iter()
            .map(|(id, agent)| {
                let inferred_edge = inferred.contains(&id);
                let inferred_cycle = inferred_edge && cycles_with_inference.contains(&id);
                let explicit_cycle = explicit_cycles.contains(&id);
                let parent_id = parents
                    .get(&id)
                    .filter(|_| !explicit_cycle && !inferred_cycle);
                let unresolved = agent
                    .provider_parent_agent_id
                    .as_ref()
                    .filter(|_| !parents.contains_key(&id));
                let warning = if inferred_cycle {
                    Some("Lien parent inféré retiré pour éviter un cycle.")
                } else if explicit_cycle {
                    Some("Cycle dans les liens déclarés ; lien parent retiré.")
                } else if unresolved.is_some() {
                    Some("Parent déclaré absent de ce contexte d'exécution.")
                } else {
                    agent.parent_warning.as_deref()
                };
                let mut item = json!({
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
                });
                if let Some(read) = &read
                    && agent.namespace.source == CLAUDE_SOURCE
                {
                    item["parent_link"] = json!(parent_id.map(|_| {
                        if inferred_edge {
                            "inferred"
                        } else {
                            "declared"
                        }
                    }));
                    if parent_id.is_some() && inferred_edge {
                        item["field_provenance"]["parent_id"] = json!("inferred");
                    } else if agent.provider_parent_agent_id.is_none()
                        && !matches!(read.children.get(&id), Some(ReadParent::ExplicitRoot))
                        && let Some(fields) = item["field_provenance"].as_object_mut()
                    {
                        fields.remove("parent_id");
                    }
                }
                item
            })
            .collect()
    }
}

fn join_id<'a>(data: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    data.get(key)?
        .as_str()
        .filter(|id| valid_id(id) && !id.contains("[REDACTED") && !id.ends_with("[truncated]"))
}

fn record_unique(index: &mut BTreeMap<String, Option<String>>, key: &str, value: &str) {
    index
        .entry(key.into())
        .and_modify(|current| {
            if current.as_deref() != Some(value) {
                *current = None;
            }
        })
        .or_insert_with(|| Some(value.into()));
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
