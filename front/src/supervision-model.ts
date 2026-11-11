// Pure model of the unified supervision view. No DOM, no network, no clock of its own.
// Every relation is an exact identifier match on values the control returned; nothing is inferred.
import type { HumanBroker, HumanContext, HumanEvent, HumanResource, HumanWorker, HumanWorkerAssociation, Principal, ResourceKind } from "./human-model.ts";

/** Presentation gating only. Rust authorizes every read; a missing flag means no request is sent. */
export interface SupervisionAccess {
  workers: boolean; brokers: boolean; composed: boolean; resources: boolean; journal: boolean; content: boolean; export: boolean; any: boolean;
}
export function supervisionAccess(context: HumanContext | null): SupervisionAccess {
  const has = (capability: HumanContext["capabilities"][number]) => !!context?.capabilities.includes(capability);
  const administration = context?.mode === "administration";
  const workers = administration && has("read_worker_registry");
  const brokers = administration && has("read_broker_registry");
  const resources = has("read_metadata") || administration && has("read_tenant_metadata");
  return { workers, brokers, composed: workers && brokers, resources, journal: resources,
    content: resources && has("read_content"), export: resources && has("export"), any: workers || brokers || resources };
}
export interface AccessLine { key: "workers" | "brokers" | "resources" | "journal" | "content" | "export" | "personal_workers" | "agents"; granted: boolean; label: string; detail: string }
/** What the selected context opens and what it does not, in the user's words. */
export function accessLines(context: HumanContext | null): AccessLine[] {
  const access = supervisionAccess(context);
  const administration = context?.mode === "administration";
  const registry = (name: string, capability: string) => administration ? `Capacité ${capability} absente de ce contexte : aucune lecture du registre ${name} n’est émise.` :
    `Le registre ${name} du tenant est réservé à un contexte d’administration portant ${capability}.`;
  return [
    { key: "workers", granted: access.workers, label: "Registre des workers du tenant",
      detail: access.workers ? "Accordé par read_worker_registry : états du contrôle, dernière observation runtime et associations d’autorité." : registry("workers", "read_worker_registry") },
    { key: "brokers", granted: access.brokers, label: "Registre des brokers du tenant",
      detail: access.brokers ? "Accordé par read_broker_registry : baux, autorités déclarées et réceptions observées." : registry("brokers", "read_broker_registry") },
    { key: "resources", granted: access.resources, label: "Sessions, agents, sandboxes et projets autorisés",
      detail: access.resources ? (administration && context?.capabilities.includes("read_tenant_metadata") ?
        "Accordé par read_tenant_metadata : métadonnées des ressources de l’organisation." : "Accordé par read_metadata : ressources possédées ou partagées avec vous.") :
        "Aucune capacité de métadonnées dans ce contexte : ressources et événements ne sont pas lus." },
    { key: "journal", granted: access.journal, label: "Événements des ressources autorisées",
      detail: access.journal ? "Mêmes droits que les ressources ; chaque lecture laisse un reçu durable." : "Indisponible sans capacité de métadonnées." },
    { key: "content", granted: access.content, label: "Contenu des événements",
      detail: access.content ? "read_content accordé ; chaque ressource garde sa propre permission de contenu." : "read_content absent : seules les métadonnées sont lisibles." },
    { key: "export", granted: access.export, label: "Export d’une sélection",
      detail: access.export ? "export accordé ; chaque ressource garde sa propre permission d’export." : "export absent de ce contexte." },
    { key: "personal_workers", granted: false, label: "Vue personnelle des workers",
      detail: "Non raccordée : aucune chaîne authentifiée d’initiation et de délégation ne relie encore un worker à une personne." },
    { key: "agents", granted: false, label: "Hiérarchie agent / sous-agent",
      detail: "Relation non fournie par le contrat nominatif : elle reste inconnue ici et n’est jamais déduite d’un nom." },
  ];
}
export function principalKey(principal: Principal): string {
  return principal.kind === "human_local" ? JSON.stringify(["local", principal.installation_id, principal.account_id]) :
    JSON.stringify(["oidc", principal.issuer, principal.subject]);
}
/** Attribution of a context, independent of its opaque identifier and expiry. */
export function grantKey(context: HumanContext): string {
  return JSON.stringify([context.tenant_id, context.organization_scope, context.mode, context.membership_revision, [...context.capabilities].sort()]);
}

