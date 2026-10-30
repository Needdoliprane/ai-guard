//! Trusted execution-plane building blocks for broker supervision.
//!
//! Clock readings are observations, not authorization or start permissions.

pub mod checkpoint;
pub mod clock;
pub mod intent;
pub mod journal;
