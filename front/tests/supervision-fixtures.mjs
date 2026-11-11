// Test-only builders for the unified supervision tests. Never imported by the application.
import { broker, brokerPage, context, federationPage, page, resource, worker, workerObservation, workerPage, uuid, secondUuid } from "./human-fixtures.mjs";

export const hexId = (prefix, number) => `${prefix}0000000-0000-4000-8000-${number.toString(16).padStart(12, "0")}`;
export const workerId = (number) => hexId("1", number);
export const brokerId = (number) => hexId("2", number);
export const sessionId = (number) => hexId("3", number);
export const contextId = "context-supervision";
export const adminContext = (capabilities = ["read_metadata", "read_content", "export", "read_tenant_metadata", "read_broker_registry", "read_worker_registry"], extra = {}) =>
  ({ ...context(contextId, "administration"), capabilities, ...extra });
export const personalContext = (capabilities = ["read_metadata", "read_content", "export"], extra = {}) =>
  ({ ...context(contextId, "personal"), capabilities, ...extra });

export const registered = (id, extra = {}) => ({ status: "registered", runtime_kind: "docker", runtime_matches: true, in_current_grant: true,
  broker: { broker_id: id, current_incarnation_id: secondUuid, state: "current", grant_revision: "1", heartbeat_revision: "2",
    last_received_at: "2026-11-01T12:00:01Z", lease_expires_at: "2099-01-01T00:00:00Z", ...(extra.broker ?? {}) },
  ...Object.fromEntries(Object.entries(extra).filter(([key]) => key !== "broker")) });
export const unassociated = (status = "unregistered") => ({ status, broker: null, runtime_kind: null, runtime_matches: null, in_current_grant: null });

export const fleetWorker = (number, extra = {}) => worker({ worker_id: workerId(number), session_id: sessionId(number),
  launch_broker: unassociated(), observation_broker: unassociated("authority_unknown"), ...extra });
export const observedAt = (minute) => `2026-10-29T10:${String(minute).padStart(2, "0")}:00.000000000Z`;
export const observation = (minute, extra = {}) => workerObservation({ observed_at: observedAt(minute), ...extra });

export const workersPage = (items, extra = {}) => workerPage(items, { context_id: contextId, ...extra });
export const brokersPage = (items, extra = {}) => brokerPage(items, { context_id: contextId, ...extra });
export const fleetPage = (workers, brokers, workerExtra = {}, brokerExtra = {}) => {
  const frame = federationPage();
  frame.workers = workersPage(workers, workerExtra); frame.brokers = brokersPage(brokers, brokerExtra);
  return frame;
};
export const resourcesPage = (items, extra = {}) => page(contextId, items, extra);
export const sessionResource = (number, permissions) => resource(sessionId(number), permissions);
export const namedResource = (kind, id, permissions = { metadata: true, content: true, export: true }) =>
  ({ ...resource(id, permissions), resource: { kind, resource_id: id } });
export const eventsPage = (items, extra = {}) => page(contextId, items, { view: "metadata", ...extra });
export const eventOf = (number, ref, extra = {}) => ({ metadata: { id: hexId("4", number), timestamp: `2026-11-01T12:${String(number % 60).padStart(2, "0")}:00Z`,
  received_at: `2026-11-01T12:${String(number % 60).padStart(2, "0")}:01Z`, kind: "command.completed", source: "gateway",
  provenance: "observed", coverage: "native", resource: ref, decision: null, ...extra }, content: null });
export { broker, uuid, secondUuid };
export const recentPage = (items, extra = {}) => ({ version: 1, context_id: contextId, authorization_version: "a".repeat(64), view: "metadata",
  order: "newest_first", coverage: "authorized_selection", items, has_more: false, next_cursor: null, live_cursor: "live-cursor-opaque",
  read_receipt: { request_id: uuid, audit_event_id: secondUuid, received_at: "2026-11-01T12:00:02Z" }, ...extra });
/** The exact body a gateway answers for an event path that is not an event identifier (see tests/human_recent_events.rs). */
export const eventNotFound = () => ({ error: { code: "not_found", message: "The event is not available in this context.", retry_action: "review_permissions" } });
