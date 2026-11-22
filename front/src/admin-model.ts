// Strict models of the administration reads of the 360° views: memberships, quota, usage and configuration.
// Pure: no DOM, no network, no clock. The control decides every value; the front never derives a right from them.
import { CAPABILITIES, HumanContractError, object, uuid, type Capability, type Principal, type ReadReceipt } from "./human-model.ts";
import type { SummaryQuota } from "./summary-model.ts";

function require(condition: unknown): asserts condition { if (!condition) throw new HumanContractError(); }
function closed(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  require(object(value) && Object.keys(value).length === fields.length && fields.every((field) => Object.hasOwn(value, field)));
}
const decimal = (value: unknown): value is string => typeof value === "string" && /^(0|[1-9][0-9]{0,38})$/.test(value);
const instant = (value: unknown): value is string =>
  typeof value === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{1,9})?(Z|[+-]\d\d:\d\d)$/.test(value) && Number.isFinite(Date.parse(value));
const hex64 = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" && value.trim().length > 0 && new TextEncoder().encode(value).length <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
const cursor = (value: unknown) => value === null || (typeof value === "string" && /^[0-9a-f]{16,8192}$/.test(value));
function receipt(value: unknown): ReadReceipt {
  closed(value, ["request_id", "audit_event_id", "received_at"]);
  require(uuid(value.request_id) && uuid(value.audit_event_id) && instant(value.received_at));
  return { request_id: value.request_id, audit_event_id: value.audit_event_id, received_at: value.received_at };
}
function frame(value: Record<string, unknown>, expected: { contextId: string; authorizationVersion: string }) {
  require(value.version === 1 && value.context_id === expected.contextId && value.authorization_version === expected.authorizationVersion &&
    hex64(value.context_id) && hex64(value.authorization_version) && instant(value.evaluated_at));
}
function principal(value: unknown): Principal {
  require(object(value));
  if (value.kind === "human_local") {
    closed(value, ["kind", "installation_id", "account_id"]);
    require(uuid(value.installation_id) && uuid(value.account_id));
    return { kind: "human_local", installation_id: value.installation_id, account_id: value.account_id };
  }
  closed(value, ["kind", "issuer", "subject"]);
  require(value.kind === "human_oidc" && text(value.issuer, 2048) && text(value.subject, 512));
  return { kind: "human_oidc", issuer: value.issuer, subject: value.subject };
}

export interface MemberCredential { credential_id: string; active: boolean; expires_at: string | null }
export interface Membership {
  membership_id: string; principal: Principal; display_name: string; account_active: boolean; organization_scope: string;
  capabilities: Capability[]; revision: string; active: boolean; credentials: MemberCredential[]; credentials_complete: boolean; own: boolean;
}
export interface MembershipPage {
  version: 1; context_id: string; authorization_version: string; source: "tenant_identity_memberships"; consistency: "snapshot_per_page";
  evaluated_at: string; items: Membership[]; has_more: boolean; next_cursor: string | null; read_receipt: ReadReceipt;
}
export function parseMembershipPage(value: unknown, expected: { contextId: string; authorizationVersion: string }): MembershipPage {
  closed(value, ["version", "context_id", "authorization_version", "source", "consistency", "evaluated_at", "items", "has_more", "next_cursor", "read_receipt"]);
  frame(value, expected);
  require(value.source === "tenant_identity_memberships" && value.consistency === "snapshot_per_page" && Array.isArray(value.items) &&
    value.items.length <= 100 && typeof value.has_more === "boolean" && cursor(value.next_cursor) && (value.next_cursor !== null) === value.has_more);
  const seen = new Set<string>();
  const items = value.items.map((item): Membership => {
    closed(item, ["membership_id", "principal", "display_name", "account_active", "organization_scope", "capabilities", "revision", "active", "credentials", "credentials_complete", "own"]);
    require(uuid(item.membership_id) && !seen.has(item.membership_id) && text(item.display_name, 160) && typeof item.account_active === "boolean" &&
      text(item.organization_scope, 128) && Array.isArray(item.capabilities) && new Set(item.capabilities).size === item.capabilities.length &&
      item.capabilities.every((capability) => (CAPABILITIES as readonly unknown[]).includes(capability)) && decimal(item.revision) && item.revision !== "0" &&
      typeof item.active === "boolean" && Array.isArray(item.credentials) && item.credentials.length <= 16 &&
      typeof item.credentials_complete === "boolean" && typeof item.own === "boolean");
    seen.add(item.membership_id);
    const credentials = item.credentials.map((credential): MemberCredential => {
      closed(credential, ["credential_id", "active", "expires_at"]);
      require(uuid(credential.credential_id) && typeof credential.active === "boolean" && (credential.expires_at === null || instant(credential.expires_at)));
      return { credential_id: credential.credential_id, active: credential.active, expires_at: credential.expires_at as string | null };
    });
    return { membership_id: item.membership_id, principal: principal(item.principal), display_name: item.display_name, account_active: item.account_active,
      organization_scope: item.organization_scope, capabilities: item.capabilities as Capability[], revision: item.revision, active: item.active,
      credentials, credentials_complete: item.credentials_complete, own: item.own };
  });
  return { version: 1, context_id: value.context_id as string, authorization_version: value.authorization_version as string,
    source: "tenant_identity_memberships", consistency: "snapshot_per_page", evaluated_at: value.evaluated_at as string, items,
    has_more: value.has_more, next_cursor: value.next_cursor as string | null, read_receipt: receipt(value.read_receipt) };
}

