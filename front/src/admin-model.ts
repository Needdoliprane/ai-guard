// Strict models of the administration reads of the 360° views: memberships, quota, usage and configuration, and of
// the delegated membership and share changes with their receipts.
// Pure: no DOM, no network, no clock. The control decides every value; the front never derives a right from them.
import { CAPABILITIES, HumanContractError, object, uuid, type Capability, type Principal, type ReadReceipt, type ResourceKind, type ResourceRef } from "./human-model.ts";
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
const RESOURCE_KINDS: readonly ResourceKind[] = ["session", "sandbox", "agent", "project"];
function resourceRef(value: unknown): ResourceRef {
  closed(value, ["kind", "resource_id"]);
  require(RESOURCE_KINDS.includes(value.kind as ResourceKind) && text(value.resource_id, 256));
  return { kind: value.kind as ResourceKind, resource_id: value.resource_id };
}
const sameResource = (left: ResourceRef, right: ResourceRef) => left.kind === right.kind && left.resource_id === right.resource_id;
const principalKey = (value: Principal) => value.kind === "human_local" ? `local\u0000${value.installation_id}\u0000${value.account_id}` : `oidc\u0000${value.issuer}\u0000${value.subject}`;
const positiveDecimal = (value: unknown): value is string => decimal(value) && value !== "0";

/** Closed share permissions, in the order of the wire. */
export const SHARE_PERMISSIONS = ["metadata", "content", "export"] as const;
export type SharePermission = typeof SHARE_PERMISSIONS[number];
export const GRANT_SHARES_MAX = 64;
export const MEMBERSHIP_CHANGE_BODY_BYTES = 4096;
export const GRANT_CHANGE_BODY_BYTES = 32768;

/** Durable outcome of a delegated membership change, decimal strings throughout. */
export interface MembershipChangeReceipt {
  version: 1; operation_id: string; membership_id: string; membership_revision: string; authorization_revision: string; audit_seq: string;
}
/** The receipt answers exactly the operation and the membership the request named. */
export function parseMembershipChangeReceipt(value: unknown, expected: { operationId: string; membershipId: string }): MembershipChangeReceipt {
  closed(value, ["version", "operation_id", "membership_id", "membership_revision", "authorization_revision", "audit_seq"]);
  require(value.version === 1 && uuid(value.operation_id) && value.operation_id === expected.operationId && uuid(value.membership_id) &&
    value.membership_id === expected.membershipId && positiveDecimal(value.membership_revision) && positiveDecimal(value.authorization_revision) &&
    positiveDecimal(value.audit_seq));
  return { version: 1, operation_id: value.operation_id, membership_id: value.membership_id, membership_revision: value.membership_revision,
    authorization_revision: value.authorization_revision as string, audit_seq: value.audit_seq as string };
}

export interface GrantShare { principal: Principal; display_name: string; permissions: SharePermission[] }
export interface HumanGrant {
  evaluated_at: string; source: "tenant_identity_resources"; resource: ResourceRef; owner: Principal; owner_display_name: string;
  revision: string; active: boolean; shares: GrantShare[]; read_receipt: ReadReceipt;
}
function permissions(value: unknown): SharePermission[] {
  require(Array.isArray(value) && value.length >= 1 && value.length <= SHARE_PERMISSIONS.length && new Set(value).size === value.length &&
    value.every((permission) => (SHARE_PERMISSIONS as readonly unknown[]).includes(permission)));
  return SHARE_PERMISSIONS.filter((permission) => (value as unknown[]).includes(permission));
}
/** One resource of the context's organization with its shares. No share names the owner, twice the same principal, or no permission. */
export function parseHumanGrant(value: unknown, expected: { contextId: string; authorizationVersion: string; resource: ResourceRef }): HumanGrant {
  closed(value, ["version", "context_id", "authorization_version", "source", "evaluated_at", "resource", "owner", "owner_display_name", "revision", "active", "shares", "read_receipt"]);
  frame(value, expected);
  const resource = resourceRef(value.resource); const owner = principal(value.owner);
  require(value.source === "tenant_identity_resources" && sameResource(resource, expected.resource) && text(value.owner_display_name, 160) &&
    positiveDecimal(value.revision) && typeof value.active === "boolean" && Array.isArray(value.shares) && value.shares.length <= GRANT_SHARES_MAX);
  const seen = new Set([principalKey(owner)]);
  const shares = value.shares.map((share): GrantShare => {
    closed(share, ["principal", "display_name", "permissions"]);
    const sharer = principal(share.principal); const key = principalKey(sharer);
    require(!seen.has(key) && text(share.display_name, 160)); seen.add(key);
    return { principal: sharer, display_name: share.display_name, permissions: permissions(share.permissions) };
  });
  return { evaluated_at: value.evaluated_at as string, source: "tenant_identity_resources", resource, owner, owner_display_name: value.owner_display_name,
    revision: value.revision, active: value.active, shares, read_receipt: receipt(value.read_receipt) };
}

