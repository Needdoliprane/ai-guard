export type HumanView = "metadata" | "content";
export type ResourceKind = "session" | "sandbox" | "agent" | "project";
export type Capability = "read_metadata" | "read_content" | "export" | "read_tenant_metadata" | "manage_memberships" | "manage_grants" | "read_broker_registry" | "read_worker_registry";
export type Principal = { kind: "human_local"; installation_id: string; account_id: string } |
  { kind: "human_oidc"; issuer: string; subject: string };
export interface HumanContext {
  id: string; tenant_id: string; organization_scope: string; mode: "personal" | "administration";
  capabilities: Capability[]; membership_revision: string; authorization_version: string; expires_at: string;
}
export interface HumanIdentity {
  version: 1; principal: Principal; display_name: string;
  authentication: { method: "local_credential" | "oidc_access_token"; credential_id: string; expires_at: string | null };
  contexts: HumanContext[]; contexts_complete: boolean;
  /** A server wire profile, independent of the permissions of each context. Omitted by legacy controls. */
  read_profiles?: HumanReadProfiles;
}
export interface HumanReadProfiles {
  broker_registry: "human_broker_registry_owned_v1"; worker_registry: "unavailable"; federation_registry: "unavailable";
}
export interface ResourceRef { kind: ResourceKind; resource_id: string }
export interface HumanResource {
  resource: ResourceRef; owner: Principal; revision: string;
  permissions: { metadata: boolean; content: boolean; export: boolean };
}
export interface ReadReceipt { request_id: string; audit_event_id: string; received_at: string }
export interface HumanPage<T> {
  version: 1; context_id: string; authorization_version: string; items: T[];
  next_cursor: string | null; has_more: boolean; coverage: "authorized_selection"; read_receipt: ReadReceipt;
}
export interface HumanEvent {
  metadata: { id: string; timestamp: string; received_at: string; kind: string; source: string;
    provenance: "observed" | "declared" | "inferred"; coverage: "native" | "observed" | "generic" | "partial";
    resource: ResourceRef; decision: "allow" | "deny" | "review" | null };
  content: { command: unknown; decision: unknown; result: unknown; data: unknown } | null;
}
export interface HumanEventPage extends HumanPage<HumanEvent> { view: HumanView; selection_complete?: boolean }
export interface HumanSelection { view: HumanView; resource?: ResourceRef; cursor?: string | null; limit?: number }
export interface HumanBroker {
  broker_id: string; incarnation_id: string; grant_revision: string; heartbeat_revision: string;
  state: "unseen" | "current" | "stale" | "expired" | "revoked" | "clock_uncertain";
  authorities: { authority_id: string | null; runtime_kind: "docker" | "podman" | "kubernetes"; authority_id_redacted: boolean }[];
  granted_at: string; not_after: string; revoked_at: string | null;
  last_heartbeat: { incarnation_id: string; sequence: string; received_at: string; lease_expires_at: string;
    heartbeat_provenance: "declared"; reception_provenance: "observed" } | null;
  /** Present only after validating the distinct owned wire profile. Never inferred from legacy metadata. */
  owned?: { clock_uncertain: boolean; lease_seconds: number; checkpoint: HumanOwnedBrokerCheckpoint };
}
export type HumanOwnedBrokerCheckpoint = {
  status: "received"; received_at: string; lease_expires_at: string;
  freshness: "current" | "stale" | "historical" | "clock_uncertain"; in_current_grant: boolean;
  coverage: "declared_partial"; reference_count: number; has_more: boolean;
  publication_provenance: "declared"; reception_provenance: "observed";
} | { status: "absent" | "unknown"; received_at: null; lease_expires_at: null; freshness: null; in_current_grant: null;
  coverage: null; reference_count: null; has_more: null; publication_provenance: null; reception_provenance: null };
export interface HumanBrokerPage {
  version: 1; context_id: string; authorization_version: string; source: "tenant_broker_registry";
  consistency: "live_keyset"; evaluated_at: string; runtime_inventory: "unknown"; items: HumanBroker[];
  has_more: boolean; next_cursor: string | null; read_receipt: ReadReceipt;
  profile?: "human_broker_registry_owned_v1"; count_scope?: "page";
}

