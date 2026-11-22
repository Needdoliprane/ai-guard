import type {
  Agent,
  AgentNode,
  AuditEvent,
  EventFilters,
  EventPeriod,
  View,
} from "./types.ts";
import { boundedText, object, uuid } from "./human-model.ts";

export function parseEntityCollection<T extends { id: string }>(input: unknown): T[] {
  if (!input || typeof input !== "object" || !Array.isArray((input as { items?: unknown }).items))
    throw new Error("Contrat de liste d’identités incomplet ou incompatible.");
  const items = (input as { items: unknown[] }).items;
  if (items.some((item) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) return true;
    const entity = item as Record<string, unknown>;
    return typeof entity.id !== "string" || !entity.id ||
      [entity.session_id, entity.sandbox_id].some((id) => id != null && typeof id !== "string");
  })) throw new Error("Contrat de liste d’identités incomplet ou incompatible.");
  return items as T[];
}

export function periodFilters(
  period: EventPeriod,
  chosenAt = Date.now(),
): Pick<EventFilters, "from" | "to"> {
  if (period === "all") return {};
  const durations = { hour: 3600000, day: 86400000, week: 604800000 };
  return { from: new Date(chosenAt - durations[period]).toISOString() };
}

export function mergeEventPage(
  current: AuditEvent[],
  incoming: AuditEvent[],
  liveAfter?: number,
): AuditEvent[] {
  const retained =
    liveAfter === undefined
      ? current
      : current.filter((event) => event.seq > liveAfter);
  return [
    ...new Map(
      [...retained, ...incoming].map((event) => [event.seq, event]),
    ).values(),
  ].sort((a, b) => b.seq - a.seq);
}

export function buildAgentForest(agents: Agent[]): AgentNode[] {
  const nodes = new Map<string, AgentNode>();
  for (const agent of agents)
    if (agent.id && !nodes.has(agent.id))
      nodes.set(agent.id, { agent, children: [] });
  const roots: AgentNode[] = [];
  for (const node of nodes.values()) {
    const parentId = node.agent.parent_id;
    if (!parentId) {
      roots.push(node);
      continue;
    }
    const parent = nodes.get(parentId);
    if (!parent) {
      node.issue = "missing-parent";
      roots.push(node);
      continue;
    }
    const visited = new Set([node.agent.id]);
    let ancestor: AgentNode | undefined = parent;
    let cyclic = false;
    while (ancestor) {
      if (visited.has(ancestor.agent.id)) {
        cyclic = true;
        break;
      }
      visited.add(ancestor.agent.id);
      ancestor = ancestor.agent.parent_id
        ? nodes.get(ancestor.agent.parent_id)
        : undefined;
    }
    if (cyclic) {
      node.issue = "cycle";
      roots.push(node);
    } else parent.children.push(node);
  }
  return roots;
}

export function formatMetric(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value)
    ? new Intl.NumberFormat("fr-FR", { maximumFractionDigits: 1 }).format(value)
    : "Inconnu";
}

export function formatBytes(value: unknown): string {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0)
    return "Inconnu";
  const units = ["o", "Kio", "Mio", "Gio"];
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index++;
  }
  return `${formatMetric(value)} ${units[index]}`;
}

export function formatDate(value: unknown, full = false): string {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value)))
    return "Inconnu";
  return new Intl.DateTimeFormat(
    "fr-FR",
    full
      ? { dateStyle: "medium", timeStyle: "medium" }
      : { hour: "2-digit", minute: "2-digit", second: "2-digit" },
  ).format(new Date(value));
}

export function asText(value: unknown, fallback = "Inconnu"): string {
  if (value === undefined || value === null || value === "") return fallback;
  return typeof value === "string"
    ? value
    : typeof value === "object"
      ? JSON.stringify(value)
      : String(value);
}

