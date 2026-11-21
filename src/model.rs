use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Allow,
    Deny,
    Review,
}

impl Outcome {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Review => "review",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub outcome: Outcome,
    pub engine: String,
    pub rule: String,
    pub reason: String,
}

impl Decision {
    pub fn new(outcome: Outcome, rule: &str, reason: &str) -> Self {
        Self {
            outcome,
            engine: "deterministic".into(),
            rule: rule.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub request_id: String,
    pub profile: String,
    pub command: CommandSpec,
    pub project_id: Option<String>,
    pub sandbox_id: Option<String>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
    #[serde(default)]
    pub human_approved: bool,
    #[serde(default)]
    pub approval_hash: Option<String>,
    /// Absent on historical batch requests; present modes are part of the approval digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecutionModeV1>,
    /// A managed client choice is frozen before approval; omission retains legacy requests.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_provider_resolution"
    )]
    pub provider_resolution: Option<crate::provider::ProviderResolutionV1>,
}

pub(crate) fn deserialize_provider_resolution<'de, D>(
    deserializer: D,
) -> Result<Option<crate::provider::ProviderResolutionV1>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    crate::provider::ProviderResolutionV1::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionModeV1 {
    InteractiveTtyV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    pub timestamp: String,
    pub kind: String,
    pub source: String,
    pub provenance: String,
    pub coverage: String,
    pub session_id: Option<String>,
    pub project_id: Option<String>,
    pub sandbox_id: Option<String>,
    pub agent_id: Option<String>,
    pub parent_agent_id: Option<String>,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
    pub command: Option<CommandSpec>,
    pub decision: Option<Decision>,
    pub result: Option<Value>,
    #[serde(default)]
    pub data: Value,
}

impl Event {
    pub fn new(kind: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            kind: kind.into(),
            source: "gateway".into(),
            provenance: "observed".into(),
            coverage: "observed".into(),
            session_id: None,
            project_id: None,
            sandbox_id: None,
            agent_id: None,
            parent_agent_id: None,
            correlation_id: None,
            trace_id: None,
            command: None,
            decision: None,
            result: None,
            data: serde_json::json!({}),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            uuid::Uuid::parse_str(&self.id).is_ok(),
            "event id must be UUID"
        );
        chrono::DateTime::parse_from_rfc3339(&self.timestamp)?;
        anyhow::ensure!(
            !self.kind.is_empty()
                && self.kind.len() <= 80
                && self
                    .kind
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'),
            "invalid event kind"
        );
        anyhow::ensure!(
            ["observed", "declared", "inferred"].contains(&self.provenance.as_str()),
            "invalid provenance"
        );
        anyhow::ensure!(
            ["native", "observed", "generic", "partial"].contains(&self.coverage.as_str()),
            "invalid coverage"
        );
        anyhow::ensure!(self.source.len() <= 80, "invalid source");
        for field in [
            &self.session_id,
            &self.project_id,
            &self.sandbox_id,
            &self.agent_id,
            &self.parent_agent_id,
            &self.correlation_id,
            &self.trace_id,
        ]
        .into_iter()
        .flatten()
        {
            anyhow::ensure!(
                field.len() <= 256 && !field.chars().any(char::is_control),
                "invalid correlation field"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub received_at: String,
    pub prev_hash: String,
    pub hash: String,
    #[serde(flatten)]
    pub event: Event,
}

/// Match an admission namespace without changing the stored event kind.
/// A namespace includes its root and dot-separated descendants only.
pub(crate) fn event_kind_in_namespace(kind: &str, namespace: &str) -> bool {
    !namespace.is_empty()
        && kind
            .get(..namespace.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(namespace))
        && (kind.len() == namespace.len() || kind.as_bytes().get(namespace.len()) == Some(&b'.'))
}

#[cfg(test)]
mod tests {
    use super::event_kind_in_namespace;

    #[test]
    fn event_namespace_matches_ascii_case_and_dot_descendants() {
        for (namespace, kind) in [
            ("control", "control"),
            ("control", "CONTROL"),
            ("control", "Control.relay.x"),
            ("control", "control.RELAY.x"),
            ("security.decision", "SECURITY.decision"),
            ("security.decision", "security.DECISION.extra"),
            ("worker", "WORKER.start.claimed"),
            ("usage.allocation", "USAGE.Allocation.terminal"),
            ("relay_channel", "RELAY_CHANNEL"),
            ("relay_channel", "Relay_Channel.v1"),
        ] {
            assert!(event_kind_in_namespace(kind, namespace), "{kind}");
        }
    }

    #[test]
    fn event_namespace_preserves_other_names_and_unicode_boundaries() {
        for (namespace, kind) in [
            ("control", "Fixture.MixedCase"),
            ("control", "ControlPlane.relay.x"),
            ("security.decision", "Security.decision_reported"),
            ("control", "control_extra"),
            ("control", "control-other"),
            ("control", "control/relay"),
            ("control", "contro"),
            ("control", "cöntrol.relay.x"),
            ("control", "controℓ.relay.x"),
            ("relay_channel", "Relay_Channels.v1"),
            ("", "control.relay.x"),
        ] {
            assert!(!event_kind_in_namespace(kind, namespace), "{kind}");
        }
    }
}
