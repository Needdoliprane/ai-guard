import type { Agent } from "./types.ts";

export type ControlView = "workers" | "quota" | "usage";
export type WorkerState = "reserved" | "preparing" | "created" | "start_issued" |
  "running" | "stopping" | "terminal" | "in_doubt" | "cancelled";
export interface RuntimeObservation {
  observed_at: string;
  status: string;
  running: boolean | null;
  started_at: string | null;
  finished_at: string | null;
  exit_code: number | null;
  oom_killed: boolean | null;
  restart_count: number | null;
}
export interface WorkerRecord {
  worker_id: string;
  state: WorkerState;
  quota_held: boolean;
  revision: number;
  launch: {
    version: number;
    session_id: string;
    profile: string;
    runtime_kind: "docker" | "podman";
    provider: string;
    cpu_millis: number;
    memory_bytes: number;
    pids_limit: number;
    timeout_seconds: number;
    network: string;
  };
  attempt: {
    attempt_id: string;
    stable_name: string;
    container_id: string | null;
    volume_name: string;
  };
  last_observation: RuntimeObservation | null;
  observed_started_at: string | null;
  allocation_inconsistent: boolean;
  unknown_reason: string | null;
  cleanup_removed?: boolean | null;
  cleanup_error?: string | null;
}
export interface QuotaRecord {
  policy: {
    version: number;
    max_active_workers: number;
    max_cpu_millis: number;
    max_memory_bytes: number;
    max_timeout_seconds: number;
  };
  held_workers: number;
  held_cpu_millis: number;
  held_memory_bytes: number;
}
export interface AllocationRecord {
  attempt_id: string;
  worker_id: string;
  terminal_observation_id: string;
  started_at: string;
  finished_at: string;
  duration_ns: string;
  cpu_millicore_nanoseconds: string;
  memory_byte_nanoseconds: string;
  method_version: number;
  billable: false;
  source: {
    authority_id: string;
    runtime_kind: "docker" | "podman";
    container_id: string;
    spec_hash: string;
    provenance: "observed";
  };
}
export interface ControlPage<T> { items: T[]; next_cursor: string | null }
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const states = new Set<WorkerState>(["reserved", "preparing", "created", "start_issued",
  "running", "stopping", "terminal", "in_doubt", "cancelled"]);