export class HumanContractError extends Error {
  constructor() { super("Contrat de consultation humaine incomplet ou incompatible. Vérifiez les versions du contrôle et du dashboard."); }
}
export function object(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
function requireContract(condition: unknown): asserts condition { if (!condition) throw new HumanContractError(); }
export function boundedText(value: unknown, max = 4096): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= max && !/[\u0000-\u001f\u007f]/.test(value);
}
export function opaque(value: unknown): value is string {
  return typeof value === "string" && /^[\x21-\x7e]{1,8192}$/.test(value) && !/^\d+$/.test(value);
}
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function uuid(value: unknown): value is string { return typeof value === "string" && uuidPattern.test(value); }
function timestamp(value: unknown): value is string {
  return typeof value === "string" && /^\d{4}-\d\d-\d\dT.+(?:Z|[+-]\d\d:\d\d)$/.test(value) && Number.isFinite(Date.parse(value));
}
function revision(value: unknown): value is string {
  return typeof value === "string" && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
}
function authorizationVersion(value: unknown): value is string { return typeof value === "string" && /^[0-9a-f]{64}$/.test(value); }
function principal(value: unknown): Principal {
  requireContract(object(value));
  if (value.kind === "human_local") {
    requireContract(Object.keys(value).every((key) => ["kind", "installation_id", "account_id"].includes(key)) && uuid(value.installation_id) && uuid(value.account_id));
    return { kind: "human_local", installation_id: value.installation_id, account_id: value.account_id };
  }
  requireContract(value.kind === "human_oidc" && Object.keys(value).every((key) => ["kind", "issuer", "subject"].includes(key)) &&
    utf8Text(value.issuer, 2048) && utf8Text(value.subject, 512));
  let issuer: URL;
  try { issuer = new URL(value.issuer); } catch { throw new HumanContractError(); }
  requireContract(["https:", "http:"].includes(issuer.protocol) && issuer.hostname &&
    !issuer.username && !issuer.password && !value.issuer.includes("?") && !value.issuer.includes("#"));
  return { kind: "human_oidc", issuer: value.issuer, subject: value.subject };
}
function utf8Text(value: unknown, max: number): value is string {
  return typeof value === "string" && value.trim().length > 0 && new TextEncoder().encode(value).length <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
}
function resource(value: unknown): ResourceRef {
  requireContract(object(value) && ["session", "sandbox", "agent", "project"].includes(String(value.kind)) && boundedText(value.resource_id));
  return { kind: value.kind as ResourceKind, resource_id: value.resource_id };
}
export function parseHumanIdentity(value: unknown): HumanIdentity {
  requireContract(object(value) && value.version === 1 && boundedText(value.display_name, 256) && object(value.authentication) &&
    ["local_credential", "oidc_access_token"].includes(String(value.authentication.method)) && uuid(value.authentication.credential_id) &&
    (value.authentication.expires_at === null || timestamp(value.authentication.expires_at)) && Array.isArray(value.contexts) && value.contexts.length <= 1000 &&
    typeof value.contexts_complete === "boolean");
  const profiles = Object.hasOwn(value, "read_profiles");
  if (profiles) {
    closed(value.read_profiles, ["broker_registry", "worker_registry", "federation_registry"]);
    requireContract(value.read_profiles.broker_registry === "human_broker_registry_owned_v1" &&
      value.read_profiles.worker_registry === "unavailable" && value.read_profiles.federation_registry === "unavailable");
  }
  const seen = new Set<string>();
  const contexts = value.contexts.map((item): HumanContext => {
    requireContract(object(item) && opaque(item.id) && !seen.has(item.id) && boundedText(item.tenant_id, 128) &&
      boundedText(item.organization_scope, 1024) && ["personal", "administration"].includes(String(item.mode)) &&
      revision(item.membership_revision) && authorizationVersion(item.authorization_version) && timestamp(item.expires_at) &&
      Array.isArray(item.capabilities) && item.capabilities.length <= 8 && new Set(item.capabilities).size === item.capabilities.length && item.capabilities.every((capability) =>
        ["read_metadata", "read_content", "export", "read_tenant_metadata", "manage_memberships", "manage_grants", "read_broker_registry", "read_worker_registry"].includes(String(capability))));
    seen.add(item.id);
    return { id: item.id, tenant_id: item.tenant_id, organization_scope: item.organization_scope,
      mode: item.mode as HumanContext["mode"], membership_revision: item.membership_revision,
      authorization_version: item.authorization_version, expires_at: item.expires_at, capabilities: item.capabilities as Capability[] };
  });
  const authenticatedPrincipal = principal(value.principal);
  requireContract((value.authentication.method === "local_credential" && authenticatedPrincipal.kind === "human_local") ||
    (value.authentication.method === "oidc_access_token" && authenticatedPrincipal.kind === "human_oidc" && timestamp(value.authentication.expires_at)));
  return { version: 1, principal: authenticatedPrincipal, display_name: value.display_name,
    authentication: { method: value.authentication.method as HumanIdentity["authentication"]["method"], credential_id: value.authentication.credential_id, expires_at: value.authentication.expires_at as string | null },
    contexts, contexts_complete: value.contexts_complete,
    ...(profiles ? { read_profiles: { broker_registry: "human_broker_registry_owned_v1" as const,
      worker_registry: "unavailable" as const, federation_registry: "unavailable" as const } } : {}) };
}
function page<T>(value: unknown, parse: (item: unknown) => T, streaming = false): HumanPage<T> {
  requireContract(object(value) && value.version === 1 && opaque(value.context_id) && authorizationVersion(value.authorization_version) &&
    value.coverage === "authorized_selection" && Array.isArray(value.items) && value.items.length <= 100 &&
    typeof value.has_more === "boolean" && (value.next_cursor === null || opaque(value.next_cursor)) &&
    (streaming ? value.next_cursor === null : value.has_more === (value.next_cursor !== null)) && object(value.read_receipt) &&
    uuid(value.read_receipt.request_id) && uuid(value.read_receipt.audit_event_id) && timestamp(value.read_receipt.received_at));
  return { version: 1, context_id: value.context_id, authorization_version: value.authorization_version,
    coverage: "authorized_selection", items: value.items.map(parse), next_cursor: value.next_cursor as string | null,
    has_more: value.has_more, read_receipt: { request_id: value.read_receipt.request_id,
      audit_event_id: value.read_receipt.audit_event_id, received_at: value.read_receipt.received_at } };
}
export function parseResourcePage(value: unknown): HumanPage<HumanResource> {
  return page(value, (item) => {
    requireContract(object(item) && revision(item.revision) && object(item.permissions) &&
      [item.permissions.metadata, item.permissions.content, item.permissions.export].every((permission) => typeof permission === "boolean"));
    return { resource: resource(item.resource), owner: principal(item.owner), revision: item.revision,
      permissions: { metadata: item.permissions.metadata as boolean, content: item.permissions.content as boolean, export: item.permissions.export as boolean } };
  });
}
function boundedJson(value: unknown) {
  const pending: { value: unknown; depth: number }[] = [{ value, depth: 0 }]; let nodes = 0;
  while (pending.length) {
    const item = pending.pop()!;
    requireContract(++nodes <= 20000 && item.depth <= 32);
    if (item.value === null || ["string", "boolean", "number"].includes(typeof item.value)) continue;
    requireContract(typeof item.value === "object");
    const values = Array.isArray(item.value) ? item.value : Object.values(item.value as object);
    requireContract(values.length <= 10000);
    for (const child of values) pending.push({ value: child, depth: item.depth + 1 });
  }
}
export function parseHumanEvent(value: unknown, view: HumanView): HumanEvent {
  requireContract(object(value) && object(value.metadata));
  const metadata = value.metadata;
  requireContract(Object.keys(metadata).every((key) => ["id", "timestamp", "received_at", "kind", "source", "provenance", "coverage", "resource", "decision"].includes(key)) &&
    uuid(metadata.id) && timestamp(metadata.timestamp) && timestamp(metadata.received_at) && boundedText(metadata.kind, 256) &&
    !metadata.kind.startsWith("control.identity.") && boundedText(metadata.source, 1024) &&
    ["observed", "declared", "inferred"].includes(String(metadata.provenance)) &&
    ["native", "observed", "generic", "partial"].includes(String(metadata.coverage)) &&
    (metadata.decision === null || ["allow", "deny", "review"].includes(String(metadata.decision))));
  const content = value.content;
  requireContract(content === null || (view === "content" && object(content) &&
    ["command", "decision", "result", "data"].every((key) => Object.hasOwn(content, key))));
  if (content !== null) boundedJson(content);
  return { metadata: { id: metadata.id, timestamp: metadata.timestamp, received_at: metadata.received_at,
    kind: metadata.kind, source: metadata.source, provenance: metadata.provenance as HumanEvent["metadata"]["provenance"],
    coverage: metadata.coverage as HumanEvent["metadata"]["coverage"], resource: resource(metadata.resource), decision: metadata.decision as HumanEvent["metadata"]["decision"] },
    content: content === null ? null : { command: (content as Record<string, unknown>).command,
      decision: (content as Record<string, unknown>).decision, result: (content as Record<string, unknown>).result, data: (content as Record<string, unknown>).data } };
}
export function parseHumanEventPage(value: unknown, exporting = false, transport: "rest" | "stream" = "rest"): HumanEventPage {
  requireContract(object(value) && ["metadata", "content"].includes(String(value.view)));
  const view = value.view as HumanView;
  const parsed = page(value, (item) => parseHumanEvent(item, view), transport === "stream");
  if (exporting) requireContract(typeof value.selection_complete === "boolean" && value.selection_complete === !parsed.has_more);
  return { ...parsed, view, ...(exporting ? { selection_complete: value.selection_complete as boolean } : {}) };
}
/** Server bound of one page. A cursor stays bound to the limit of its traversal. */
export const HUMAN_DEFAULT_PAGE_LIMIT = 25;
export const HUMAN_MAX_PAGE_LIMIT = 100;
export function pageLimit(value: number = HUMAN_DEFAULT_PAGE_LIMIT): string {
  requireContract(typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= HUMAN_MAX_PAGE_LIMIT);
  return String(value);
}
export function humanUrl(endpoint: "resources" | "events" | "export" | "stream", selection?: HumanSelection): string {
  requireContract(["resources", "events", "export", "stream"].includes(endpoint) &&
    (selection === undefined || ["metadata", "content"].includes(selection.view)));
  const query = new URLSearchParams({ limit: pageLimit(selection?.limit) });
  if (endpoint !== "resources") query.set("view", selection?.view ?? "metadata");
  if (selection?.resource) {
    const ref = resource(selection.resource);
    query.set("resource_kind", ref.kind); query.set("resource_id", ref.resource_id);
  }
  if (selection?.cursor != null) { requireContract(opaque(selection.cursor)); query.set("cursor", selection.cursor); }
  return `/api/v1/human/${endpoint}?${query}`;
}