/** Control states, grouped by what the control recorded. "open" is not an observed execution. */
export type WorkerActivity = "open" | "uncertain" | "ended";
export function workerActivity(state: HumanWorker["state"]): WorkerActivity {
  return state === "in_doubt" ? "uncertain" : state === "terminal" || state === "cancelled" ? "ended" : "open";
}
export const workerStateLabel: Record<HumanWorker["state"], string> = { reserved: "Réservé", preparing: "Préparation", created: "Créé",
  start_issued: "Démarrage émis", running: "Exécution relevée", stopping: "Arrêt demandé", terminal: "Terminé", in_doubt: "Incertain", cancelled: "Annulé" };
export const activityLabel: Record<WorkerActivity, string> = { open: "Cycle ouvert", uncertain: "Incertain", ended: "Cycle clos" };
export const associationLabel: Record<HumanWorkerAssociation["status"], string> = { registered: "Association historique", unregistered: "Autorité non enregistrée",
  authority_unknown: "Autorité inconnue", registry_unavailable: "Registre indisponible" };
export const brokerStateLabel: Record<HumanBroker["state"], string> = { unseen: "Heartbeat non reçu", current: "Bail courant", stale: "Bail périmé",
  expired: "Autorisation expirée", revoked: "Autorisation révoquée", clock_uncertain: "Horloge incertaine" };
export const resourceKindLabel: Record<ResourceKind, string> = { session: "Session", sandbox: "Sandbox", agent: "Agent", project: "Projet" };
/** The registry keeps the last heartbeat across a new grant: it then belongs to an incarnation that is no longer the current one. */
export function heartbeatOfPreviousIncarnation(broker: HumanBroker): boolean {
  return !!broker.last_heartbeat && broker.last_heartbeat.incarnation_id !== broker.incarnation_id;
}

export type WorkerFlag = "allocation_inconsistent" | "in_doubt" | "oom_killed" | "runtime_mismatch" | "association_not_current";
export const workerFlagLabel: Record<WorkerFlag, string> = { allocation_inconsistent: "Allocation incohérente", in_doubt: "Issue incertaine",
  oom_killed: "Arrêt mémoire relevé", runtime_mismatch: "Runtime différent du broker associé", association_not_current: "Broker associé sans bail courant" };
const associations = (worker: HumanWorker) => [worker.launch_broker, worker.observation_broker];
/** Facts reported by the registry. They carry no severity and no diagnosis. */
export function workerFlags(worker: HumanWorker): WorkerFlag[] {
  const flags: WorkerFlag[] = [];
  if (worker.allocation_inconsistent) flags.push("allocation_inconsistent");
  if (worker.state === "in_doubt") flags.push("in_doubt");
  if (worker.last_observation?.oom_killed === true) flags.push("oom_killed");
  if (associations(worker).some((link) => link.status === "registered" && link.runtime_matches === false)) flags.push("runtime_mismatch");
  if (associations(worker).some((link) => link.status === "registered" && link.broker?.state !== "current")) flags.push("association_not_current");
  return flags;
}
export function workerBrokerIds(worker: HumanWorker): string[] {
  return [...new Set(associations(worker).flatMap((link) => link.broker ? [link.broker.broker_id] : []))];
}

export const SEARCH_MAX_LENGTH = 128;
/** The count of a list is announced once the person stopped typing or choosing for this long, in every view. */
export const FILTER_ANNOUNCE_MS = 500;
/** Lower-cased literal terms. No pattern syntax is interpreted. */
export function searchTerms(text: string): string[] {
  return text.slice(0, SEARCH_MAX_LENGTH).toLowerCase().split(/\s+/).filter(Boolean);
}
const matches = (terms: string[], fields: (string | null | undefined)[]) => {
  if (!terms.length) return true;
  const haystack = fields.filter((field): field is string => typeof field === "string").join("\n").toLowerCase();
  return terms.every((term) => haystack.includes(term));
};

