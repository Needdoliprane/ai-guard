export const uuid = "00000000-0000-4000-8000-000000000001";
export const secondUuid = "00000000-0000-4000-8000-000000000002";
export const principal = { kind: "human_local", installation_id: uuid, account_id: uuid };
export const context = (id = "context-a", mode = "personal") => ({ id, tenant_id: id === "context-b" ? "tenant-b" : "tenant-a",
  organization_scope: "organization-a", mode, capabilities: ["read_metadata", "read_content", "export"],
  membership_revision: "1", authorization_version: "a".repeat(64), expires_at: new Date(Date.now() + 240000).toISOString() });
export const identity = () => ({ version: 1, principal, display_name: "Alice locale",
  authentication: { method: "local_credential", credential_id: uuid, expires_at: null },
  contexts: [context(), context("context-admin", "administration"), context("context-b")], contexts_complete: true });
export const resource = (id = "session-a", permissions = { metadata: true, content: true, export: true }) => ({
  resource: { kind: "session", resource_id: id }, owner: principal, revision: "1", permissions,
});
export const event = (view = "metadata", id = "session-a", data = {}) => ({ metadata: { id: uuid,
  timestamp: "2026-11-01T12:00:00Z", received_at: "2026-11-01T12:00:01Z", kind: "command.completed", source: "fixture",
  provenance: "observed", coverage: "partial", resource: { kind: "session", resource_id: id }, decision: "allow" },
  content: view === "metadata" ? null : { command: { argv: ["printf", "safe"] }, decision: null, result: null, data } });
export const page = (contextId = "context-a", items = [], extra = {}) => ({ version: 1, context_id: contextId,
  authorization_version: "a".repeat(64), items, next_cursor: null, has_more: false, coverage: "authorized_selection",
  read_receipt: { request_id: uuid, audit_event_id: secondUuid, received_at: "2026-11-01T12:00:02Z" }, ...extra });
export const errorResponse = (status, code, retry_action = "retry") => Response.json({ error: { code, message: `Refus ${code}`, retry_action } }, { status });
export const brokerContext = (mixed = false) => ({ ...context("context-brokers", "administration"),
  capabilities: mixed ? ["read_metadata", "read_broker_registry"] : ["read_broker_registry"] });
export const brokerIdentity = (mixed = false) => ({ ...identity(), contexts: [brokerContext(mixed)] });
export const broker = (extra = {}) => ({ broker_id: uuid, incarnation_id: secondUuid,
  grant_revision: "9223372036854775807", heartbeat_revision: "2", state: "current",
  authorities: [{ authority_id: "authority-a", runtime_kind: "docker", authority_id_redacted: false }],
  granted_at: "2026-11-01T12:00:00Z", not_after: "2099-01-01T00:00:00Z", revoked_at: null,
  last_heartbeat: { incarnation_id: secondUuid, sequence: "2", received_at: "2026-11-01T12:00:01Z",
    lease_expires_at: "2099-01-01T00:00:00Z", heartbeat_provenance: "declared", reception_provenance: "observed" }, ...extra });
export const brokerPage = (items = [broker()], extra = {}) => ({ version: 1, context_id: "context-brokers",
  authorization_version: "a".repeat(64), source: "tenant_broker_registry", consistency: "live_keyset",
  evaluated_at: "2026-11-01T12:00:02Z", runtime_inventory: "unknown", items, has_more: false, next_cursor: null,
  read_receipt: { request_id: uuid, audit_event_id: secondUuid, received_at: "2026-11-01T12:00:02Z" }, ...extra });

export const workerIdentity = () => ({ ...identity(), contexts: [{ ...context("context-workers", "administration"), capabilities: ["read_worker_registry"] }] });
export const workerObservation = (extra = {}) => ({ observed_at: "2026-10-29T10:00:02.000000000Z", observed_at_status: "known",
  status: "exited", running: false, started_at: "2026-10-29T10:00:00.000000000Z", started_at_status: "known",
  finished_at: "2026-10-29T10:00:01.000000000Z", finished_at_status: "known", exit_code: "0", oom_killed: false, restart_count: 0, provenance: "observed", ...extra });