const failurePhaseLabels: ReadonlyMap<string, string> = new Map([
  ["decision", "Décision"],
  ["workspace_preparation", "Préparation de l’espace de travail"],
  ["admission", "Admission"],
  ["import", "Import"],
  ["import_verification", "Vérification de l’import"],
  ["worker", "Worker"],
  ["final_scan", "Analyse finale"],
  ["export", "Export"],
  ["cleanup", "Nettoyage"],
]);
const failureStepLabels: ReadonlyMap<string, string> = new Map([
  ["arm_exchange", "Armement de l’arrêt automatique"],
  ["registration", "Connexion au gardien d’arrêt"],
  ["readiness_before_claim", "Vérification avant démarrage"],
  ["load_after_registration", "Vérification avant démarrage"],
  ["clock_before_claim", "Vérification du délai"],
  ["claim_exchange", "Autorisation de démarrage"],
  ["generation_before_prepare", "Vérification du lancement"],
  ["prepare", "Préparation de l’exécution"],
  ["ownership_commit", "Enregistrement du conteneur"],
  ["ownership_reload", "Vérification du conteneur enregistré"],
  ["created_inspection", "Vérification de la création"],
  ["created_exchange", "Confirmation de la création"],
  ["failure_observed", "Observation d’un échec"],
]);
const enforcerStageLabels: ReadonlyMap<string, string> = new Map([
  ["register", "Connexion au gardien d’arrêt"],
  ["before_claim", "Vérification avant démarrage"],
  ["prepare", "Préparation de l’exécution"],
  ["before_spawn", "Vérification finale avant démarrage"],
]);
const runtimeStageLabels: ReadonlyMap<string, string> = new Map([
  ["observe_only", "Démarrage non émis lors de cette tentative"],
  ["authority_before_spawn", "Vérification de l’autorité du moteur"],
  ["invocation_before_spawn", "Vérification du programme"],
  ["stop_before_spawn", "Vérification de la poursuite du lancement"],
  ["guard_before_spawn", "Vérification du garde"],
  ["spawn", "Lancement du programme"],
  ["started_observation", "Vérification de l’exécution"],
]);

export function brokerFailureDetails(event: AuditEvent) {
  if (event.kind !== "broker.batch.failure") return null;
  const supported = event.data?.version === 1;
  const label = (labels: ReadonlyMap<string, string>, value: unknown) =>
    supported && typeof value === "string"
      ? labels.get(value)
      : undefined;
  const stage = (value: unknown) =>
    value && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>).stage
      : undefined;
  return {
    phase: label(failurePhaseLabels, event.data?.phase) ?? "Inconnue",
    step:
      label(failureStepLabels, event.data?.broker_step) ??
      label(enforcerStageLabels, stage(event.data?.enforcer)) ??
      label(runtimeStageLabels, stage(event.data?.runtime)) ??
      "Inconnue",
    // This report observes a broker failure, never an execution outcome.
    runtimeEffect: "non confirmé",
  };
}

export type BrokerCapture = {
  state: "retained" | "not_retained" | "unknown";
  text: string | null;
  truncated: boolean;
  textClipped: boolean;
};

const utf8 = new TextEncoder();
// The audit redactor can expand the captured text before applying its own UTF-8 bound.
const maxBrokerTextBytes = 32768 + utf8.encode("…[truncated]").byteLength;
const maxU128 = (1n << 128n) - 1n;

function brokerCapture(value: unknown): BrokerCapture {
  const facts = object(value) ? value : null;
  const output = facts?.text;
  const text = typeof output === "string" && output.length <= maxBrokerTextBytes &&
    utf8.encode(output).byteLength <= maxBrokerTextBytes ? output : null;
  return {
    state: text !== null ? "retained" : output === null ? "not_retained" : "unknown",
    text,
    truncated: facts?.truncated === true,
    textClipped: text !== null && facts?.text_clipped === true,
  };
}

function exitCode(value: unknown, minimum: number, maximum: number): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= minimum && value <= maximum ? value : null;
}

function brokerTime(value: unknown): string | null {
  return typeof value === "string" && /^\d{4}-\d{2}-\d{2}T/.test(value) && Number.isFinite(Date.parse(value)) ? value : null;
}

export function brokerBatchResultDetails(event: AuditEvent) {
  if (event.kind !== "broker.batch.result" || event.source !== "runner" ||
    event.provenance !== "observed" || event.data?.version !== 1) return null;
  const data = event.data;
  const process = object(data.observation) && object(data.observation.process) ? data.observation.process : null;
  const startedAt = brokerTime(process?.started_at);
  const finishedAt = brokerTime(process?.finished_at);
  const observedExitCode = exitCode(process?.exit_code, 0, 4294967295);
  const terminal = (process?.status === "exited" || process?.status === "stopped") && process.running === false &&
    observedExitCode !== null && startedAt !== null && finishedAt !== null && Date.parse(startedAt) <= Date.parse(finishedAt);
  const transport = object(data.transport) ? data.transport : null;
  const duration = transport?.duration_ms_decimal;
  return {
    observedExitCode: terminal ? observedExitCode : null,
    oomKilled: terminal && typeof process.oom_killed === "boolean" ? process.oom_killed : null,
    startedAt: terminal ? startedAt : null,
    finishedAt: terminal ? finishedAt : null,
    transportExitCode: exitCode(transport?.exit_code, -2147483648, 2147483647),
    transportDurationMs: typeof duration === "string" && /^(?:0|[1-9][0-9]{0,38})$/.test(duration) &&
      duration.trim() === duration && BigInt(duration) <= maxU128 ? duration : null,
    stdout: brokerCapture(transport?.stdout),
    stderr: brokerCapture(transport?.stderr),
  };
}