export interface FleetFilter {
  text: string; activity: "all" | WorkerActivity; runtime: "all" | "docker" | "podman";
  /** "all", an association status of the launch authority, or an exact broker identifier. */
  broker: string; observed: "all" | "observed" | "unknown"; flagged: boolean;
}
export const defaultFleetFilter = (): FleetFilter => ({ text: "", activity: "all", runtime: "all", broker: "all", observed: "all", flagged: false });
export function isDefaultFleetFilter(filter: FleetFilter): boolean {
  return !filter.text.trim() && filter.activity === "all" && filter.runtime === "all" && filter.broker === "all" && filter.observed === "all" && !filter.flagged;
}
const unassociated = ["unregistered", "authority_unknown", "registry_unavailable"];
export function filterWorkers(workers: HumanWorker[], filter: FleetFilter): HumanWorker[] {
  const terms = searchTerms(filter.text);
  return workers.filter((worker) =>
    (filter.activity === "all" || workerActivity(worker.state) === filter.activity) &&
    (filter.runtime === "all" || worker.runtime_kind === filter.runtime) &&
    (filter.broker === "all" || (unassociated.includes(filter.broker) ? worker.launch_broker.status === filter.broker : workerBrokerIds(worker).includes(filter.broker))) &&
    (filter.observed === "all" || (worker.last_observation !== null) === (filter.observed === "observed")) &&
    (!filter.flagged || workerFlags(worker).length > 0) &&
    matches(terms, [worker.worker_id, worker.session_id, worker.request_id, worker.attempt_id, worker.state, workerStateLabel[worker.state],
      worker.runtime_kind, worker.network, worker.last_observation?.status, ...workerBrokerIds(worker),
      ...associations(worker).map((link) => link.status), ...workerFlags(worker).map((flag) => workerFlagLabel[flag])]));
}
export type WorkerOrder = "activity" | "observation" | "identifier";
const observedAt = (worker: HumanWorker) => worker.last_observation?.observed_at_status === "known" ? worker.last_observation.observed_at ?? "" : "";
const byId = (left: HumanWorker, right: HumanWorker) => left.worker_id < right.worker_id ? -1 : left.worker_id > right.worker_id ? 1 : 0;
// Known registry dates share one canonical UTC format, so their text order is their time order.
const byObservation = (left: HumanWorker, right: HumanWorker) => observedAt(left) === observedAt(right) ? 0 : observedAt(left) > observedAt(right) ? -1 : 1;
const rank: Record<WorkerActivity, number> = { open: 0, uncertain: 1, ended: 2 };
export function sortWorkers(workers: HumanWorker[], order: WorkerOrder): HumanWorker[] {
  const sorted = [...workers];
  if (order === "identifier") return sorted.sort(byId);
  if (order === "observation") return sorted.sort((left, right) => byObservation(left, right) || byId(left, right));
  return sorted.sort((left, right) => rank[workerActivity(left.state)] - rank[workerActivity(right.state)] || byObservation(left, right) || byId(left, right));
}

export interface FleetGroup {
  /** Broker identifier, or the association status of workers without a registered launch authority. */
  key: string; kind: "broker" | "unlisted_broker" | "unregistered" | "authority_unknown" | "registry_unavailable";
  broker: HumanBroker | null; workers: HumanWorker[];
}
/**
 * Brokers received from the control keep their row even without any worker in the loaded window.
 * A worker joins a broker only when one of its two authority associations names that exact broker.
 * Such an association is historical: it never proves that this broker ran or inspected the worker.
 */
export function joinFleet(brokers: HumanBroker[] | null, workers: HumanWorker[]): FleetGroup[] {
  const groups = new Map<string, FleetGroup>();
  for (const broker of brokers ?? []) groups.set(broker.broker_id, { key: broker.broker_id, kind: "broker", broker, workers: [] });
  const rest: FleetGroup[] = [];
  const other = (key: string, kind: FleetGroup["kind"]) => {
    let group = groups.get(key);
    if (!group) { group = { key, kind, broker: null, workers: [] }; groups.set(key, group); rest.push(group); }
    return group;
  };
  for (const worker of workers) {
    for (const id of workerBrokerIds(worker)) other(id, "unlisted_broker").workers.push(worker);
    const launch = worker.launch_broker.status;
    if (launch !== "registered") other(launch, launch).workers.push(worker);
  }
  const order = ["unlisted_broker", "unregistered", "authority_unknown", "registry_unavailable"];
  return [...[...groups.values()].filter((group) => group.kind === "broker"),
    ...rest.sort((left, right) => order.indexOf(left.kind) - order.indexOf(right.kind) || (left.key < right.key ? -1 : 1))];
}

