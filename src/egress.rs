//! Local egress policy validation, without a resolver or socket-owning path.
//!
//! A policy allowance and a validated candidate list are descriptive values.
//! Neither is an execution capability, durable audit receipt or DNS witness.

pub mod frontend;
mod policy;
pub mod protocol;
mod types;

pub use policy::{EgressPolicyV1, ValidatedResolution};
pub use types::{
    Authority, AuthorityError, DenyReason, EgressTransport, PolicyAllowed, PolicyDecision,
    PolicyError, PolicyLimits, ResolutionError,
};

#[cfg(test)]
mod frontend_tests;
#[cfg(test)]
#[path = "egress/policy_tests.rs"]
mod policy_tests;
#[cfg(test)]
mod protocol_tests;