const evaluatorLabels = new Map([
  ["rust_rules", "Rust déterministe"],
  ["typesafe", "Jev TypeSafe"],
  ["local_openai", "Modèle local"],
]);
const evaluationStateLabels = new Map([
  ["evaluated", "Évaluée"],
  ["skipped_local_policy", "Non sollicitée par la politique locale"],
  ["unavailable_or_invalid", "Indisponible ou invalide"],
]);
const evaluatorSelectionLabels = new Map([
  ["cli", "Ligne de commande"],
  ["environment", "Variables d’environnement"],
  ["legacy", "Configuration historique"],
  ["programmatic", "Configuration du gateway"],
]);
const adviceLabels = new Map([
  ["allow", "Autoriser"],
  ["deny", "Refuser"],
  ["review", "Demander une revue"],
]);

function modelOnWire(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 128 && utf8.encode(value).byteLength <= 128;
}

function evaluationModel(value: unknown): string | null {
  return modelOnWire(value) && !!value.trim() && !/\p{Cc}/u.test(value) ? value : null;
}

function legacyTypeSafeStatus(evidence: Record<string, unknown>, decision: AuditEvent["decision"]) {
  if (Object.hasOwn(evidence, "backend")) return null;
  const fields = Object.keys(evidence);
  if (fields.length === 1 && evidence.status === "unavailable_or_invalid" &&
    decision?.outcome === "deny" && decision.engine === "jev" && decision.rule === "jev_unavailable_or_invalid")
    return "unavailable_or_invalid";
  // Historical JevAnswer held these four fields before the backend envelope.
  const outcome = evidence.outcome;
  if (fields.length !== 4 || !Object.hasOwn(evidence, "usage") ||
    typeof outcome !== "string" || !adviceLabels.has(outcome) ||
    typeof evidence.confidence !== "number" || !(evidence.confidence >= 0 && evidence.confidence <= 1) ||
    !modelOnWire(evidence.model)) return null;
  const matchesDecision = outcome === "allow" ? decision?.outcome === "allow" && decision.engine === "deterministic"
    : decision?.outcome === "deny" && decision.engine === "jev" &&
      decision.rule === (outcome === "deny" ? "jev_deny" : "jev_review_fail_closed");
  return matchesDecision ? "evaluated" : null;
}

export function decisionEvaluationDetails(event: AuditEvent) {
  const evidence = event.data?.jev;
  if (event.kind !== "security.decision" || event.source !== "gateway" ||
    event.provenance !== "observed" || !object(evidence)) return null;
  const historicalStatus = legacyTypeSafeStatus(evidence, event.decision);
  const backend = historicalStatus ? "typesafe" : evidence.backend;
  const status = historicalStatus ?? evidence.status;
  const evaluator = typeof backend === "string" ? evaluatorLabels.get(backend) : undefined;
  const evaluationState = typeof status === "string" ? evaluationStateLabels.get(status) : undefined;
  if (!evaluator || !evaluationState) return null;
  const selection = evidence.selection_source;
  const localAdvisory = backend === "local_openai";
  const models = backend === "typesafe" || localAdvisory;
  const outcome = localAdvisory ? evidence.provider_outcome : evidence.outcome;
  const advice = localAdvisory && outcome === "unknown" ? "Indéterminé" :
    typeof outcome === "string" ? adviceLabels.get(outcome) ?? null : null;
  return {
    evaluator,
    evaluationState,
    selectionSource: typeof selection === "string" ? evaluatorSelectionLabels.get(selection) ?? null : null,
    requestedModel: models ? evaluationModel(evidence.requested_model) : null,
    returnedModel: models && status === "evaluated" ? evaluationModel(evidence.model) : null,
    advice: models && status === "evaluated" ? advice : null,
    localAdvisory,
  };
}

type BrokerWorkspaceChange = {
  path: string;
  change: "created" | "modified" | "deleted";
  beforeSha256: string | null;
  afterSha256: string | null;
};

