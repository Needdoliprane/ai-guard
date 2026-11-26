//! Trusted execution-plane building blocks for broker supervision.
//!
//! Clock readings are observations, not authorization or start permissions.

pub mod checkpoint;
pub mod clock;
pub(crate) mod cold_terminal;
pub mod conversation;
pub(crate) mod discovery;
pub(crate) mod egress_owner;
#[doc(hidden)]
pub mod enforcer;
pub(crate) mod held_create_client;
pub mod intent;
pub mod journal;
pub mod lifecycle;
pub mod local_ipc;
pub mod outbox;
pub(crate) mod presence;
pub(crate) mod relay_owner;
pub(crate) mod retirement;
pub(crate) mod retirement_checkpoint;
pub mod service;
pub(crate) mod terminal_facts;
pub mod writer;