export interface GrantChangeReceipt {
  version: 1; operation_id: string; resource: ResourceRef; grant_revision: string; authorization_revision: string; audit_seq: string;
}
export function parseGrantChangeReceipt(value: unknown, expected: { operationId: string; resource: ResourceRef }): GrantChangeReceipt {
  closed(value, ["version", "operation_id", "resource", "grant_revision", "authorization_revision", "audit_seq"]);
  const resource = resourceRef(value.resource);
  require(value.version === 1 && uuid(value.operation_id) && value.operation_id === expected.operationId && sameResource(resource, expected.resource) &&
    positiveDecimal(value.grant_revision) && positiveDecimal(value.authorization_revision) && positiveDecimal(value.audit_seq));
  return { version: 1, operation_id: value.operation_id, resource, grant_revision: value.grant_revision as string,
    authorization_revision: value.authorization_revision as string, audit_seq: value.audit_seq as string };
}

/** The closed bodies of the two delegated changes, checked against their bounds before any request. */
export interface MembershipChange { operation_id: string; expected_revision: string; capabilities: Capability[]; active: boolean }
export function membershipChangeBody(change: MembershipChange): string {
  require(uuid(change.operation_id) && positiveDecimal(change.expected_revision) && typeof change.active === "boolean" &&
    Array.isArray(change.capabilities) && new Set(change.capabilities).size === change.capabilities.length &&
    change.capabilities.every((capability) => (CAPABILITIES as readonly unknown[]).includes(capability)));
  const body = JSON.stringify({ operation_id: change.operation_id, expected_revision: change.expected_revision,
    capabilities: CAPABILITIES.filter((capability) => change.capabilities.includes(capability)), active: change.active });
  require(new TextEncoder().encode(body).length <= MEMBERSHIP_CHANGE_BODY_BYTES);
  return body;
}
export interface GrantChange { operation_id: string; resource: ResourceRef; expected_revision: string; shares: { principal: Principal; permissions: SharePermission[] }[] }
export function grantChangeBody(change: GrantChange, owner?: Principal): string {
  require(uuid(change.operation_id) && positiveDecimal(change.expected_revision) && Array.isArray(change.shares) && change.shares.length <= GRANT_SHARES_MAX);
  const resource = resourceRef(change.resource);
  const seen = new Set(owner ? [principalKey(owner)] : []);
  const shares = change.shares.map((share) => {
    const sharer = principal(share.principal); const key = principalKey(sharer);
    require(!seen.has(key)); seen.add(key);
    return { principal: sharer, permissions: permissions(share.permissions) };
  });
  const body = JSON.stringify({ operation_id: change.operation_id, resource, expected_revision: change.expected_revision, shares });
  require(new TextEncoder().encode(body).length <= GRANT_CHANGE_BODY_BYTES);
  return body;
}

export const adminUrls = {
  memberships: (cursor: string | null) => `/api/v1/human/admin/memberships?${new URLSearchParams({ limit: "50", ...(cursor ? { cursor } : {}) })}`,
  quota: () => "/api/v1/human/admin/quota",
  usage: (cursor: string | null) => `/api/v1/human/admin/usage?${new URLSearchParams({ limit: "50", ...(cursor ? { cursor } : {}) })}`,
  configuration: () => "/api/v1/human/admin/configuration",
  membershipChange: (membershipId: string) => { require(uuid(membershipId)); return `/api/v1/human/admin/memberships/${membershipId}`; },
  grant: (resource: ResourceRef) => { const ref = resourceRef(resource); return `/api/v1/human/admin/grants?${new URLSearchParams({ resource_kind: ref.kind, resource_id: ref.resource_id })}`; },
  grantChange: () => "/api/v1/human/admin/grants",
};