export type BrokerWorkspaceDetails =
  | {
      kind: "scan";
      changedFiles: number;
      manifestDigest: string;
      truncated: boolean;
      changes: BrokerWorkspaceChange[];
    }
  | { kind: "export"; files: number; manifestDigest: string; requestId: string };

// The broker diff is over two workspaces and retains at most 200 detail rows.
const maxWorkspaceEntries = 20_000;
const maxWorkspaceChanges = 200;

function workspaceCount(value: unknown, maximum: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}

function workspaceDigest(value: unknown): value is string {
  return typeof value === "string" && value.length === 64 && !/[^0-9a-f]/.test(value);
}

function workspaceFingerprint(value: unknown): string | undefined {
  return object(value) && workspaceDigest(value.sha256) ? value.sha256 : undefined;
}

export function brokerWorkspaceDetails(event: AuditEvent): BrokerWorkspaceDetails | null {
  if (event.kind !== "broker.batch.scan" && event.kind !== "broker.batch.export") return null;
  const data = event.data;
  if (event.source !== "runner" || event.provenance !== "observed" || !object(data) ||
      data.version !== 1 || !workspaceDigest(data.manifest_digest)) return null;
  const manifestDigest = data.manifest_digest;

  if (event.kind === "broker.batch.export") {
    const { files, request_id: requestId } = data;
    if (!workspaceCount(files, maxWorkspaceEntries) || !uuid(requestId) || requestId.length !== 36 ||
        requestId !== requestId.toLowerCase() || requestId === "00000000-0000-0000-0000-000000000000") return null;
    return { kind: "export", files, manifestDigest, requestId };
  }

  const { changed_files: changedFiles, truncated, changes: items } = data;
  if (!workspaceCount(changedFiles, 2 * maxWorkspaceEntries) || typeof truncated !== "boolean" ||
      !Array.isArray(items) || items.length > maxWorkspaceChanges ||
      (truncated ? items.length >= changedFiles : items.length !== changedFiles)) return null;

  const changes: BrokerWorkspaceChange[] = [];
  const paths = new Set<string>();
  let totalPathBytes = 0;
  for (const item of items) {
    if (!object(item)) return null;
    const { path, change, before, after } = item;
    if (!boundedText(path) || /[\\:]/.test(path) ||
        path.split("/").some((part) => part === "" || part === "." || part === "..") ||
        paths.has(path)) return null;
    const pathBytes = utf8.encode(path).length;
    totalPathBytes += pathBytes;
    if (pathBytes > 4096 || totalPathBytes > 110 * 1024) return null;
    paths.add(path);

    if (change !== "created" && change !== "modified" && change !== "deleted") return null;
    const beforeSha256 = before === null ? null : workspaceFingerprint(before);
    const afterSha256 = after === null ? null : workspaceFingerprint(after);
    if (beforeSha256 === undefined || afterSha256 === undefined ||
        (beforeSha256 === null) !== (change === "created") ||
        (afterSha256 === null) !== (change === "deleted")) return null;
    changes.push({ path, change, beforeSha256, afterSha256 });
  }
  return { kind: "scan", changedFiles, manifestDigest, truncated, changes };
}

const ACTIVITY_CHANNEL: Record<string, string> = {
  stdout: "sortie structurée",
  hook: "hook officiel",
};

function text(value: unknown): string | null {
  return typeof value === "string" && value ? value : null;
}

/** Channel of a live provider record, as recorded by the control. */
export function activityChannel(event: AuditEvent): string | null {
  const activity = event.data?.activity;
  if (!activity || typeof activity !== "object") return null;
  const channel = (activity as Record<string, unknown>).channel;
  return typeof channel === "string" ? (ACTIVITY_CHANNEL[channel] ?? channel) : null;
}

export interface ActivityChannelCounts {
  records: number;
  mapped: number;
  ignored: number;
  unrecognized: number;
  oversized: number;
  invalid: number;
  events: number;
  dropped: number;
  refused: number;
  declaredLosses: number;
}

export interface ActivityDetails {
  channel: string | null;
  client: string | null;
  interactive: boolean | null;
  versionLabel: string | null;
  workerId: string | null;
  attemptId: string | null;
  receivedAt: string | null;
  /** The record is the client's own declaration, not an ai-guard observation. */
  declaredByClient: boolean;
  truncated: boolean;
  coverage: {
    complete: boolean;
    budgetExhausted: boolean | null;
    auditUnavailable: boolean | null;
    stalled: boolean | null;
    spooledEvents: number | null;
    deliveredEvents: number | null;
    hookChannel: string | null;
    stdout: ActivityChannelCounts | null;
    hook: ActivityChannelCounts | null;
  } | null;
}

function optionalBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

export function formatActivityCoverageFlag(value: boolean | null): string {
  if (value === null) return "Inconnu";
  return value ? "Oui" : "Non";
}

function count(value: unknown): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function channelCounts(value: unknown): ActivityChannelCounts | null {
  if (!value || typeof value !== "object") return null;
  const v = value as Record<string, unknown>;
  return {
    records: count(v.records),
    mapped: count(v.mapped),
    ignored: count(v.ignored),
    unrecognized: count(v.unrecognized),
    oversized: count(v.oversized),
    invalid: count(v.invalid),
    events: count(v.events),
    dropped: count(v.dropped),
    refused: count(v.refused),
    declaredLosses: count(v.declared_losses),
  };
}

/** Collection metadata recorded by the control for a live activity record. */
export function activityDetails(event: AuditEvent): ActivityDetails | null {
  const data = (event.data ?? {}) as Record<string, unknown>;
  const activity = data.activity;
  if (!activity || typeof activity !== "object") return null;
  const a = activity as Record<string, unknown>;
  const coverage =
    event.kind === "activity.coverage" && event.source === "runner" && event.provenance === "observed"
      ? {
          complete: data.complete === true,
          budgetExhausted: optionalBoolean(data.budget_exhausted),
          auditUnavailable: optionalBoolean(data.audit_unavailable),
          stalled: optionalBoolean(data.stalled),
          spooledEvents: typeof data.spooled_events === "number" ? data.spooled_events : null,
          deliveredEvents: typeof data.delivered_events === "number" ? data.delivered_events : null,
          hookChannel: text(data.hook_channel),
          stdout: channelCounts(data.stdout),
          hook: channelCounts(data.hook),
        }
      : null;
  return {
    channel: activityChannel(event),
    client: text(a.client),
    interactive: typeof a.interactive === "boolean" ? a.interactive : null,
    versionLabel: text(a.client_version_label),
    workerId: text(a.worker_id),
    attemptId: text(a.attempt_id),
    receivedAt: text(a.received_at),
    declaredByClient: event.provenance === "declared" && event.source !== "runner",
    truncated:
      data.text_truncated === true ||
      data.prompt_truncated === true ||
      data.hook_input_truncated === true ||
      data.adapter_payload_truncated === true,
    coverage,
  };
}

/**
 * One-line reading of live provider activity. Provider facts stay declared:
 * the wording never turns a declaration into an observed effect.
 */
export function providerActivitySummary(event: AuditEvent): string | null {
  const data = (event.data ?? {}) as Record<string, unknown>;
  const truncated = data.text_truncated === true ? " (tronqué)" : "";
  switch (event.kind) {
    case "message.declared": {
      const body = text(data.text);
      if (!body) return null;
      const who = data.role === "user" ? "Consigne déclarée" : "Réponse déclarée";
      return `${who} : ${body}${truncated}`;
    }
    case "reasoning.declared":
      return data.redacted === true
        ? "Raisonnement masqué par le fournisseur"
        : `Raisonnement déclaré : ${text(data.text) ?? "—"}${truncated}`;
    case "tool.requested": {
      const tool = text(data.tool_name) ?? "outil";
      const command = text(data.command_text) ?? text(data.query);
      return command ? `Outil demandé · ${tool} : ${command}` : `Outil demandé · ${tool}`;
    }
    case "tool.result": {
      const result = (event.result ?? {}) as Record<string, unknown>;
      const tool = text(data.tool_name) ?? text(data.provider_tool_use_id) ?? "outil";
      if (result.is_error === true)
        return `Résultat déclaré en échec · ${tool}${text(result.error) ? ` : ${result.error}` : ""}`;
      if (typeof result.exit_code === "number")
        return `Résultat déclaré · ${tool} · code ${result.exit_code}`;
      return `Résultat déclaré · ${tool}`;
    }
    case "delegation.declared": {
      const target = text(data.agent_type) ?? text(data.delegation_tool) ?? "sous-agent";
      const description = text(data.description) ?? text(data.prompt);
      return description ? `Délégation déclarée · ${target} : ${description}` : `Délégation déclarée · ${target}`;
    }
    case "agent.declared": {
      const role = data.agent_role === "root" ? "Agent principal" : "Sous-agent";
      const id = text(data.provider_agent_id) ?? "inconnu";
      const state = text(data.state);
      return `${role} déclaré ${id}${state ? ` · ${state}` : ""}`;
    }
    case "plan.declared": {
      const items = Array.isArray(data.items) ? data.items : [];
      const done = items.filter((item) => (item as Record<string, unknown>)?.completed === true).length;
      return `Plan déclaré · ${done}/${items.length} étape${items.length === 1 ? "" : "s"} terminée${done === 1 ? "" : "s"}`;
    }
    case "activity.loss": {
      const reason = text(data.reason) ?? "cause inconnue";
      const who = event.provenance === "declared" ? "Perte déclarée par le worker" : "Perte de collecte observée";
      return `${who} · ${reason}`;
    }
    case "activity.channel":
      return `Canal ${ACTIVITY_CHANNEL[String(data.channel)] ?? String(data.channel ?? "inconnu")} · ${text(data.state) ?? "état inconnu"}${text(data.reason) ? ` (${data.reason})` : ""}`;
    case "activity.coverage":
      return data.complete === true
        ? "Couverture de collecte live complète pour les canaux attachés"
        : "Couverture de collecte live partielle : voir le détail";
    default:
      return null;
  }
}