export interface HumanQuota { evaluated_at: string; source: "tenant_worker_reservations"; quota: SummaryQuota; max_timeout_seconds: string; read_receipt: ReadReceipt }
export function parseHumanQuota(value: unknown, expected: { contextId: string; authorizationVersion: string }): HumanQuota {
  closed(value, ["version", "context_id", "authorization_version", "evaluated_at", "source", "quota", "max_timeout_seconds", "read_receipt"]);
  frame(value, expected);
  const fields = ["max_active_workers", "max_cpu_millis", "max_memory_bytes", "held_workers", "held_cpu_millis", "held_memory_bytes"] as const;
  closed(value.quota, fields);
  const quota = value.quota;
  require(value.source === "tenant_worker_reservations" && fields.every((field) => decimal(quota[field])) && decimal(value.max_timeout_seconds));
  return { evaluated_at: value.evaluated_at as string, source: "tenant_worker_reservations", max_timeout_seconds: value.max_timeout_seconds,
    quota: Object.fromEntries(fields.map((field) => [field, quota[field] as string])) as unknown as SummaryQuota, read_receipt: receipt(value.read_receipt) };
}

export interface Allocation {
  attempt_id: string; worker_id: string; terminal_observation_id: string; started_at: string; finished_at: string;
  duration_ns: string; cpu_millicore_nanoseconds: string; memory_byte_nanoseconds: string; method_version: 1; billable: false;
  source: { authority_id: string; runtime_kind: "docker" | "podman"; container_id: string; spec_hash: string; provenance: "observed" };
}
export interface UsagePage { evaluated_at: string; items: Allocation[]; has_more: boolean; next_cursor: string | null; read_receipt: ReadReceipt }
export function parseUsagePage(value: unknown, expected: { contextId: string; authorizationVersion: string }): UsagePage {
  closed(value, ["version", "context_id", "authorization_version", "evaluated_at", "source", "method_version", "billable", "items", "has_more", "next_cursor", "read_receipt"]);
  frame(value, expected);
  require(value.source === "tenant_terminal_allocations" && value.method_version === 1 && value.billable === false && Array.isArray(value.items) &&
    value.items.length <= 100 && typeof value.has_more === "boolean" && cursor(value.next_cursor) && (value.next_cursor !== null) === value.has_more);
  const items = value.items.map((item): Allocation => {
    closed(item, ["attempt_id", "worker_id", "terminal_observation_id", "started_at", "finished_at", "duration_ns", "cpu_millicore_nanoseconds",
      "memory_byte_nanoseconds", "method_version", "billable", "source"]);
    closed(item.source, ["authority_id", "runtime_kind", "container_id", "spec_hash", "provenance"]);
    const source = item.source;
    require(uuid(item.attempt_id) && uuid(item.worker_id) && text(item.terminal_observation_id, 256) && instant(item.started_at) && instant(item.finished_at) &&
      decimal(item.duration_ns) && decimal(item.cpu_millicore_nanoseconds) && decimal(item.memory_byte_nanoseconds) && item.method_version === 1 &&
      item.billable === false && text(source.authority_id, 256) && (source.runtime_kind === "docker" || source.runtime_kind === "podman") &&
      text(source.container_id, 256) && /^[0-9a-f]{64}$/.test(String(source.spec_hash)) && source.provenance === "observed");
    return { attempt_id: item.attempt_id, worker_id: item.worker_id, terminal_observation_id: item.terminal_observation_id, started_at: item.started_at,
      finished_at: item.finished_at, duration_ns: item.duration_ns, cpu_millicore_nanoseconds: item.cpu_millicore_nanoseconds,
      memory_byte_nanoseconds: item.memory_byte_nanoseconds, method_version: 1, billable: false,
      source: { authority_id: source.authority_id as string, runtime_kind: source.runtime_kind, container_id: source.container_id as string,
        spec_hash: source.spec_hash as string, provenance: "observed" } };
  });
  return { evaluated_at: value.evaluated_at as string, items, has_more: value.has_more, next_cursor: value.next_cursor as string | null, read_receipt: receipt(value.read_receipt) };
}