/** Newest-first read. The explicit owned metadata profile has no live cursor. */
export interface HumanRecentEventPage {
  version: 1; context_id: string; authorization_version: string; view: HumanView; order: "newest_first"; coverage: "authorized_selection";
  items: HumanEvent[]; has_more: boolean; next_cursor: string | null; live_cursor: string | null; read_receipt: ReadReceipt;
  /** Current owned metadata coverage, never a permission or a generic server capability. */
  stream_mode?: "snapshot_only";
}
export function parseHumanRecentEventPage(value: unknown, first: boolean): HumanRecentEventPage {
  const snapshot = object(value) && Object.hasOwn(value, "stream_mode");
  closed(value, ["version", "context_id", "authorization_version", "view", "order", "coverage", "items", "has_more", "next_cursor", "live_cursor", "read_receipt", ...(snapshot ? ["stream_mode"] : [])]);
  requireContract(!snapshot || value.stream_mode === "snapshot_only");
  requireContract(value.version === 1 && opaque(value.context_id) && authorizationVersion(value.authorization_version) &&
    ["metadata", "content"].includes(String(value.view)) && value.order === "newest_first" && value.coverage === "authorized_selection" &&
    Array.isArray(value.items) && value.items.length <= HUMAN_MAX_PAGE_LIMIT && typeof value.has_more === "boolean" &&
    (value.next_cursor === null || opaque(value.next_cursor)) && value.has_more === (value.next_cursor !== null) &&
    (snapshot ? value.view === "metadata" && value.live_cursor === null :
      first ? opaque(value.live_cursor) && value.live_cursor !== value.next_cursor : value.live_cursor === null));
  closed(value.read_receipt, ["request_id", "audit_event_id", "received_at"]);
  requireContract(uuid(value.read_receipt.request_id) && uuid(value.read_receipt.audit_event_id) && timestamp(value.read_receipt.received_at));
  const view = value.view as HumanView;
  const items = value.items.map((item) => parseHumanEvent(item, view));
  requireContract(new Set(items.map((item) => item.metadata.id)).size === items.length);
  return { version: 1, context_id: value.context_id, authorization_version: value.authorization_version, view, order: "newest_first", coverage: "authorized_selection",
    ...(snapshot ? { stream_mode: "snapshot_only" as const } : {}),
    items, has_more: value.has_more, next_cursor: value.next_cursor as string | null, live_cursor: value.live_cursor as string | null,
    read_receipt: { request_id: value.read_receipt.request_id, audit_event_id: value.read_receipt.audit_event_id, received_at: value.read_receipt.received_at } };
}
export function humanRecentEventsUrl(selection: HumanSelection): string {
  return humanUrl("events", selection).replace("/api/v1/human/events?", "/api/v1/human/events/recent?");
}

