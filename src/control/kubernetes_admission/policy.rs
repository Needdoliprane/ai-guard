//! Descriptive tenant policy; serialized policy never grants emission authority.
use super::{
    KUBERNETES_HELD_PROFILE_V1, KubernetesCreateFailureV1, KubernetesCreateIntentV1,
    validation::canonical_hash,
};
use crate::runtime::kubernetes::valid_immutable_image;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesHeldCreateApprovalV1 {
    pub authority_fingerprint: String,
    pub image: String,
    pub profile: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesHeldCreatePolicyV1 {
    pub version: u32,
    pub decision_ttl_seconds: u32,
    pub approvals: Vec<KubernetesHeldCreateApprovalV1>,
}

impl KubernetesHeldCreatePolicyV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.version != 1
            || !(1..=300).contains(&self.decision_ttl_seconds)
            || !(1..=64).contains(&self.approvals.len())
        {
            return Err(KubernetesCreateFailureV1::InvalidRequest.into());
        }
        let mut distinct = BTreeSet::new();
        for approval in &self.approvals {
            crate::control::hash(&approval.authority_fingerprint)
                .map_err(|_| KubernetesCreateFailureV1::InvalidRequest)?;
            if !valid_immutable_image(&approval.image)
                || approval.profile != KUBERNETES_HELD_PROFILE_V1
                || !distinct.insert((
                    approval.authority_fingerprint.as_str(),
                    approval.image.as_str(),
                    approval.profile.as_str(),
                ))
            {
                return Err(KubernetesCreateFailureV1::InvalidRequest.into());
            }
        }
        // Sorting keys and approvals changes order, not the compact byte length.
        let bytes =
            serde_json::to_vec(self).map_err(|_| KubernetesCreateFailureV1::InvalidRequest)?;
        if bytes.len() > 16 * 1024 {
            return Err(KubernetesCreateFailureV1::InvalidRequest.into());
        }
        Ok(())
    }

    pub fn canonicalized(&self) -> anyhow::Result<Self> {
        self.validate()?;
        let mut canonical = self.clone();
        canonical.approvals.sort_by(|left, right| {
            (&left.authority_fingerprint, &left.image, &left.profile).cmp(&(
                &right.authority_fingerprint,
                &right.image,
                &right.profile,
            ))
        });
        Ok(canonical)
    }

    pub fn policy_hash(&self) -> anyhow::Result<String> {
        canonical_hash(
            b"ai-guard:kubernetes-held-create-policy:v1\0",
            &self.canonicalized()?,
        )
    }

    pub fn approves(&self, intent: &KubernetesCreateIntentV1) -> anyhow::Result<bool> {
        self.validate()?;
        intent.validate()?;
        let authority_fingerprint = intent.authority.fingerprint();
        Ok(self.approvals.iter().any(|approval| {
            approval.authority_fingerprint == authority_fingerprint
                && approval.image == intent.image
                && approval.profile == intent.profile
        }))
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
