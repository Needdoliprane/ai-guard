import type {
  Agent,
  AgentNode,
  AuditEvent,
  EventFilters,
  EventPeriod,
  View,
} from "./types.ts";

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

export function eventSummary(event: AuditEvent): string {
  const failure = brokerFailureDetails(event);
  if (failure)
    return `Échec signalé · Phase : ${failure.phase} · Étape : ${failure.step} · Effet sur l’exécution : ${failure.runtimeEffect}`;
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
        event.kind === "broker.batch.failure" ||
        event.command ||
        event.decision ||
        /^(command|action|exec)\./.test(event.kind),
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
