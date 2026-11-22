// Wire of the administration reads, for the client and view tests. Test data only; never imported by the application.
import { uuid, secondUuid } from "./human-fixtures.mjs";

const receipt = { request_id: uuid, audit_event_id: secondUuid, received_at: "2026-11-21T12:00:00Z" };
const anchors = (contextId) => ({ version: 1, context_id: contextId, authorization_version: "a".repeat(64), evaluated_at: "2026-11-21T12:00:00.000000000Z" });
export const membership = (extra = {}) => ({ membership_id: "00000000-0000-4000-8000-0000000000c1",
  principal: { kind: "human_oidc", issuer: "https://idp.example.test/realms/demo", subject: "user-1" }, display_name: "User One",
  account_active: true, organization_scope: "organization-a", capabilities: ["read_metadata"], revision: "3", active: true,
  credentials: [], credentials_complete: true, own: false, ...extra });
export const membershipPage = (contextId, extra = {}) => ({ ...anchors(contextId), source: "tenant_identity_memberships", consistency: "snapshot_per_page",
  items: [membership()], has_more: false, next_cursor: null, read_receipt: receipt, ...extra });
export const quotaBody = (contextId, extra = {}) => ({ ...anchors(contextId), source: "tenant_worker_reservations", max_timeout_seconds: "3600", read_receipt: receipt,
  quota: { max_active_workers: "100", max_cpu_millis: "200000", max_memory_bytes: "107374182400", held_workers: "2", held_cpu_millis: "2000", held_memory_bytes: "2147483648" }, ...extra });
export const usagePage = (contextId, extra = {}) => ({ ...anchors(contextId), source: "tenant_terminal_allocations", method_version: 1, billable: false,
  items: [], has_more: false, next_cursor: null, read_receipt: receipt, ...extra });
export const configurationBody = (contextId, extra = {}) => ({ ...anchors(contextId), read_receipt: receipt,
  entries: [{ axis: "storage", key: "backend", value: "sqlite", provenance: "configured" }], ...extra });