function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
function integer(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
function identifier(value: unknown): value is string {
  return typeof value === "string" && uuid.test(value);
}
function text(value: unknown): value is string { return typeof value === "string"; }
function nullableText(value: unknown): boolean { return value === null || text(value); }
function contract(): never {
  throw new Error("Contrat de lecture v1 incomplet ou incompatible. Vérifiez les versions du dashboard et du collecteur.");
}
function observation(value: unknown): boolean {
  if (value === null) return true;
  return record(value) && text(value.observed_at) && Number.isFinite(Date.parse(value.observed_at)) &&
    text(value.status) && (value.running === null || typeof value.running === "boolean") &&
    nullableText(value.started_at) && nullableText(value.finished_at) &&
    (value.exit_code === null || (typeof value.exit_code === "number" && Number.isSafeInteger(value.exit_code))) &&
    (value.oom_killed === null || typeof value.oom_killed === "boolean") &&
    (value.restart_count === null || integer(value.restart_count));
}
export function parseWorker(value: unknown): WorkerRecord {
  if (!record(value) || !identifier(value.worker_id) || !states.has(value.state as WorkerState) ||
    typeof value.quota_held !== "boolean" || !integer(value.revision) || !record(value.launch) ||
    !record(value.attempt) || !observation(value.last_observation) ||
    !nullableText(value.observed_started_at) || typeof value.allocation_inconsistent !== "boolean") return contract();
  const launch = value.launch;
  const attempt = value.attempt;
  if (launch.version !== 1 || !identifier(launch.session_id) || !text(launch.profile) ||
    !["docker", "podman"].includes(String(launch.runtime_kind)) || !text(launch.provider) ||
    !text(launch.network) || ![launch.cpu_millis, launch.memory_bytes, launch.pids_limit,
      launch.timeout_seconds].every(integer) || !identifier(attempt.attempt_id) ||
    !text(attempt.stable_name) || !text(attempt.volume_name) || !nullableText(attempt.container_id) ||
    !nullableText(value.unknown_reason) ||
    (value.cleanup_removed != null && typeof value.cleanup_removed !== "boolean") ||
    (value.cleanup_error != null && !text(value.cleanup_error))) return contract();
  const source = value as unknown as WorkerRecord;
  const latest = source.last_observation;
  // Keep only the read model. A future server field must not enter a raw detail by accident.
  return {
    worker_id: source.worker_id, state: source.state, quota_held: source.quota_held,
    revision: source.revision, unknown_reason: source.unknown_reason,
    observed_started_at: source.observed_started_at, allocation_inconsistent: source.allocation_inconsistent,
    cleanup_removed: source.cleanup_removed, cleanup_error: source.cleanup_error,
    launch: {
      version: source.launch.version, session_id: source.launch.session_id,
      profile: source.launch.profile, runtime_kind: source.launch.runtime_kind,
      provider: source.launch.provider, cpu_millis: source.launch.cpu_millis,
      memory_bytes: source.launch.memory_bytes, pids_limit: source.launch.pids_limit,
      timeout_seconds: source.launch.timeout_seconds, network: source.launch.network,
    },
    attempt: {
      attempt_id: source.attempt.attempt_id, stable_name: source.attempt.stable_name,
      container_id: source.attempt.container_id, volume_name: source.attempt.volume_name,
    },
    last_observation: latest ? {
      observed_at: latest.observed_at, status: latest.status, running: latest.running,
      started_at: latest.started_at, finished_at: latest.finished_at, exit_code: latest.exit_code,
      oom_killed: latest.oom_killed, restart_count: latest.restart_count,
    } : null,
  };
}
function parsePage<T>(value: unknown, parseItem: (item: unknown) => T): ControlPage<T> {
  if (!record(value) || !Array.isArray(value.items) || value.items.length > 100 ||
    !(value.next_cursor === null || identifier(value.next_cursor))) return contract();
  return { items: value.items.map(parseItem), next_cursor: value.next_cursor };
}
export function parseWorkerPage(value: unknown): ControlPage<WorkerRecord> {
  return parsePage(value, parseWorker);
}
export function parseQuota(value: unknown): QuotaRecord {
  if (!record(value) || !record(value.policy) || value.policy.version !== 1 ||
    ![value.policy.max_active_workers, value.policy.max_cpu_millis, value.policy.max_memory_bytes,
      value.policy.max_timeout_seconds, value.held_workers, value.held_cpu_millis,
      value.held_memory_bytes].every(integer)) return contract();
  return value as unknown as QuotaRecord;
}
export function parseAllocationPage(value: unknown): ControlPage<AllocationRecord> {
  return parsePage(value, (item) => {
    if (!record(item) || !identifier(item.attempt_id) || !identifier(item.worker_id) ||
      !identifier(item.terminal_observation_id) || !text(item.started_at) ||
      !Number.isFinite(Date.parse(item.started_at)) || !text(item.finished_at) ||
      !Number.isFinite(Date.parse(item.finished_at)) || item.method_version !== 1 || item.billable !== false ||
      !record(item.source) || !text(item.source.authority_id) || !text(item.source.container_id) ||
      !["docker", "podman"].includes(String(item.source.runtime_kind)) || item.source.provenance !== "observed" ||
      typeof item.source.spec_hash !== "string" || !/^[a-f0-9]{64}$/.test(item.source.spec_hash) ||
      ![item.duration_ns, item.cpu_millicore_nanoseconds, item.memory_byte_nanoseconds]
        .every((unit) => typeof unit === "string" && /^(0|[1-9][0-9]{0,38})$/.test(unit))) return contract();
    return item as unknown as AllocationRecord;
  });
}
export function controlPageUrl(view: "workers" | "usage", cursor: string | null = null): string {
  if (cursor !== null && !identifier(cursor)) throw new Error("Curseur de lecture invalide.");
  const query = new URLSearchParams({ limit: "50" });
  if (cursor !== null) query.set("cursor", cursor);
  return `/api/v1/${view}?${query}`;
}
export function workerUrl(id: string): string {
  if (!identifier(id)) throw new Error("Identifiant de worker invalide.");
  return `/api/v1/workers/${id}`;
}
export function isControlReadPath(path: string): boolean {
  const match = /^\/api\/v1\/(workers|quota|usage)(?:\/([^/?#]+))?(?:\?([^#]*))?$/.exec(path);
  if (!match) return false;
  if (match[2]) return match[1] === "workers" && identifier(match[2]) && match[3] === undefined;
  if (match[1] === "quota") return match[3] === undefined;
  const query = new URLSearchParams(match[3]);
  if ([...query.keys()].some((key) => !["limit", "cursor"].includes(key))) return false;
  if (query.getAll("limit").length > 1 || query.getAll("cursor").length > 1) return false;
  const limit = query.get("limit");
  const cursor = query.get("cursor");
  return (limit === null || (/^[1-9][0-9]{0,2}$/.test(limit) && Number(limit) <= 100)) &&
    (cursor === null || identifier(cursor));
}
/** Decimal shift on strings: allocation units must never pass through a floating point number. */
export function formatExactDecimal(value: unknown, scale = 0): string {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]{0,38})$/.test(value)) return "Inconnu";
  const padded = value.padStart(scale + 1, "0");
  const whole = (scale ? padded.slice(0, -scale) : padded).replace(/\B(?=(\d{3})+(?!\d))/g, "\u202f");
  const fraction = scale ? padded.slice(-scale).replace(/0+$/, "") : "";
  return fraction ? `${whole},${fraction}` : whole;
}
export function allocationDuration(value: AllocationRecord | null | undefined): string {
  return value ? `${formatExactDecimal(value.duration_ns, 9)} s` : "Inconnue · aucune observation terminale";
}
export function relatedWorkers(agent: Agent, workers: WorkerRecord[]): WorkerRecord[] {
  return workers.filter((worker) =>
    !!agent.session_id && worker.launch.session_id === agent.session_id ||
    !!agent.sandbox_id && [worker.attempt.container_id, worker.attempt.stable_name].includes(agent.sandbox_id));
}
