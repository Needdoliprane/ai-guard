export const uuid = "00000000-0000-4000-8000-000000000001";
export const secondUuid = "00000000-0000-4000-8000-000000000002";
export const principal = { kind: "human_local", installation_id: uuid, account_id: uuid };
export const context = (id = "context-a", mode = "personal") => ({ id, tenant_id: id === "context-b" ? "tenant-b" : "tenant-a",
  organization_scope: "organization-a", mode, capabilities: ["read_metadata", "read_content", "export"],
  membership_revision: "1", authorization_version: "a".repeat(64), expires_at: new Date(Date.now() + 240000).toISOString() });
export const identity = () => ({ version: 1, principal, display_name: "Alice locale",
  authentication: { method: "local_credential", credential_id: uuid, expires_at: null },
  contexts: [context(), context("context-admin", "administration"), context("context-b")] });
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