function closed(value: unknown, fields: string[]): asserts value is Record<string, unknown> {
  requireContract(object(value) && Object.keys(value).length === fields.length && fields.every((field) => Object.hasOwn(value, field)));
}
function brokerUuid(value: unknown): value is string {
  return uuid(value) && value === value.toLowerCase() && value !== "00000000-0000-0000-0000-000000000000";
}
function brokerRevision(value: unknown, positive = false): value is string {
  return typeof value === "string" && /^(0|[1-9][0-9]{0,18})$/.test(value) && BigInt(value) <= 9223372036854775807n && (!positive || value !== "0");
}
function brokerAuthorityId(value: unknown): value is string {
  // BrokerAuthorityV1 allows non-empty whitespace IDs; do not apply profile/principal trimming rules here.
  return typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= 256 && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
}
/** Registry metadata has its own closed projection; resource ACL coverage does not apply. */
export function parseHumanBrokerPage(value: unknown, validateRegistryTime: (value: unknown) => value is string = timestamp): HumanBrokerPage {
  closed(value, ["version", "context_id", "authorization_version", "source", "consistency", "evaluated_at", "runtime_inventory", "items", "has_more", "next_cursor", "read_receipt"]);
  requireContract(value.version === 1 && opaque(value.context_id) && authorizationVersion(value.authorization_version) &&
    value.source === "tenant_broker_registry" && value.consistency === "live_keyset" && validateRegistryTime(value.evaluated_at) &&
    value.runtime_inventory === "unknown" && Array.isArray(value.items) && value.items.length <= 100 &&
    typeof value.has_more === "boolean" && (value.next_cursor === null || opaque(value.next_cursor)) && value.has_more === (value.next_cursor !== null));
  closed(value.read_receipt, ["request_id", "audit_event_id", "received_at"]);
  requireContract(brokerUuid(value.read_receipt.request_id) && brokerUuid(value.read_receipt.audit_event_id) && validateRegistryTime(value.read_receipt.received_at));
  const items = value.items.map((item): HumanBroker => {
    closed(item, ["broker_id", "incarnation_id", "grant_revision", "heartbeat_revision", "state", "authorities", "granted_at", "not_after", "revoked_at", "last_heartbeat"]);
    requireContract(brokerUuid(item.broker_id) && brokerUuid(item.incarnation_id) && brokerRevision(item.grant_revision, true) &&
      brokerRevision(item.heartbeat_revision) && ["unseen", "current", "stale", "expired", "revoked", "clock_uncertain"].includes(String(item.state)) &&
      validateRegistryTime(item.granted_at) && validateRegistryTime(item.not_after) && (item.revoked_at === null || validateRegistryTime(item.revoked_at)) &&
      Array.isArray(item.authorities) && item.authorities.length >= 1 && item.authorities.length <= 16);
    const authorities = item.authorities.map((authority): HumanBroker["authorities"][number] => {
      closed(authority, ["authority_id", "runtime_kind", "authority_id_redacted"]);
      requireContract(typeof authority.authority_id_redacted === "boolean" && ["docker", "podman", "kubernetes"].includes(String(authority.runtime_kind)) &&
        (authority.authority_id_redacted === true ? authority.authority_id === null : authority.authority_id_redacted === false && brokerAuthorityId(authority.authority_id)));
      return { authority_id: authority.authority_id as string | null, runtime_kind: authority.runtime_kind as HumanBroker["authorities"][number]["runtime_kind"], authority_id_redacted: authority.authority_id_redacted };
    });
    let heartbeat: HumanBroker["last_heartbeat"] = null;
    if (item.last_heartbeat !== null) {
      closed(item.last_heartbeat, ["incarnation_id", "sequence", "received_at", "lease_expires_at", "heartbeat_provenance", "reception_provenance"]);
      const h = item.last_heartbeat;
      requireContract(brokerUuid(h.incarnation_id) && brokerRevision(h.sequence, true) && validateRegistryTime(h.received_at) && validateRegistryTime(h.lease_expires_at) &&
        h.heartbeat_provenance === "declared" && h.reception_provenance === "observed");
      heartbeat = { incarnation_id: h.incarnation_id, sequence: h.sequence, received_at: h.received_at, lease_expires_at: h.lease_expires_at,
        heartbeat_provenance: "declared", reception_provenance: "observed" };
    }
    return { broker_id: item.broker_id, incarnation_id: item.incarnation_id, grant_revision: item.grant_revision,
      heartbeat_revision: item.heartbeat_revision, state: item.state as HumanBroker["state"], authorities,
      granted_at: item.granted_at, not_after: item.not_after, revoked_at: item.revoked_at as string | null, last_heartbeat: heartbeat };
  });
  requireContract(new Set(items.map((item) => item.broker_id)).size === items.length);
  return { version: 1, context_id: value.context_id, authorization_version: value.authorization_version, source: "tenant_broker_registry",
    consistency: "live_keyset", evaluated_at: value.evaluated_at, runtime_inventory: "unknown", items,
    has_more: value.has_more, next_cursor: value.next_cursor as string | null,
    read_receipt: { request_id: value.read_receipt.request_id, audit_event_id: value.read_receipt.audit_event_id, received_at: value.read_receipt.received_at } };
}
export function humanBrokersUrl(cursor: string | null = null, limit?: number): string {
  const query = new URLSearchParams({ limit: pageLimit(limit) });
  if (cursor !== null) { requireContract(opaque(cursor)); query.set("cursor", cursor); }
  return `/api/v1/human/brokers?${query}`;
}

