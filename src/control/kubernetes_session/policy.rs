use super::{
    KubernetesSessionReservationIntentV2,
    validation::{canonical, digest, hash, require, uuid},
};
use crate::provider::{AuthMode, ClientKind, ClientTransport};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn client(value: ClientKind) -> &'static str {
    match value {
        ClientKind::Codex => "codex",
        ClientKind::ClaudeCode => "claude_code",
    }
}
fn auth(value: AuthMode) -> &'static str {
    match value {
        AuthMode::Api => "api",
        AuthMode::Subscription => "subscription",
    }
}
fn transport(value: ClientTransport) -> &'static str {
    match value {
        ClientTransport::Batch => "batch",
        ClientTransport::InteractiveTty => "interactive_tty",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationApprovalV2 {
    pub authority_id: String,
    pub authority_fingerprint: String,
    pub image: String,
    pub client: ClientKind,
    pub auth_mode: AuthMode,
    pub transport: ClientTransport,
    pub profile_id: String,
    pub profile_revision: u32,
}
impl KubernetesSessionReservationApprovalV2 {
    fn key(&self) -> (&str, &str, &str, &str, &str, &str, &str, u32) {
        (
            &self.authority_id,
            &self.authority_fingerprint,
            &self.image,
            client(self.client),
            auth(self.auth_mode),
            transport(self.transport),
            &self.profile_id,
            self.profile_revision,
        )
    }
    fn validate(&self) -> anyhow::Result<()> {
        uuid(&self.authority_id)?;
        hash(&self.authority_fingerprint)?;
        require(
            crate::runtime::kubernetes::valid_immutable_image(&self.image)
                && !self.profile_id.is_empty()
                && self.profile_id.len() <= 80
                && self
                    .profile_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && self.profile_revision > 0,
        )
    }
}

/// Server configuration only. A serialized policy is never an authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationPolicyV2 {
    pub version: u32,
    pub decision_ttl_seconds: u32,
    pub approvals: Vec<KubernetesSessionReservationApprovalV2>,
}
impl KubernetesSessionReservationPolicyV2 {
    pub fn canonicalized(&self) -> anyhow::Result<Self> {
        require(
            self.version == 2
                && (1..=60).contains(&self.decision_ttl_seconds)
                && self.approvals.len() <= 64,
        )?;
        let mut distinct = BTreeSet::new();
        for approval in &self.approvals {
            approval.validate()?;
            require(distinct.insert(approval.key()))?;
        }
        let mut policy = self.clone();
        policy
            .approvals
            .sort_by(|left, right| left.key().cmp(&right.key()));
        require(canonical(&policy)?.len() <= 16 * 1024)?;
        Ok(policy)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.canonicalized().map(|_| ())
    }
    pub fn policy_hash(&self) -> anyhow::Result<String> {
        digest(
            b"ai-guard:kubernetes-session-reservation-policy:v2\0",
            &self.canonicalized()?,
        )
    }
    pub fn approves(&self, intent: &KubernetesSessionReservationIntentV2) -> anyhow::Result<bool> {
        self.validate()?;
        intent.validate()?;
        let provider = intent.profile().retained_provider()?;
        let fingerprint = intent.authority().fingerprint();
        let authority_id = intent.authority().authority_id().to_string();
        Ok(self.approvals.iter().any(|approval| {
            approval.authority_id == authority_id
                && approval.authority_fingerprint == fingerprint
                && approval.image == intent.profile().retained_image()
                && approval.client == provider.client
                && approval.auth_mode == provider.auth_mode
                && approval.transport == provider.transport
                && approval.profile_id == provider.profile_id
                && approval.profile_revision == provider.profile_revision
        }))
    }
}

macro_rules! recovery_policy {
    ($name:ident, $domain:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub version: u32,
            pub decision_ttl_seconds: u32,
            pub allowed_broker_ids: Vec<String>,
        }
        impl $name {
            pub fn canonicalized(&self) -> anyhow::Result<Self> {
                require(
                    self.version == 2
                        && (1..=60).contains(&self.decision_ttl_seconds)
                        && self.allowed_broker_ids.len() <= 64,
                )?;
                let mut distinct = BTreeSet::new();
                for broker in &self.allowed_broker_ids {
                    uuid(broker)?;
                    require(distinct.insert(broker))?;
                }
                let mut policy = self.clone();
                policy.allowed_broker_ids.sort();
                require(canonical(&policy)?.len() <= 16 * 1024)?;
                Ok(policy)
            }
            pub fn validate(&self) -> anyhow::Result<()> {
                self.canonicalized().map(|_| ())
            }
            pub fn policy_hash(&self) -> anyhow::Result<String> {
                digest($domain, &self.canonicalized()?)
            }
            pub fn approves(&self, broker_id: &str) -> anyhow::Result<bool> {
                self.validate()?;
                uuid(broker_id)?;
                Ok(self.allowed_broker_ids.iter().any(|id| id == broker_id))
            }
        }
    };
}
recovery_policy!(
    KubernetesSessionRecoveryPolicyV2,
    b"ai-guard:kubernetes-session-recovery-policy:v2\0"
);
recovery_policy!(
    KubernetesSessionControlRecoveryPolicyV2,
    b"ai-guard:kubernetes-session-control-recovery-policy:v2\0"
);
