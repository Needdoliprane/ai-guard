//! Lifecycle of one engine CLI command as its caller sees it: completion,
//! failure, timeout, held output and cancellation, with a compiled Rust
//! stand-in for the engine CLI as the approved runtime program.
//!
//! Rust CLI fixtures only: no Docker or Podman engine is qualified by this target.
#![cfg(unix)]

#[path = "retained_cli_lifecycle/adapter.rs"]
mod adapter;
#[path = "retained_cli_lifecycle/contract.rs"]
mod contract;
#[path = "source_lifecycle/harness.rs"]
mod harness;
#[path = "retained_cli_lifecycle/reproductions.rs"]
mod reproductions;
#[path = "retained_cli_lifecycle/retained.rs"]
mod retained;