export interface HumanWorkerAssociation {
  status: "registry_unavailable" | "authority_unknown" | "unregistered" | "registered";
  runtime_kind: "docker" | "podman" | "kubernetes" | null; runtime_matches: boolean | null; in_current_grant: boolean | null;
  broker: { broker_id: string; current_incarnation_id: string; state: HumanBroker["state"]; grant_revision: string;
    heartbeat_revision: string; last_received_at: string | null; lease_expires_at: string | null } | null;
}
export type HumanWorkerTimeStatus = "known" | "absent" | "unknown";
export interface HumanWorker {
  worker_id: string; request_id: string; attempt_id: string; revision: string;
  state: "reserved" | "preparing" | "created" | "start_issued" | "running" | "stopping" | "terminal" | "in_doubt" | "cancelled";
  quota_held: boolean; session_id: string; session_provenance: "declared"; runtime_kind: "docker" | "podman";
  cpu_millis: number; memory_bytes: string; pids_limit: number; timeout_seconds: number; network: "none" | "bridge";
  observed_started_at: string | null; observed_started_at_status: HumanWorkerTimeStatus; allocation_inconsistent: boolean; cleanup_removed: boolean | null;
  last_observation: { observed_at: string | null; observed_at_status: "known" | "unknown"; status: "created" | "running" | "exited" | "dead" | "unknown";
    running: boolean | null; started_at: string | null; started_at_status: HumanWorkerTimeStatus;
    finished_at: string | null; finished_at_status: HumanWorkerTimeStatus; exit_code: string | null;
    oom_killed: boolean | null; restart_count: number | null; provenance: "observed" } | null;
  launch_broker: HumanWorkerAssociation; observation_broker: HumanWorkerAssociation;
}
export interface HumanWorkerPage {
  version: 1; context_id: string; authorization_version: string; source: "tenant_worker_broker_registry";
  consistency: "snapshot_per_page"; evaluated_at: string; runtime_inventory: "unknown"; registry_status: "available";
  items: HumanWorker[]; has_more: boolean; next_cursor: string | null; read_receipt: ReadReceipt;
}
function workerUuid(value: unknown): value is string {
  return uuid(value) && value === value.toLowerCase();
}
/** Rust emits UTC with nanoseconds. Validate the calendar without Date.parse,
 * which cannot represent every RFC3339 value accepted by the historical Store. */
