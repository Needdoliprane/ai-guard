//! Trusted execution-plane building blocks for broker supervision.
//!
//! Clock readings are observations, not authorization or start permissions.

pub mod checkpoint;
pub mod clock;
pub(crate) mod cold_terminal;
pub(crate) mod discovery;
#[doc(hidden)]
pub mod enforcer;
pub(crate) mod held_create_client;
pub mod intent;
pub mod journal;
pub mod lifecycle;
pub mod local_ipc;
pub mod outbox;
pub(crate) mod presence;
pub mod service;
pub(crate) mod terminal_facts;
pub mod writer;
