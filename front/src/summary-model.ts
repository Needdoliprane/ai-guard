// Strict model of the per-context 360° summary served by the Rust control (`GET /api/v1/human/summary`).
// Pure: no DOM, no network, no clock. Every count is a decimal string computed by the control over its own
// registries or the verified journal; the front never adds, extrapolates or infers a count. A section the context
// may not read is "not granted", never zero; an unavailable section keeps its reason.
import { HumanContractError, object, uuid, type ReadReceipt } from "./human-model.ts";

export type SummaryWindow = "24h" | "7d" | "30d" | "all";
export const SUMMARY_WINDOWS: readonly SummaryWindow[] = ["24h", "7d", "30d", "all"];
export type SummaryCoverage = "complete_registry" | "authorized_selection" | "verified_journal";
export type SummaryUnavailable = "profile_unavailable" | "migration_required" | "capacity_exceeded";
export type SummarySection<T> = { status: "ready"; coverage: SummaryCoverage; data: T } |
  { status: "not_granted" } | { status: "unavailable"; code: SummaryUnavailable };
export type Counts = Readonly<Record<string, string>>;

export interface SummaryResources { by_kind: Counts }
export interface SummaryDecisions { by_outcome: Counts; complete: boolean; oldest_counted_at: string | null }
export interface SummaryBrokerRef { broker_id: string; state: BrokerState; last_received_at: string | null; lease_expires_at: string | null }
export interface SummaryWorkerGroup {
  association: "registry_unavailable" | "authority_unknown" | "unregistered" | "registered";
  runtime_kind: "docker" | "podman"; broker: SummaryBrokerRef | null; by_state: Counts; quota_held: string;
}
export interface SummaryWorkers { by_state: Counts; quota_held: string; groups: SummaryWorkerGroup[]; groups_complete: boolean; runtime_inventory: "unknown" }
export interface SummaryBrokers { by_state: Counts }
export interface SummaryQuota {
  max_active_workers: string; max_cpu_millis: string; max_memory_bytes: string;
  held_workers: string; held_cpu_millis: string; held_memory_bytes: string;
}
export interface SummaryUsage {
  allocations: string; duration_ns: string; cpu_millicore_nanoseconds: string; memory_byte_nanoseconds: string;
  method_version: 1; billable: false;
}
export type AttentionKind = typeof ATTENTION_KINDS[number];
export interface SummaryAttentionItem { kind: AttentionKind; subject_kind: "worker" | "broker"; subject_id: string; at: string | null }
export interface SummaryAttention { by_kind: Counts; items: SummaryAttentionItem[]; has_more: boolean }
export interface HumanSummary {
  version: 1; context_id: string; authorization_version: string; mode: "personal" | "administration";
  evaluated_at: string; period: { window: SummaryWindow; from: string | null; to: string };
  consistency: "transaction_snapshot";
  sections: {
    resources: SummarySection<SummaryResources>; decisions: SummarySection<SummaryDecisions>;
    workers: SummarySection<SummaryWorkers>; brokers: SummarySection<SummaryBrokers>;
    quota: SummarySection<SummaryQuota>; usage: SummarySection<SummaryUsage>; attention: SummarySection<SummaryAttention>;
  };
  read_receipt: ReadReceipt;
}

export type BrokerState = typeof BROKER_STATES[number];
export const BROKER_STATES = ["unseen", "current", "stale", "expired", "revoked", "clock_uncertain"] as const;
export const WORKER_STATES = ["reserved", "preparing", "created", "start_issued", "running", "stopping", "terminal", "in_doubt", "cancelled"] as const;
export const RESOURCE_KINDS = ["session", "agent", "sandbox", "project"] as const;
export const DECISION_OUTCOMES = ["allow", "deny", "review"] as const;
export const ATTENTION_KINDS = ["worker_in_doubt", "worker_allocation_inconsistent", "worker_oom_killed", "broker_heartbeat_unseen",
  "broker_lease_stale", "broker_grant_expired", "broker_revoked", "broker_clock_uncertain"] as const;
export const SUMMARY_MAX_GROUPS = 100;
export const SUMMARY_MAX_ATTENTION = 50;
export const SUMMARY_PAGE_BYTES = 256 * 1024;

function require(condition: unknown): asserts condition { if (!condition) throw new HumanContractError(); }
function closed(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  require(object(value) && Object.keys(value).length === fields.length && fields.every((field) => Object.hasOwn(value, field)));
}
const decimal = (value: unknown): value is string => typeof value === "string" && /^(0|[1-9][0-9]{0,38})$/.test(value);
const instant = (value: unknown): value is string =>
  typeof value === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{1,9})?(Z|[+-]\d\d:\d\d)$/.test(value) && Number.isFinite(Date.parse(value));
