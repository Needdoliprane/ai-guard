//! Trusted, broker-owned side of worker egress (direction C).
//!
//! The worker keeps `--network none`. Its untrusted loopback helper forwards
//! CONNECT requests over the broker-owned channel; this module owns every
//! upstream effect: durable decision first, then resolution with full address
//! validation, then a single connect to a pinned numeric address.

// Consumed by the broker executor hooks of the next slice.
#[allow(dead_code)]
pub(crate) mod channel;
#[allow(dead_code)]
pub(crate) mod resolve;
