//! Which events a retention segment may reclaim from the live store.
//!
//! Structural events stay: projections reference them by sequence and the
//! startup replays fold them. The structural set is exactly what ordinary
//! ingestion refuses as reserved (`api.rs` event admission): every reserved
//! control namespace, decisions, worker and usage facts, the D5 broker kinds
//! and the relay channel kinds. Everything else is an ordinary leaf. The three
//! human read audit kinds are control events that no projection references:
//! the identity replay validates each one and then moves on.
use crate::model::{Event, event_kind_in_namespace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReclaimClassV1 {
    Ordinary,
    HumanRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventClass {
    Structural,
    Reclaimable(ReclaimClassV1),
}

pub(crate) fn human_read_kind(kind: &str) -> bool {
    kind == "control.identity.access"
        || kind == super::super::human_brokers::READ_KIND
        || kind == super::super::human_workers::READ_KIND
}

pub(crate) fn structural_kind(kind: &str) -> bool {
    super::super::D5_CONTROL_EVENT_KINDS.contains(&kind)
        || crate::relay::admission::reserved_kind(kind)
        || ["security.decision", "control", "worker", "usage.allocation"]
            .iter()
            .any(|namespace| event_kind_in_namespace(kind, namespace))
}

pub(crate) fn classify(kind: &str) -> EventClass {
    if human_read_kind(kind) {
        EventClass::Reclaimable(ReclaimClassV1::HumanRead)
    } else if structural_kind(kind) {
        EventClass::Structural
    } else {
        EventClass::Reclaimable(ReclaimClassV1::Ordinary)
    }
}

/// The resource keys an identity grant checks for prior sightings
/// (`identity::ensure_resource_known`): a session, agent, sandbox or project
/// named by any event outside `control.identity.*`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ResourceKey {
    Session(String),
    Agent(String),
    Sandbox(String),
    Project(String),
}

pub(crate) fn sighted_resources(event: &Event) -> Vec<ResourceKey> {
    if event.kind.starts_with("control.identity.") {
        return Vec::new();
    }
    let mut keys = Vec::new();
    if let Some(id) = &event.session_id {
        keys.push(ResourceKey::Session(id.clone()));
    }
    if let Some(id) = &event.agent_id {
        keys.push(ResourceKey::Agent(id.clone()));
    }
    if let Some(id) = &event.sandbox_id {
        keys.push(ResourceKey::Sandbox(id.clone()));
    }
    if let Some(id) = &event.project_id {
        keys.push(ResourceKey::Project(id.clone()));
    }
    keys
}

/// Decides, in sequence order, whether each event of a range is reclaimed.
/// It must observe every retained event before the range too, so that a
/// resource first sighted in an earlier segment is not pinned again.
#[derive(Default)]
pub(crate) struct ReclaimSelector {
    sighted: BTreeSet<ResourceKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Selection {
    Structural,
    Boundary,
    FirstSighting,
    Reclaim(ReclaimClassV1),
}

impl ReclaimSelector {
    /// Observe an event retained below the range; returns nothing.
    pub(crate) fn observe_retained(&mut self, event: &Event) {
        for key in sighted_resources(event) {
            self.sighted.insert(key);
        }
    }

    pub(crate) fn select(
        &mut self,
        event: &Event,
        boundary: bool,
        classes: &BTreeSet<ReclaimClassV1>,
    ) -> Selection {
        let mut first = false;
        for key in sighted_resources(event) {
            first |= self.sighted.insert(key);
        }
        match classify(&event.kind) {
            EventClass::Structural => Selection::Structural,
            _ if boundary => Selection::Boundary,
            _ if first => Selection::FirstSighting,
            EventClass::Reclaimable(class) if classes.contains(&class) => Selection::Reclaim(class),
            EventClass::Reclaimable(_) => Selection::Structural,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_namespaces_are_structural_and_reads_are_reclaimable() {
        for kind in [
            "control.worker.transition",
            "CONTROL.identity.mutation",
            "security.decision",
            "worker.started",
            "usage.allocation",
            "broker.retirement.final",
            "relay_channel.send",
            "control.audit_retention.applied",
        ] {
            assert_eq!(classify(kind), EventClass::Structural, "{kind}");
        }
        for kind in [
            "control.identity.access",
            "control.identity.brokers.access",
            "control.identity.workers.access",
        ] {
            assert_eq!(
                classify(kind),
                EventClass::Reclaimable(ReclaimClassV1::HumanRead),
                "{kind}"
            );
        }
        for kind in ["command.requested", "security.alert", "agent.declared"] {
            assert_eq!(
                classify(kind),
                EventClass::Reclaimable(ReclaimClassV1::Ordinary),
                "{kind}"
            );
        }
    }

    #[test]
    fn the_first_sighting_of_a_resource_and_the_boundary_are_pinned() {
        let classes = BTreeSet::from([ReclaimClassV1::Ordinary, ReclaimClassV1::HumanRead]);
        let mut selector = ReclaimSelector::default();
        let mut first = Event::new("command.requested");
        first.session_id = Some("s-1".into());
        let mut again = Event::new("command.completed");
        again.session_id = Some("s-1".into());
        let mut identity = Event::new("control.identity.mutation");
        identity.session_id = Some("s-2".into());
        let mut later = Event::new("file.changed");
        later.session_id = Some("s-2".into());
        assert_eq!(
            selector.select(&first, false, &classes),
            Selection::FirstSighting
        );
        assert_eq!(
            selector.select(&again, false, &classes),
            Selection::Reclaim(ReclaimClassV1::Ordinary)
        );
        // An identity event never counts as a sighting of the resource.
        assert_eq!(
            selector.select(&identity, false, &classes),
            Selection::Structural
        );
        assert_eq!(
            selector.select(&later, false, &classes),
            Selection::FirstSighting
        );
        assert_eq!(
            selector.select(&Event::new("note"), true, &classes),
            Selection::Boundary
        );
        let ordinary_only = BTreeSet::from([ReclaimClassV1::Ordinary]);
        assert_eq!(
            selector.select(
                &Event::new("control.identity.access"),
                false,
                &ordinary_only
            ),
            Selection::Structural
        );
    }
}