export function eventSummary(event: AuditEvent): string {
  const activity = providerActivitySummary(event);
  if (activity) return activity;
  const failure = brokerFailureDetails(event);
  if (failure)
    return `Échec signalé · Phase : ${failure.phase} · Étape : ${failure.step} · Effet sur l’exécution : ${failure.runtimeEffect}`;
  const workspace = brokerWorkspaceDetails(event);
  if (workspace?.kind === "scan")
    return `${workspace.changedFiles} ${workspace.changedFiles === 1 ? "modification observée" : "modifications observées"}`;
  if (workspace?.kind === "export")
    return `${workspace.files} ${workspace.files === 1 ? "fichier exporté" : "fichiers exportés"}`;
  if (Array.isArray(event.command?.argv) && event.command.argv.length)
    return event.command.argv.map((arg) => asText(arg, "")).join(" ");
  for (const key of ["path", "message", "reason", "task"]) {
    const value = event.data?.[key];
    if (typeof value === "string" && value) return value;
  }
  return asText(event.decision?.reason, event.kind);
}

export function matchesFilters(
  event: AuditEvent,
  filters: EventFilters,
): boolean {
  if (filters.from || filters.to) {
    const received = event.received_at
      ? Date.parse(event.received_at)
      : Number.NaN;
    const timestamp = Number.isFinite(received)
      ? received
      : Date.parse(event.timestamp);
    if (!Number.isFinite(timestamp)) return false;
    if (filters.from && !(timestamp >= Date.parse(filters.from))) return false;
    if (filters.to && !(timestamp <= Date.parse(filters.to))) return false;
  }
  for (const key of ["agent_id", "sandbox_id", "project_id", "kind"] as const) {
    if (filters[key] && event[key] !== filters[key]) return false;
  }
  if (filters.decision && event.decision?.outcome !== filters.decision)
    return false;
  return (
    !filters.q ||
    JSON.stringify(event)
      .toLocaleLowerCase("fr")
      .includes(filters.q.toLocaleLowerCase("fr"))
  );
}

export function projectEvents(events: AuditEvent[], view: View): AuditEvent[] {
  if (view === "actions")
    return events.filter(
      (event) =>
        event.kind === "broker.batch.failure" || event.kind === "broker.batch.result" ||
        event.command ||
        event.decision ||
        /^(command|action|exec)\./.test(event.kind) ||
        /^(tool|delegation)\./.test(event.kind),
    );
  if (view === "security")
    return events.filter(
      (event) =>
        event.decision?.outcome === "deny" ||
        event.decision?.outcome === "review" ||
        /^(security|policy|audit)\./.test(event.kind),
    );
  if (view === "files")
    return events.filter(
      (event) =>
        event.kind === "broker.batch.scan" ||
        event.kind === "broker.batch.export" ||
        /^(file|files|artifact)\./.test(event.kind) ||
        /^workspace\.(diff|snapshot|export|import)/.test(event.kind),
    );
  return events;
}

export function isAuditEvent(value: unknown): value is AuditEvent {
  if (!value || typeof value !== "object") return false;
  const event = value as Partial<AuditEvent>;
  return (
    Number.isSafeInteger(event.seq) &&
    (event.seq ?? -1) >= 0 &&
    typeof event.id === "string" &&
    typeof event.timestamp === "string" &&
    typeof event.kind === "string"
  );
}