function canonicalWorkerTime(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const match = /^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)\.\d{9}Z$/.exec(value);
  if (!match) return false;
  const year = Number(match[1]); const month = Number(match[2]); const day = Number(match[3]);
  const hour = Number(match[4]); const minute = Number(match[5]); const second = Number(match[6]);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return month >= 1 && month <= 12 && day >= 1 && day <= (days[month - 1] ?? 0) &&
    hour <= 23 && minute <= 59 && second <= 60;
}
function workerTime(value: unknown, status: unknown, credibleStart = false) {
  return status === "known" ? canonicalWorkerTime(value) && (!credibleStart || value >= "1970-01-01T00:00:01.000000000Z") :
    (status === "absent" || status === "unknown") && value === null;
}
function workerExitCode(value: unknown): value is string {
  return typeof value === "string" && /^(?:0|[1-9][0-9]{0,18}|-[1-9][0-9]{0,18})$/.test(value) &&
    BigInt(value) >= -9223372036854775808n && BigInt(value) <= 9223372036854775807n;
}
function nullableTime(value: unknown) { return value === null || timestamp(value); }
function nullableBool(value: unknown) { return value === null || typeof value === "boolean"; }
function boundedInteger(value: unknown, max: number, min = 0): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= min && value <= max;
}
function workerAssociation(value: unknown): HumanWorkerAssociation {
  closed(value, ["status", "broker", "runtime_kind", "runtime_matches", "in_current_grant"]);
  if (value.status !== "registered") {
    requireContract(["registry_unavailable", "authority_unknown", "unregistered"].includes(String(value.status)) &&
      [value.broker, value.runtime_kind, value.runtime_matches, value.in_current_grant].every((field) => field === null));
  } else {
    requireContract(["docker", "podman", "kubernetes"].includes(String(value.runtime_kind)) &&
      typeof value.runtime_matches === "boolean" && typeof value.in_current_grant === "boolean");
    closed(value.broker, ["broker_id", "current_incarnation_id", "state", "grant_revision", "heartbeat_revision", "last_received_at", "lease_expires_at"]);
    const broker = value.broker;
    requireContract(brokerUuid(broker.broker_id) && brokerUuid(broker.current_incarnation_id) &&
      ["unseen", "current", "stale", "expired", "revoked", "clock_uncertain"].includes(String(broker.state)) &&
      brokerRevision(broker.grant_revision, true) && brokerRevision(broker.heartbeat_revision) &&
      nullableTime(broker.last_received_at) && nullableTime(broker.lease_expires_at) &&
      (broker.last_received_at === null) === (broker.lease_expires_at === null));
  }
  return structuredClone(value) as unknown as HumanWorkerAssociation;
}
export function parseHumanWorkerPage(value: unknown): HumanWorkerPage {
  closed(value, ["version", "context_id", "authorization_version", "source", "consistency", "evaluated_at", "runtime_inventory", "registry_status", "items", "has_more", "next_cursor", "read_receipt"]);
  requireContract(value.version === 1 && opaque(value.context_id) && authorizationVersion(value.authorization_version) &&
    value.source === "tenant_worker_broker_registry" && value.consistency === "snapshot_per_page" && timestamp(value.evaluated_at) &&
    value.runtime_inventory === "unknown" && value.registry_status === "available" &&
    Array.isArray(value.items) && value.items.length <= 100 && typeof value.has_more === "boolean" &&
    (value.next_cursor === null || opaque(value.next_cursor)) && value.has_more === (value.next_cursor !== null));
  closed(value.read_receipt, ["request_id", "audit_event_id", "received_at"]);
  requireContract(brokerUuid(value.read_receipt.request_id) && brokerUuid(value.read_receipt.audit_event_id) && timestamp(value.read_receipt.received_at));
  const items = value.items.map((item): HumanWorker => {
    closed(item, ["worker_id", "request_id", "attempt_id", "revision", "state", "quota_held", "session_id", "session_provenance", "runtime_kind", "cpu_millis", "memory_bytes", "pids_limit", "timeout_seconds", "network", "observed_started_at", "observed_started_at_status", "allocation_inconsistent", "cleanup_removed", "last_observation", "launch_broker", "observation_broker"]);
    requireContract([item.worker_id, item.request_id, item.attempt_id, item.session_id].every(workerUuid) &&
      brokerRevision(item.revision, true) && ["reserved", "preparing", "created", "start_issued", "running", "stopping", "terminal", "in_doubt", "cancelled"].includes(String(item.state)) &&
      typeof item.quota_held === "boolean" && item.session_provenance === "declared" &&
      ["docker", "podman"].includes(String(item.runtime_kind)) && boundedInteger(item.cpu_millis, 200000, 1) &&
      brokerRevision(item.memory_bytes, true) && BigInt(item.memory_bytes) <= 107374182400n &&
      boundedInteger(item.pids_limit, 4294967295, 1) && boundedInteger(item.timeout_seconds, 86400, 1) &&
      ["none", "bridge"].includes(String(item.network)) && workerTime(item.observed_started_at, item.observed_started_at_status, true) &&
      typeof item.allocation_inconsistent === "boolean" && nullableBool(item.cleanup_removed));
    if (item.last_observation !== null) {
      closed(item.last_observation, ["observed_at", "observed_at_status", "status", "running", "started_at", "started_at_status", "finished_at", "finished_at_status", "exit_code", "oom_killed", "restart_count", "provenance"]);
      const observation = item.last_observation;
      requireContract((observation.observed_at_status === "known" || observation.observed_at_status === "unknown") &&
        workerTime(observation.observed_at, observation.observed_at_status) && ["created", "running", "exited", "dead", "unknown"].includes(String(observation.status)) &&
        nullableBool(observation.running) && workerTime(observation.started_at, observation.started_at_status, true) && workerTime(observation.finished_at, observation.finished_at_status) &&
        (observation.exit_code === null || workerExitCode(observation.exit_code)) &&
        nullableBool(observation.oom_killed) && (observation.restart_count === null || boundedInteger(observation.restart_count, 4294967295)) &&
        observation.provenance === "observed");
    }
    const launch_broker = workerAssociation(item.launch_broker);
    const observation_broker = workerAssociation(item.observation_broker);
    return { ...structuredClone(item), launch_broker, observation_broker } as unknown as HumanWorker;
  });
  requireContract(new Set(items.map((item) => item.worker_id)).size === items.length);
  return { ...structuredClone(value), items } as unknown as HumanWorkerPage;
}
export function humanWorkersUrl(cursor: string | null = null, limit?: number): string {
  const query = new URLSearchParams({ limit: pageLimit(limit) });
  if (cursor !== null) { requireContract(opaque(cursor)); query.set("cursor", cursor); }
  return `/api/v1/human/workers?${query}`;
}