export function filterBrokers(brokers: HumanBroker[], text: string): HumanBroker[] {
  const terms = searchTerms(text);
  return brokers.filter((broker) => matches(terms, [broker.broker_id, broker.incarnation_id, broker.state, brokerStateLabel[broker.state],
    ...broker.authorities.flatMap((authority) => [authority.authority_id, authority.runtime_kind])]));
}
export interface ResourceFilter { text: string; kind: "all" | ResourceKind }
export function filterResources(resources: HumanResource[], filter: ResourceFilter): HumanResource[] {
  const terms = searchTerms(filter.text);
  return resources.filter((item) => (filter.kind === "all" || item.resource.kind === filter.kind) &&
    matches(terms, [item.resource.resource_id, item.resource.kind, resourceKindLabel[item.resource.kind],
      item.owner.kind === "human_local" ? item.owner.account_id : item.owner.subject]));
}
export interface EventFilter { text: string; decision: "all" | "allow" | "deny" | "review" | "none"; provenance: "all" | HumanEvent["metadata"]["provenance"]; kind: "all" | ResourceKind }
export const defaultEventFilter = (): EventFilter => ({ text: "", decision: "all", provenance: "all", kind: "all" });
export function filterEvents(events: HumanEvent[], filter: EventFilter): HumanEvent[] {
  const terms = searchTerms(filter.text);
  return events.filter(({ metadata }) =>
    (filter.decision === "all" || (filter.decision === "none" ? metadata.decision === null : metadata.decision === filter.decision)) &&
    (filter.provenance === "all" || metadata.provenance === filter.provenance) &&
    (filter.kind === "all" || metadata.resource.kind === filter.kind) &&
    matches(terms, [metadata.id, metadata.kind, metadata.source, metadata.resource.resource_id, metadata.resource.kind, metadata.decision, metadata.provenance, metadata.coverage]));
}
/** A declared session reference links to a resource only when kind and identifier are exactly equal. */
export function sessionResource(resources: HumanResource[], sessionId: string): HumanResource | null {
  return resources.find((item) => item.resource.kind === "session" && item.resource.resource_id === sessionId) ?? null;
}
export function workersOfSession(workers: HumanWorker[], sessionId: string): HumanWorker[] {
  return workers.filter((worker) => worker.session_id === sessionId);
}

const countNouns = { workers: ["worker", "workers", ""], brokers: ["broker", "brokers", ""], resources: ["ressource", "ressources", "e"], events: ["événement", "événements", ""] } as const;
export type CountNoun = keyof typeof countNouns;
/** "n displayed of N loaded", agreed in number and gender; zero and one are singular. */
export function shownOfLoaded(noun: CountNoun, shown: number, loaded: number): string {
  const [one, many, feminine] = countNouns[noun];
  return `${shown} ${shown > 1 ? many : one} affiché${feminine}${shown > 1 ? "s" : ""} sur ${loaded} chargé${feminine}${loaded > 1 ? "s" : ""}`;
}
export function windowSummary(input: { shown: number; loaded: number; hasMore: boolean; noun: CountNoun }): string {
  return `${shownOfLoaded(input.noun, input.shown, input.loaded)} · ${input.hasMore ?
    "d’autres pages existent au contrôle, aucun total du tenant n’est connu" : "fin du registre à cette lecture"}`;
}