const nullableInstant = (value: unknown) => value === null || instant(value);
const hex64 = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const oneOf = <T extends string>(values: readonly T[], value: unknown): value is T => (values as readonly unknown[]).includes(value);

/** A closed count map: exactly the given keys, each a decimal string. */
function exactCounts(value: unknown, keys: readonly string[]): Counts {
  closed(value, keys);
  require(keys.every((key) => decimal(value[key])));
  return Object.freeze({ ...(value as Record<string, string>) });
}
/** A count map over a subset of known keys (only states present in the registry). */
function partialCounts(value: unknown, keys: readonly string[]): Counts {
  require(object(value) && Object.keys(value).length <= keys.length &&
    Object.entries(value).every(([key, count]) => keys.includes(key) && decimal(count)));
  return Object.freeze({ ...(value as Record<string, string>) });
}
function section<T>(value: unknown, data: (value: unknown) => T): SummarySection<T> {
  require(object(value));
  if (value.status === "not_granted") { closed(value, ["status"]); return { status: "not_granted" }; }
  if (value.status === "unavailable") {
    closed(value, ["status", "code"]);
    require(oneOf(["profile_unavailable", "migration_required", "capacity_exceeded"] as const, value.code));
    return { status: "unavailable", code: value.code };
  }
  closed(value, ["status", "coverage", "data"]);
  require(value.status === "ready" && oneOf(["complete_registry", "authorized_selection", "verified_journal"] as const, value.coverage));
  return { status: "ready", coverage: value.coverage, data: data(value.data) };
}
function brokerRef(value: unknown): SummaryBrokerRef | null {
  if (value === null) return null;
  closed(value, ["broker_id", "state", "last_received_at", "lease_expires_at"]);
  require(uuid(value.broker_id) && oneOf(BROKER_STATES, value.state) && nullableInstant(value.last_received_at) && nullableInstant(value.lease_expires_at));
  return { broker_id: value.broker_id, state: value.state, last_received_at: value.last_received_at as string | null, lease_expires_at: value.lease_expires_at as string | null };
}

