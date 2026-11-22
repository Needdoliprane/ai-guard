//! Egress policy validation and the untrusted worker side of the broker-owned
//! egress channel. The worker helper never resolves names or opens an
//! upstream socket; the trusted broker owns those effects.
//!
//! A policy allowance and a validated candidate list are descriptive values.
//! Neither is an execution capability, durable audit receipt or DNS witness.

pub mod channel_helper;
pub mod channel_protocol;
pub mod frontend;
pub mod ids;
mod policy;
pub mod protocol;
pub mod proxy_env;
mod types;

pub use policy::{EgressPolicyV1, ValidatedResolution};
pub use protocol::Admission;
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
