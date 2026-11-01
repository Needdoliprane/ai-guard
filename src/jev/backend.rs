use super::{JevClient, JevError, RuleCatalog, local::LocalClient, transport::bounded_json};
use crate::{
    model::{DecisionRequest, Outcome},
    redaction::Redactor,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    Cli,
    Environment,
    Legacy,
    Programmatic,
}
#[derive(Clone)]
enum Engine {
    TypeSafe(JevClient),
    RustRules(RuleCatalog),
    LocalOpenAI(LocalClient),
}
/// An additional veto after the mandatory Rust policy; never an execution permission.
#[derive(Clone)]
pub struct JevBackend {
    engine: Engine,
    source: SelectionSource,
}
pub(crate) struct BackendEvaluation {
    pub outcome: Outcome,
    pub metadata: Value,
}
impl JevBackend {
    pub fn typesafe(client: JevClient, source: SelectionSource) -> Self {
        Self {
            engine: Engine::TypeSafe(client),
            source,
        }
    }
    pub fn rust_rules(catalogue: RuleCatalog, source: SelectionSource) -> Self {
        Self {
            engine: Engine::RustRules(catalogue),
            source,
        }
    }
    pub fn local_openai(
        endpoint: &str,
        key: Option<String>,
        model: String,
        allow_local_http: bool,
        source: SelectionSource,
    ) -> Result<Self, JevError> {
        Ok(Self {
            engine: Engine::LocalOpenAI(LocalClient::new(endpoint, key, model, allow_local_http)?),
            source,
        })
    }
    pub fn id(&self) -> &'static str {
        match self.engine {
            Engine::TypeSafe(_) => "typesafe",
            Engine::RustRules(_) => "rust_rules",
            Engine::LocalOpenAI(_) => "local_openai",
        }
    }
    pub fn validate_control_credentials<'a>(
        &self,
        credentials: impl IntoIterator<Item = &'a str>,
    ) -> anyhow::Result<()> {
        let key = match &self.engine {
            Engine::TypeSafe(client) => client.transport.key(),
            Engine::LocalOpenAI(client) => client.transport.key(),
            Engine::RustRules(_) => None,
        };
        if let Some(key) = key {
            super::validate_credential_separation(key, credentials)?;
        }
        Ok(())
    }
    pub(crate) fn redactor(&self, base: &Redactor) -> Redactor {
        let key = match &self.engine {
            Engine::TypeSafe(client) => client.transport.key(),
            Engine::LocalOpenAI(client) => client.transport.key(),
            Engine::RustRules(_) => None,
        };
        base.union(&Redactor::new(key.map(str::to_owned).into_iter().collect()))
    }
    fn configuration(&self) -> Value {
        match &self.engine {
            Engine::TypeSafe(client) => {
                json!({"transport":client.transport.configuration(),"requested_model":client.model,"min_confidence":client.min_confidence})
            }
            Engine::RustRules(catalogue) => catalogue.configuration(),
            Engine::LocalOpenAI(client) => {
                json!({"transport":client.transport.configuration(),"requested_model":client.model,"decision_policy":"advisory_fail_closed"})
            }
        }
    }
    pub fn validate_redaction(&self, redactor: &Redactor) -> Result<(), JevError> {
        let configuration = self.configuration();
        if redactor.value_unclipped(&configuration) != configuration {
            return Err(JevError::InvalidConfiguration);
        }
        if let Engine::RustRules(catalogue) = &self.engine {
            catalogue.validate_redaction(redactor)?;
        }
        Ok(())
    }
    pub(crate) fn metadata(&self, status: &str) -> Value {
        let configuration = self.configuration();
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:decision-backend-config:v1\0");
        // Fixed, validated configuration is far below the transport budget.
        digest.update(configuration.to_string().as_bytes());
        let requested_model = configuration
            .get("requested_model")
            .cloned()
            .unwrap_or(Value::Null);
        json!({"backend":self.id(),"selection_source":self.source,"status":status,"config_sha256":hex::encode(digest.finalize()),"configuration":configuration,"requested_model":requested_model,"model":null,"confidence":null,"usage":null})
    }
    pub(crate) async fn evaluate(
        &self,
        request: &DecisionRequest,
        redactor: &Redactor,
    ) -> Result<BackendEvaluation, JevError> {
        let redactor = self.redactor(redactor);
        self.validate_redaction(&redactor)?;
        let (outcome, details) = match &self.engine {
            Engine::TypeSafe(client) => {
                let answer = client.decide(request, &redactor).await.map_err(|error| {
                    error
                        .downcast_ref::<JevError>()
                        .copied()
                        .unwrap_or(JevError::InvalidResponse)
                })?;
                let outcome = answer.outcome.clone();
                let mut details = redactor.value_unclipped(
                    &serde_json::to_value(answer).map_err(|_| JevError::InvalidResponse)?,
                );
                details["usage_provenance"] = if details["usage"].is_null() {
                    Value::Null
                } else {
                    json!("provider_declared")
                };
                details["confidence_provenance"] = json!("provider_declared_not_calibrated");
                // The durable store's historic redactor must not silently clip judge metadata.
                if redactor.value(&details) != details {
                    return Err(JevError::InvalidResponse);
                }
                (outcome, details)
            }
            Engine::RustRules(catalogue) => {
                // Even local evaluation uses the same masked serialized input budget.
                bounded_json(&super::masked_state(request, &redactor))?;
                let (outcome, reason) = catalogue.evaluate(request)?;
                (
                    outcome.clone(),
                    json!({"outcome":outcome,"reason":reason,"confidence":null,"model":null,"usage":null,"coverage":if reason == "validated_managed_bootstrap_structure_only" { "managed_bootstrap_structure_only_no_transitive_mediation" } else { "incomplete_exact_top_level_argv_only" }}),
                )
            }
            Engine::LocalOpenAI(client) => client.evaluate(request, &redactor).await?,
        };
        let mut metadata = self.metadata("evaluated");
        metadata
            .as_object_mut()
            .ok_or(JevError::InvalidResponse)?
            .extend(
                details
                    .as_object()
                    .ok_or(JevError::InvalidResponse)?
                    .clone(),
            );
        if redactor.value(&metadata) != metadata {
            return Err(JevError::InvalidResponse);
        }
        bounded_json(&metadata).map_err(|_| JevError::ResponseTooLarge)?;
        Ok(BackendEvaluation { outcome, metadata })
    }
}