export const CONFIGURATION_KEYS = [["build", "product_version"], ["topology", "source_profile"], ["identity", "local_credentials"], ["identity", "oidc"],
  ["identity", "oidc_transport"], ["storage", "backend"], ["storage", "tls_profile"], ["storage", "store_format"], ["storage", "human_rights_marker"],
  ["decision", "engine"], ["decision", "selection"], ["runtime", "declared_runtime_kinds"], ["runtime", "kubernetes_held_create"],
  ["egress", "mode"], ["providers", "availability"]] as const;
export type ConfigurationProvenance = "configured" | "default" | "derived" | "declared" | "unknown";
export interface ConfigurationEntry { axis: string; key: string; value: string; provenance: ConfigurationProvenance }
export interface HumanConfiguration { evaluated_at: string; entries: ConfigurationEntry[]; read_receipt: ReadReceipt }
export function parseHumanConfiguration(value: unknown, expected: { contextId: string; authorizationVersion: string }): HumanConfiguration {
  closed(value, ["version", "context_id", "authorization_version", "evaluated_at", "entries", "read_receipt"]);
  frame(value, expected);
  require(Array.isArray(value.entries) && value.entries.length <= CONFIGURATION_KEYS.length);
  const entries = value.entries.map((entry): ConfigurationEntry => {
    closed(entry, ["axis", "key", "value", "provenance"]);
    require(CONFIGURATION_KEYS.some(([axis, key]) => axis === entry.axis && key === entry.key) &&
      typeof entry.value === "string" && /^[a-z0-9_.,-]{1,64}$/.test(entry.value) &&
      ["configured", "default", "derived", "declared", "unknown"].includes(String(entry.provenance)));
    return { axis: entry.axis as string, key: entry.key as string, value: entry.value, provenance: entry.provenance as ConfigurationProvenance };
  });
  require(new Set(entries.map((entry) => `${entry.axis}/${entry.key}`)).size === entries.length);
  return { evaluated_at: value.evaluated_at as string, entries, read_receipt: receipt(value.read_receipt) };
}

/** Bound of one administration response, envelope included. */
export const ADMIN_PAGE_BYTES = 256 * 1024;
export const adminUrls = {
  memberships: (cursor: string | null) => `/api/v1/human/admin/memberships?${new URLSearchParams({ limit: "50", ...(cursor ? { cursor } : {}) })}`,
  quota: () => "/api/v1/human/admin/quota",
  usage: (cursor: string | null) => `/api/v1/human/admin/usage?${new URLSearchParams({ limit: "50", ...(cursor ? { cursor } : {}) })}`,
  configuration: () => "/api/v1/human/admin/configuration",
};
