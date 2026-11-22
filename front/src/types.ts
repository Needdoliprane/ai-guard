export type Provenance = "observed" | "declared" | "inferred";
export type Coverage = "native" | "observed" | "generic";
export interface ViewerIdentity {
  tenant_id: string;
  role: "viewer";
}
export type View =
  | "overview"
  | "agents"
  | "actions"
  | "security"
  | "files"
  | "sandboxes"
  | "workers"
  | "quota"
  | "usage";
export interface AuditEvent {
  seq: number;
  id: string;
  timestamp: string;
  received_at?: string | null;
  kind: string;
  source?: string | null;
  provenance?: Provenance | null;
  coverage?: Coverage | null;
  session_id?: string | null;
  project_id?: string | null;
  sandbox_id?: string | null;
  agent_id?: string | null;
  parent_agent_id?: string | null;
  correlation_id?: string | null;
  trace_id?: string | null;
  command?: { argv?: string[] | null; cwd?: string | null } | null;
  decision?: {
    outcome?: string | null;
    engine?: string | null;
    rule?: string | null;
    reason?: string | null;
  } | null;
  result?: {
    exit_code?: number | null;
    duration_ms?: number | null;
    timed_out?: boolean | null;
    transport_stop_reason?: string | null;
    control_deadline?: {
      deadline: "original_execution_deadline" | "observation_window";
      stop: "no_owned_worker" | "already_stopped" | "engine_acknowledged" | "unconfirmed";
    } | null;
    stdout?: string | null;
    stderr?: string | null;
    truncated?: boolean | null;
  } | null;
  data?: Record<string, unknown> | null;
  hash?: string | null;
  prev_hash?: string | null;
}
export interface Agent {
  id: string;
  provider?: string | null;
  name?: string | null;
  status?: string | null;
  parent_id?: string | null;
  project_id?: string | null;
  sandbox_id?: string | null;
  model?: string | null;
  session_id?: string | null;
  started_at?: string | null;
  last_activity?: string | null;
  task?: string | null;
  compatibility?: Coverage | null;
  provenance?: Provenance | null;
  field_provenance?: Record<string, Provenance> | null;
  parent_unresolved?: string | null;
  hierarchy_warning?: string | null;
}
export interface Sandbox {
  id: string;
  engine?: string | null;
  image?: string | null;
  profile?: string | null;
  status?: string | null;
  project_id?: string | null;
  session_id?: string | null;
  network?: string | null;
  started_at?: string | null;
  resources?: {
    cpu_percent?: number | null;
    memory_bytes?: number | null;
  } | null;
  mounts?: unknown[] | null;
}
export interface Overview {
  counts?: Partial<
    Record<
      | "agents"
      | "subagents"
      | "declared_agents"
      | "declared_subagents"
      | "sandboxes"
      | "commands"
      | "allowed"
      | "denied"
      | "pending"
      | "security",
      number | null
    >
  >;
  health?: {
    collector?: string | null;
    engine?: string | null;
    jev?: string | null;
  };
  resources?: { cpu_percent?: number | null; memory_bytes?: number | null };
  coverage?: Coverage | null;
  timestamp?: string | null;
}
export interface Integrity {
  valid: boolean;
  /** Chain length; after audit retention, more than the events still stored. */
  event_count?: number | null;
  head_hash?: string | null;
  stored_events?: number | null;
  /** Parsed by `parseRetentionLedger`; unknown until it validates. */
  retention?: unknown;
}
export interface EventPage {
  items: AuditEvent[];
  next_cursor: number | null;
}
export type EventPeriod = "all" | "hour" | "day" | "week";
export interface EventFilters {
  from?: string;
  to?: string;
  q?: string;
  agent_id?: string;
  sandbox_id?: string;
  project_id?: string;
  kind?: string;
  decision?: string;
}
export interface AgentNode {
  agent: Agent;
  children: AgentNode[];
  issue?: "missing-parent" | "cycle";
}
