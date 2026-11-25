//! Session control contracts shared by the catalogue and launch.

use super::{ControlFailure, invalid};
use uuid::Uuid;

/// The exact opaque target selected by the caller. The server resolves the
/// source and execution settings from its current target policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLaunchTargetSelectionV1 {
    launch_target_id: Uuid,
    expected_target_revision: u64,
}

impl SessionLaunchTargetSelectionV1 {
    pub fn new(launch_target_id: Uuid, expected_target_revision: u64) -> anyhow::Result<Self> {
        invalid(
            !launch_target_id.is_nil() && (1..=i64::MAX as u64).contains(&expected_target_revision),
        )?;
        Ok(Self {
            launch_target_id,
            expected_target_revision,
        })
    }

    /// Decode the canonical UUID and decimal revision used by the catalogue.
    pub fn parse(launch_target_id: &str, expected_target_revision: &str) -> anyhow::Result<Self> {
        let id = Uuid::parse_str(launch_target_id).map_err(|_| ControlFailure::InvalidRequest)?;
        invalid(
            id.hyphenated().to_string() == launch_target_id && expected_target_revision.len() <= 19,
        )?;
        let revision = expected_target_revision
            .parse::<u64>()
            .map_err(|_| ControlFailure::InvalidRequest)?;
        invalid(revision.to_string() == expected_target_revision)?;
        Self::new(id, revision)
    }

    pub fn launch_target_id(self) -> Uuid {
        self.launch_target_id
    }

    pub fn expected_target_revision(self) -> u64 {
        self.expected_target_revision
    }
}

#[cfg(test)]
#[path = "session_control_tests.rs"]
mod tests;
