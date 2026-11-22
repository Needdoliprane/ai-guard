//! Live collection of provider activity for supervised Claude Code and Codex
//! sessions: durable broker-local spool, collector, stdout framing and gateway
//! forwarding. Every provider record stays a provider declaration; the
//! collector observes only its own channels, budgets and losses.
//!
//! Interface: create an `ActivitySpool` in a private control directory, wrap
//! it in an `ActivityCollector`, feed it from the stdout tap (`stdout::pump`)
//! and the worker hook channel (`ActivityCollector::hook`), run
//! `forwarder::forward` until the execution ends, then `finish`.

pub mod collector;
pub mod forwarder;
pub mod hooks;
pub mod spool;
pub mod stdout;

pub use collector::{
    ActivityChannel, ActivityClient, ActivityCollector, ActivityContext, ActivityLimits,
    ActivitySummary, Admission,
};
pub use forwarder::{ActivitySink, ForwardOutcome, forward};
pub use hooks::{HookAck, HookFrame, HookPumpOutcome, HookTransport, pump_hooks};
pub use spool::{ActivitySpool, SpoolRecovery, SpooledEvent};
pub use stdout::{
    LineFramer, PumpOutcome, StdoutTap, TapReceiver, pump, stdout_tap, stdout_tap_with_stall,
};