export const HUMAN_REGISTRY_PAGE_BYTES = 256 * 1024;
export const HUMAN_FEDERATION_PAGE_BYTES = 2 * HUMAN_REGISTRY_PAGE_BYTES + 4096;
export interface HumanFederationWorkerPage {
  version: 1; source: "tenant_federation_registry"; consistency: "independent_snapshots";
  coverage: "partial"; runtime_inventory: "unknown"; workers: HumanWorkerPage; brokers: HumanBrokerPage;
}
export interface HumanFederationSelection { workerCursor?: string | null; brokerCursor?: string | null; workerLimit?: number; brokerLimit?: number }
/** Presentation gating only. Rust remains the authorization authority for every read. */
export function canReadHumanFederation(context: HumanContext | null): boolean {
  return context?.mode === "administration" && context.capabilities.includes("read_worker_registry") && context.capabilities.includes("read_broker_registry");
}
export function parseHumanFederationWorkerPage(value: unknown): HumanFederationWorkerPage {
  closed(value, ["version", "source", "consistency", "coverage", "runtime_inventory", "workers", "brokers"]);
  requireContract(value.version === 1 && value.source === "tenant_federation_registry" && value.consistency === "independent_snapshots" &&
    value.coverage === "partial" && value.runtime_inventory === "unknown");
  // Both projections must decode before any part of the composition can be published.
  const workers = parseHumanWorkerPage(value.workers); const brokers = parseHumanBrokerPage(value.brokers);
  for (const page of [workers, brokers]) requireContract(new TextEncoder().encode(JSON.stringify(page)).byteLength <= HUMAN_REGISTRY_PAGE_BYTES);
  return { version: 1, source: "tenant_federation_registry", consistency: "independent_snapshots", coverage: "partial", runtime_inventory: "unknown", workers, brokers };
}
export function humanFederationWorkersUrl(selection: HumanFederationSelection = {}): string {
  const query = new URLSearchParams({ worker_limit: pageLimit(selection.workerLimit), broker_limit: pageLimit(selection.brokerLimit) });
  if (selection.workerCursor != null) { requireContract(opaque(selection.workerCursor)); query.set("worker_cursor", selection.workerCursor); }
  if (selection.brokerCursor != null) { requireContract(opaque(selection.brokerCursor)); query.set("broker_cursor", selection.brokerCursor); }
  return `/api/v1/human/federation/workers?${query}`;
}
