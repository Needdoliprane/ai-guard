//! Operator retention policy. There is no implicit default: every command
//! that selects a range reads an explicit private policy file.
use super::{
    classify::ReclaimClassV1,
    failure::{RetentionFailure, code},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

/// Retention never reclaims events received during the last 24 hours, so that
/// a producer retry of a reclaimed event identifier stays inside the window in
/// which the live store still detects the duplicate.
pub const MIN_AGE_FLOOR_SECONDS: u64 = 86_400;
const MAX_AGE_SECONDS: u64 = 100 * 366 * 86_400;
const MAX_POLICY_BYTES: u64 = 16 * 1024;
pub const MAX_SEGMENT_EVENTS: u64 = 1_000_000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditRetentionPolicyV1 {
    pub version: u32,
    /// Only events whose highest preceding receive time is at least this old.
    pub min_age_seconds: u64,
    /// Events closest to the head that a segment always leaves untouched.
    pub keep_recent_events: u64,
    /// Upper bound of one segment, in archived events.
    pub max_segment_events: u64,
    /// Reclaimable classes, in their canonical order.
    pub classes: Vec<ReclaimClassV1>,
}

impl AuditRetentionPolicyV1 {
    pub fn validate(&self) -> Result<(), RetentionFailure> {
        let canonical: Vec<ReclaimClassV1> = self.class_set().into_iter().collect();
        if self.version != 1
            || !(MIN_AGE_FLOOR_SECONDS..=MAX_AGE_SECONDS).contains(&self.min_age_seconds)
            || !(1..=MAX_SEGMENT_EVENTS).contains(&self.keep_recent_events)
            || !(1..=MAX_SEGMENT_EVENTS).contains(&self.max_segment_events)
            || self.classes.is_empty()
            || canonical != self.classes
        {
            return Err(RetentionFailure::new(
                code::POLICY_INVALID,
                "the retention policy must be version 1, keep at least 24 hours and list distinct classes in canonical order",
            )
            .with("min_age_floor_seconds", MIN_AGE_FLOOR_SECONDS));
        }
        Ok(())
    }

    pub fn class_set(&self) -> BTreeSet<ReclaimClassV1> {
        self.classes.iter().copied().collect()
    }

    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:audit-retention-policy:v1\0");
        digest.update(serde_json::to_vec(self).expect("serializable policy"));
        hex::encode(digest.finalize())
    }

    /// Read and validate a private policy file.
    pub fn load(path: &Path) -> Result<Self, RetentionFailure> {
        let invalid = |message: &str, cause: String| {
            RetentionFailure::new(code::POLICY_INVALID, message)
                .with("path", path.display().to_string())
                .with("cause", cause)
        };
        let absolute = crate::control_fs::absolute_path(path)
            .map_err(|error| invalid("the policy path is not usable", error.to_string()))?;
        let held =
            crate::control_fs::PrivateFile::open(&absolute, MAX_POLICY_BYTES).map_err(|error| {
                invalid(
                    "the policy must be a private file of at most 16 KiB",
                    error.to_string(),
                )
            })?;
        let policy: Self = serde_json::from_slice(held.bytes())
            .map_err(|error| invalid("the policy is not a version 1 policy", error.to_string()))?;
        held.revalidate()
            .map_err(|error| invalid("the policy changed while it was read", error.to_string()))?;
        policy.validate()?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(min_age_seconds: u64, classes: Vec<ReclaimClassV1>) -> AuditRetentionPolicyV1 {
        AuditRetentionPolicyV1 {
            version: 1,
            min_age_seconds,
            keep_recent_events: 10,
            max_segment_events: 1000,
            classes,
        }
    }

    #[test]
    fn the_policy_keeps_at_least_a_day_and_canonical_classes() {
        let both = vec![ReclaimClassV1::Ordinary, ReclaimClassV1::HumanRead];
        assert!(policy(86_400, both.clone()).validate().is_ok());
        assert_eq!(
            policy(86_399, both.clone()).validate().unwrap_err().code,
            code::POLICY_INVALID
        );
        assert!(
            policy(
                86_400,
                vec![ReclaimClassV1::HumanRead, ReclaimClassV1::Ordinary]
            )
            .validate()
            .is_err()
        );
        assert!(
            policy(
                86_400,
                vec![ReclaimClassV1::Ordinary, ReclaimClassV1::Ordinary]
            )
            .validate()
            .is_err()
        );
        assert!(policy(86_400, vec![]).validate().is_err());
        assert_ne!(
            policy(86_400, both.clone()).fingerprint(),
            policy(86_401, both).fingerprint()
        );
    }
}
