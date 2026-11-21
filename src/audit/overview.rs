use serde_json::{Value, json};

#[derive(Default)]
pub(crate) struct EventCounts {
    pub(crate) commands: i64,
    pub(crate) allowed: i64,
    pub(crate) denied: i64,
    pub(crate) pending: i64,
    pub(crate) security: i64,
}

impl EventCounts {
    #[cfg(feature = "mysql-storage")]
    pub(crate) fn observe(&mut self, event: &crate::model::Event) {
        use crate::{audit::decision_codec, model::Outcome};

        match event.kind.as_str() {
            "command.requested" => self.commands += 1,
            decision_codec::KIND => match event.decision.as_ref().map(|decision| &decision.outcome)
            {
                Some(Outcome::Allow) => self.allowed += 1,
                Some(Outcome::Deny) => self.denied += 1,
                _ => {}
            },
            _ => {}
        }

        // Preserve SQLite's ASCII case-insensitive LIKE 'security.%'.
        const SECURITY_PREFIX: &str = "security.";
        if event
            .kind
            .get(..SECURITY_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(SECURITY_PREFIX))
        {
            self.security += 1;
        }
    }

    pub(crate) fn render(
        self,
        agents: &[Value],
        sandboxes: usize,
        sequence: i64,
        collector_healthy: bool,
    ) -> Value {
        let running = |provenance: &'static str| {
            agents.iter().filter(move |agent| {
                agent["status"] == "running" && agent["provenance"] == provenance
            })
        };
        json!({
            "counts": {
                "agents": running("observed").count(),
                "subagents": running("observed").filter(|agent| !agent["parent_id"].is_null()).count(),
                "declared_agents": running("declared").count(),
                "declared_subagents": running("declared").filter(|agent| !agent["parent_id"].is_null()).count(),
                "sandboxes": sandboxes,
                "commands": self.commands,
                "allowed": self.allowed,
                "denied": self.denied,
                "pending": self.pending,
                "security": self.security,
            },
            "health": {
                "collector": if collector_healthy { "ok" } else { "degraded" },
                "engine": "unknown",
                "jev": "disabled",
            },
            "resources": {"cpu_percent": null, "memory_bytes": null},
            "coverage": "observed",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "sequence": sequence,
        })
    }
}