export const worker = (extra = {}) => ({ worker_id: uuid, request_id: secondUuid, attempt_id: uuid, revision: "9223372036854775807",
  state: "reserved", quota_held: true, session_id: secondUuid, session_provenance: "declared", runtime_kind: "docker",
  cpu_millis: 2000, memory_bytes: "1073741824", pids_limit: 256, timeout_seconds: 60, network: "none",
  observed_started_at: null, observed_started_at_status: "absent", allocation_inconsistent: false, cleanup_removed: null, last_observation: null,
  launch_broker: { status: "unregistered", broker: null, runtime_kind: null, runtime_matches: null, in_current_grant: null },
  observation_broker: { status: "authority_unknown", broker: null, runtime_kind: null, runtime_matches: null, in_current_grant: null }, ...extra });
export const workerPage = (items = [worker()], extra = {}) => ({ version: 1, context_id: "context-workers", authorization_version: "a".repeat(64),
  source: "tenant_worker_broker_registry", consistency: "snapshot_per_page", evaluated_at: "2026-11-01T12:00:02Z",
  runtime_inventory: "unknown", registry_status: "available", items, has_more: false, next_cursor: null,
  read_receipt: { request_id: uuid, audit_event_id: secondUuid, received_at: "2026-11-01T12:00:02Z" }, ...extra });

export const federationContext = () => ({ ...context("context-federation", "administration"),
  capabilities: ["read_worker_registry", "read_broker_registry"] });
export const federationIdentity = () => ({ ...identity(), contexts: [federationContext()] });
export const federationPage = (extra = {}) => ({ version: 1, source: "tenant_federation_registry", consistency: "independent_snapshots",
  coverage: "partial", runtime_inventory: "unknown",
  workers: workerPage(undefined, { context_id: "context-federation" }),
  brokers: brokerPage(undefined, { context_id: "context-federation", evaluated_at: "2026-11-01T12:00:03Z",
    read_receipt: { request_id: secondUuid, audit_event_id: uuid, received_at: "2026-11-01T12:00:03Z" } }), ...extra });

// A full requested page, with registry states and old-incarnation evidence kept distinct.
export const denseBrokerItems = () => {
  const id = (number) => `10000000-0000-4000-8000-${number.toString(16).padStart(12, "0")}`;
  const states = ["unseen", "current", "stale", "expired", "revoked", "clock_uncertain"];
  return Array.from({ length: 25 }, (_, index) => {
    const state = states[index % states.length]; const incarnation = id(index + 1001);
    const oldHeartbeat = state === "unseen" && index > 0;
    const hasHeartbeat = state !== "unseen" || oldHeartbeat;
    const redacted = index % 7 === 0;
    return broker({ broker_id: id(index + 1), incarnation_id: incarnation, state,
      heartbeat_revision: hasHeartbeat ? "2" : "0",
      authorities: [{ authority_id: redacted ? null : `authority-dense-${index + 1}`,
        authority_id_redacted: redacted, runtime_kind: ["docker", "podman", "kubernetes"][index % 3] }],
      granted_at: state === "clock_uncertain" ? "2026-11-01T12:00:03Z" : "2026-11-01T12:00:00Z",
      not_after: state === "expired" ? "2026-11-01T12:00:01Z" : "2099-01-01T00:00:00Z",
      revoked_at: state === "revoked" ? "2026-11-01T12:00:01Z" : null,
      last_heartbeat: hasHeartbeat ? { incarnation_id: oldHeartbeat ? id(index + 2001) : incarnation,
        sequence: "2", received_at: "2026-11-01T12:00:01Z", lease_expires_at: state === "stale" ? "2026-11-01T12:00:01Z" : "2099-01-01T00:00:00Z",
        heartbeat_provenance: "declared", reception_provenance: "observed" } : null });
  });
};