export type SourcePhase = "idle" | "loading" | "ready" | "error" | "forbidden" | "not_granted";
export interface SourceSummary {
  phase: SourcePhase; loaded: number; hasMore: boolean; stale: boolean; capped: boolean;
  /** Rows removed after a refusal whose reason could not be read: neither a failed read nor an empty registry. */
  withdrawn?: boolean;
}
export interface CoverageNote { source: "workers" | "brokers" | "resources" | "contexts" | "runtime"; tone: "neutral" | "amber"; text: string }
export interface FleetCoverage { partial: true; notes: CoverageNote[] }
const sourceNames = { workers: ["Workers", "worker"], brokers: ["Brokers", "broker"], resources: ["Ressources", "ressource"] } as const;
function sourceNote(source: "workers" | "brokers" | "resources", summary: SourceSummary, exactView: string): CoverageNote {
  const [title, singular] = sourceNames[source];
  const registry = source === "resources" ? "accès non accordé" : "registre non accordé";
  const plural = summary.loaded > 1 ? "s" : "";
  const loaded = `${summary.loaded} ligne${plural} chargée${plural}`;
  const text = summary.phase === "not_granted" ? `${registry} à ce contexte, aucune lecture émise.` :
    summary.phase === "forbidden" ? "lecture refusée par le contrôle, données retirées." :
    summary.withdrawn ? "données retirées après un refus sans motif lisible, contenu inconnu tant que vos accès ne sont pas relus." :
    summary.phase === "loading" && !summary.loaded ? "lecture en cours, contenu encore inconnu." :
    summary.phase === "idle" ? "aucune lecture encore émise, contenu inconnu." :
    summary.phase === "error" && !summary.loaded ? "lecture en échec, contenu inconnu. Une erreur n’est pas une absence d’activité." :
    summary.stale || summary.phase === "error" ? `${summary.loaded} ligne${plural} de la dernière lecture réussie, périmée${plural}.` :
    !summary.loaded ? `${source === "resources" ? "aucune" : "aucun"} ${singular} ${source === "resources" ? "autorisée" : "enregistré"} à cette lecture réussie.` :
    summary.capped ? `${loaded}, fenêtre de cette vue atteinte ; utilisez la vue ${exactView} pour la suite.` :
    summary.hasMore ? `${loaded}, d’autres pages existent au contrôle.` :
    `${loaded}, fin du registre à cette lecture.`;
  const settled = summary.phase === "ready" && !summary.stale && !summary.hasMore;
  return { source, tone: settled ? "neutral" : "amber", text: `${title} : ${text}` };
}
/**
 * The composition is always partial: it joins independent registry snapshots and never a runtime inventory.
 * A failed read, a source the context does not grant and a successful empty read stay three different facts.
 */
export function fleetCoverage(input: { workers: SourceSummary; brokers: SourceSummary; resources: SourceSummary; contextsComplete: boolean }): FleetCoverage {
  const notes: CoverageNote[] = [sourceNote("workers", input.workers, "Workers"), sourceNote("brokers", input.brokers, "Brokers"), sourceNote("resources", input.resources, "Ressources")];
  if (!input.contextsComplete) notes.push({ source: "contexts", tone: "amber", text: "Contextes : liste partielle, d’autres périmètres peuvent manquer." });
  notes.push({ source: "runtime", tone: "amber", text: "Inventaire runtime : inconnu. Ces registres ne prouvent ni exécution, ni arrêt, ni libération de quota." });
  return { partial: true, notes };
}

export interface AnnouncedSource extends SourceSummary { busy: boolean }
/**
 * One short sentence for assistive technology, or null while a read is in flight. It carries neither a time
 * nor a count: an unchanged state gives the same sentence, so a successful refresh cycle announces nothing.
 */
export function sourceAnnouncement(sources: { workers: AnnouncedSource; brokers: AnnouncedSource; resources: AnnouncedSource }): string | null {
  const granted = (["workers", "brokers", "resources"] as const).filter((key) => sources[key].phase !== "not_granted");
  if (granted.some((key) => sources[key].busy || sources[key].phase === "loading" || sources[key].phase === "idle")) return null;
  const groups = new Map<string, string[]>();
  for (const key of granted) {
    const item = sources[key];
    const label = item.phase === "forbidden" ? "lecture refusée, données retirées" : item.withdrawn ? "données retirées, accès à relire" :
      item.phase === "error" ? "lecture en échec, contenu inconnu" : item.stale ? "données périmées, dernière lecture conservée" : "lecture réussie";
    groups.set(label, [...(groups.get(label) ?? []), sourceNames[key][0].toLowerCase()]);
  }
  return [...groups].map(([label, names]) => { const list = names.join(", "); return `${list[0]!.toUpperCase()}${list.slice(1)} : ${label}.`; }).join(" ");
}

export type Freshness = "never" | "fresh" | "aging" | "stale";
/** Below the 300-second life of a context, so a window left in manual mode can be seen ageing before it is removed. */
export const MANUAL_AGING_MS = 120000;
/** "aging" after twice the refresh cadence, or two minutes when refresh is manual. `loadedAt` is the oldest read of the window. */
export function freshness(input: { loadedAt: number | null; now: number; cadence: number; stale: boolean }): Freshness {
  if (input.loadedAt === null) return "never";
  if (input.stale) return "stale";
  return input.now - input.loadedAt > (input.cadence > 0 ? 2 * input.cadence : MANUAL_AGING_MS) ? "aging" : "fresh";
}
