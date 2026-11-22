//! Worker-local collection channel for provider activity hooks (protocol v1).
//!
//! Claude Code and Codex CLI run inside the untrusted worker. Their managed
//! hook configuration, baked on the read-only image root, invokes the fixed
//! command `ai-guard-activity hook-v1 <client> <event>`. That short-lived hook
//! client forwards the hook's stdin to the worker-local helper started by the
//! broker (`ai-guard-activity serve-v1`), which relays it over its exec stdio
//! to the broker and routes the broker's acknowledgement back.
//!
//! Provenance and limits:
//! - A hook submission is what the provider reported to its own hook. It is
//!   `declared` provider activity, never an independent observation of an
//!   effect, and the payload text is untrusted input.
//! - The hook client, the helper and the agent share UID 1000. File modes and
//!   the peer UID check are transport hygiene, not a boundary against the
//!   agent: a same-UID agent can stop the helper, remove its socket or answer
//!   hooks itself. The broker must therefore treat a missing, closed or
//!   silent channel as an explicit loss of coverage, never as "no activity".
//! - A gate reply `recorded` means only that the broker acknowledged the
//!   submission. It is not a permission decision and the client never prints
//!   one.
//!
//! No credential, environment value, network address, tenant, audit or
//! runtime invocation is read or produced here.
pub mod helper;
pub mod hook;
pub mod wire;

/// Fixed worker-local root. Production code never takes it from argv or env.
pub const WORKER_ACTIVITY_ROOT: &str = "/home/agent/.ai-guard-activity";
/// Fixed socket name under the root, bound by the helper only.
pub const SOCKET_NAME: &str = "activity-v1.sock";
/// Private directory under the root where hook clients leave loss markers.
pub const LOSS_DIRECTORY: &str = "loss";

#[cfg(test)]
mod tests;
