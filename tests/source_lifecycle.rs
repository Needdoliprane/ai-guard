//! What `agentsandbox build`, `up` and `down` hand to the engine CLI, and how
//! long it stays available: operator commands end to end, with the real
//! `agentsandbox` binary driving a compiled Rust stand-in for the engine CLI.
//!
//! Rust CLI fixtures only: no Docker or Podman engine is qualified by this target.
#![cfg(unix)]

#[path = "source_lifecycle/harness.rs"]
mod harness;
#[path = "source_lifecycle/paths.rs"]
mod paths;
#[path = "source_lifecycle/regressions.rs"]
mod regressions;
#[path = "source_lifecycle/reproductions.rs"]
mod reproductions;