export function parseHumanSummary(value: unknown, expected: { contextId: string; authorizationVersion: string }): HumanSummary {
  closed(value, ["version", "context_id", "authorization_version", "mode", "evaluated_at", "period", "consistency", "sections", "read_receipt"]);
  require(value.version === 1 && value.context_id === expected.contextId && value.authorization_version === expected.authorizationVersion &&
    hex64(value.context_id) && hex64(value.authorization_version) && oneOf(["personal", "administration"] as const, value.mode) &&
    instant(value.evaluated_at) && value.consistency === "transaction_snapshot");
  const period = value.period;
  closed(period, ["window", "from", "to"]);
  require(oneOf(SUMMARY_WINDOWS, period.window) && (period.window === "all" ? period.from === null : instant(period.from)) &&
    period.to === value.evaluated_at);
  const receipt = value.read_receipt;
  closed(receipt, ["request_id", "audit_event_id", "received_at"]);
  require(uuid(receipt.request_id) && uuid(receipt.audit_event_id) && instant(receipt.received_at));
  const sections = value.sections;
  closed(sections, ["resources", "decisions", "workers", "brokers", "quota", "usage", "attention"]);
  const parsed: HumanSummary["sections"] = {
    resources: section(sections.resources, (data) => { closed(data, ["by_kind"]); return { by_kind: exactCounts(data.by_kind, RESOURCE_KINDS) }; }),
    decisions: section(sections.decisions, (data) => {
      closed(data, ["by_outcome", "complete", "oldest_counted_at"]);
      require(typeof data.complete === "boolean" && (data.complete ? data.oldest_counted_at === null : nullableInstant(data.oldest_counted_at)));
      return { by_outcome: exactCounts(data.by_outcome, DECISION_OUTCOMES), complete: data.complete, oldest_counted_at: data.oldest_counted_at as string | null };
    }),
    workers: section(sections.workers, (data) => {
      closed(data, ["by_state", "quota_held", "groups", "groups_complete", "runtime_inventory"]);
      require(decimal(data.quota_held) && Array.isArray(data.groups) && data.groups.length <= SUMMARY_MAX_GROUPS &&
        typeof data.groups_complete === "boolean" && data.runtime_inventory === "unknown");
      return { by_state: partialCounts(data.by_state, WORKER_STATES), quota_held: data.quota_held, groups_complete: data.groups_complete,
        runtime_inventory: "unknown", groups: data.groups.map((group): SummaryWorkerGroup => {
          closed(group, ["association", "runtime_kind", "broker", "by_state", "quota_held"]);
          require(oneOf(["registry_unavailable", "authority_unknown", "unregistered", "registered"] as const, group.association) &&
            oneOf(["docker", "podman"] as const, group.runtime_kind) && decimal(group.quota_held));
          const broker = brokerRef(group.broker);
          require((group.association === "registered") === (broker !== null));
          return { association: group.association, runtime_kind: group.runtime_kind, broker, by_state: partialCounts(group.by_state, WORKER_STATES), quota_held: group.quota_held };
        }) };
    }),
    brokers: section(sections.brokers, (data) => { closed(data, ["by_state"]); return { by_state: exactCounts(data.by_state, BROKER_STATES) }; }),
    quota: section(sections.quota, (data) => {
      const fields = ["max_active_workers", "max_cpu_millis", "max_memory_bytes", "held_workers", "held_cpu_millis", "held_memory_bytes"] as const;
      closed(data, fields);
      require(fields.every((field) => decimal(data[field])));
      return Object.fromEntries(fields.map((field) => [field, data[field] as string])) as unknown as SummaryQuota;
    }),
    usage: section(sections.usage, (data) => {
      closed(data, ["allocations", "duration_ns", "cpu_millicore_nanoseconds", "memory_byte_nanoseconds", "method_version", "billable"]);
      require(decimal(data.allocations) && decimal(data.duration_ns) && decimal(data.cpu_millicore_nanoseconds) &&
        decimal(data.memory_byte_nanoseconds) && data.method_version === 1 && data.billable === false);
      return { allocations: data.allocations, duration_ns: data.duration_ns, cpu_millicore_nanoseconds: data.cpu_millicore_nanoseconds,
        memory_byte_nanoseconds: data.memory_byte_nanoseconds, method_version: 1, billable: false };
    }),
    attention: section(sections.attention, (data) => {
      closed(data, ["by_kind", "items", "has_more"]);
      require(Array.isArray(data.items) && data.items.length <= SUMMARY_MAX_ATTENTION && typeof data.has_more === "boolean");
      return { by_kind: partialCounts(data.by_kind, ATTENTION_KINDS), has_more: data.has_more, items: data.items.map((item): SummaryAttentionItem => {
        closed(item, ["kind", "subject_kind", "subject_id", "at"]);
        require(oneOf(ATTENTION_KINDS, item.kind) && oneOf(["worker", "broker"] as const, item.subject_kind) && uuid(item.subject_id) &&
          nullableInstant(item.at) && item.kind.startsWith(`${item.subject_kind}_`));
        return { kind: item.kind, subject_kind: item.subject_kind, subject_id: item.subject_id, at: item.at as string | null };
      }) };
    }),
  };
  return { version: 1, context_id: value.context_id, authorization_version: value.authorization_version, mode: value.mode,
    evaluated_at: value.evaluated_at, period: { window: period.window, from: period.from as string | null, to: period.to as string },
    consistency: "transaction_snapshot", sections: parsed,
    read_receipt: { request_id: receipt.request_id, audit_event_id: receipt.audit_event_id, received_at: receipt.received_at } };
}

export function humanSummaryUrl(window: SummaryWindow): string {
  return `/api/v1/human/summary?${new URLSearchParams({ window })}`;
}
/** The value of one count, or null when the map does not hold it (a state absent from the registry counts zero only
 *  when the map is exact; a partial map omits absent states and the caller says so). */
export function countOf(counts: Counts, key: string): string | null { return Object.hasOwn(counts, key) ? counts[key] ?? null : null; }
/** Digits grouped by thin spaces, from the exact decimal string; no float conversion. */
export function grouped(value: string): string {
  return value.replace(/\B(?=(\d{3})+(?!\d))/g, "\u202f");
}
/** Bytes as GiB with one decimal, computed exactly; the exact byte count stays available in the title. */
export function gibibytes(value: string): string {
  const tenths = BigInt(value) * 10n / (1n << 30n);
  return `${grouped((tenths / 10n).toString())},${(tenths % 10n).toString()} Gio`;
}
/** Nanosecond products expressed per second, exactly truncated. */
export function perSecond(value: string): string { return grouped((BigInt(value) / 1_000_000_000n).toString()); }
