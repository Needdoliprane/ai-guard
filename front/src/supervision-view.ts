// DOM of the unified supervision view. State and reads live in SupervisionStore; relations and filters in supervision-model.
// Text is always inserted as text nodes. No fixture, counter or status is created here: every value comes from a control read.
// Nodes are kept across publications (see dom-sync): a row, a button, an error line or a section of an open detail
// that did not change is not rebuilt.
import { button, el, icon } from "./dom.ts";
import { announce, place, replaceIfChanged, setText, signatureOf, syncRows, syncSections } from "./dom-sync.ts";
import { formatDate } from "./model.ts";
import { CADENCES, SupervisionStore, sessionAnnouncement, summarize } from "./supervision-store.ts";
import { checkpointFacts, checkpointLabel, formatBrokerDate } from "./human-owned-brokers.ts";
import { accessLines, activityLabel, associationLabel, brokerAuthorityLabel, brokerStateLabel, defaultFleetFilter, FILTER_ANNOUNCE_MS, filterBrokers, filterResources, filterWorkers, fleetCoverage, freshness,
  heartbeatOfPreviousIncarnation, isDefaultFleetFilter, joinFleet, resourceKindLabel, SEARCH_MAX_LENGTH, sessionResource, shownOfLoaded, sortWorkers, sourceAnnouncement, windowSummary,
  workerActivity, workerFlagLabel, workerFlags, workersOfSession, workerStateLabel } from "./supervision-model.ts";
import "./supervision.css";
import type { HumanBroker, HumanContext, HumanEvent, HumanResource, HumanWorker, HumanWorkerAssociation, HumanWorkerTimeStatus, ResourceKind, ResourceRef } from "./human-model.ts";
import type { FleetFilter, FleetGroup, ResourceFilter, WorkerOrder } from "./supervision-model.ts";
import type { Source, SourceError, SupervisionClock, SupervisionReads } from "./supervision-store.ts";

/** What the user typed or chose. It holds no server data and lives as long as the selected context. */
export interface SupervisionPreferences { fleet: FleetFilter; order: WorkerOrder; brokers: { text: string }; resources: ResourceFilter; cadence: number; rendered: number }
export const RENDER_STEP = 200;
export const defaultSupervisionPreferences = (): SupervisionPreferences =>
  ({ fleet: defaultFleetFilter(), order: "activity", brokers: { text: "" }, resources: { text: "", kind: "all" }, cadence: CADENCES[0], rendered: RENDER_STEP });
export interface SupervisionHost {
  reads: SupervisionReads; context: HumanContext; preferences: SupervisionPreferences;
  /** False while the control reports that its list of contexts may be missing some scopes. */
  contextsComplete: () => boolean;
  /** Leaves the view for the context list, where the control's current offer is read again. */
  onReviewAccess: () => void;
  /** A read was refused without a readable reason: the host reads the identity once, then refreshes this view if the context still stands. */
  onAccessDoubt: () => void;
  /** Updates the host's existing read command without rebuilding its navigation or moving the focus. */
  onReadStateChange: () => void;
  /** Opens an existing exact registry view, when the dashboard offers it for this context. */
  openRegistry: (view: "workers" | "brokers" | "resources") => void;
  hasRegistry: (view: "workers" | "brokers" | "resources") => boolean;
  /** Opens the Journal limited to one resource, when this context can read events. */
  openJournal: ((resource: ResourceRef) => void) | null;
}
type Detail = { kind: "worker"; id: string } | { kind: "broker"; id: string } | { kind: "resource"; resourceKind: ResourceKind; id: string };
/** The three lists of the view, in the order their counts are said. */
type List = "workers" | "brokers" | "resources";
const LISTS: readonly List[] = ["workers", "brokers", "resources"];

const browserClock: SupervisionClock = { now: () => Date.now(), visible: () => document.visibilityState === "visible",
  setTimer: (run, milliseconds) => setTimeout(run, milliseconds), clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>) };
function badge(label: string, tone = "neutral") { return el("span", `badge badge-${tone}`, label); }
function action(label: string, key: string, call: () => void, disabled = false) {
  const node = button(label, "button button-secondary", call); node.dataset.focusKey = key; node.disabled = disabled; return node;
}
/**
 * A control that starts a read stays focusable while a read is in flight: it is announced as unavailable
 * and ignores activation, but the keyboard focus does not leave it. Only a lasting stop disables it.
 */
function reader(label: string, key: string, call: () => void) {
  const node = action(label, key, () => { if (node.getAttribute("aria-disabled") !== "true") call(); });
  return node;
}
/** `busy`: its read is in flight. `blocked`: a lasting stop, such as the durable read budget. */
function waiting(node: HTMLButtonElement, busy: boolean, blocked: boolean) { node.setAttribute("aria-disabled", String(busy)); node.disabled = blocked; return node; }
/** Updates a read control; true when its read just ended while it holds the focus, so the caller can keep it in sight. */
function settled(node: HTMLButtonElement, busy: boolean, blocked: boolean) {
  const was = node.getAttribute("aria-disabled") === "true"; waiting(node, busy, blocked);
  return was && !busy && document.activeElement === node;
}
/** What replaces a footer control once there is nothing more to read: it can take the focus that control held. */
function endNote(text: string) { const node = el("span", "muted sv-end", text); node.tabIndex = -1; return node; }
function time(value: string | null, status: HumanWorkerTimeStatus | "known" | "unknown") {
  if (status === "absent") return "Non fourni";
  if (status === "unknown") return "Inconnu";
  return value && Number.isFinite(Date.parse(value)) ? formatDate(value, true) : value ?? "Inconnu";
}
const clockTime = (milliseconds: number) => formatDate(new Date(milliseconds).toISOString(), true);
function field<K extends "input" | "select">(label: string, control: HTMLElementTagNameMap[K], id: string) {
  control.id = id; const caption = el("label", "", label); caption.htmlFor = id;
  return el("div", "sv-field", caption, control);
}
function select<T extends string>(options: [T, string][], value: T, change: (value: T) => void) {
  const node = el("select", "select-input");
  for (const [key, label] of options) { const option = el("option", "", label); option.value = key; node.append(option); }
  node.value = value; node.addEventListener("change", () => change(node.value as T));
  return node;
}
function searchInput(value: string, placeholder: string, change: (value: string) => void) {
  const node = el("input", "search-input sv-search"); node.type = "search"; node.maxLength = SEARCH_MAX_LENGTH; node.value = value;
  node.autocomplete = "off"; node.spellcheck = false; node.placeholder = placeholder;
  node.addEventListener("input", () => change(node.value));
  return node;
}
function cell(label: string, ...children: (Node | string | null)[]) {
  const node = el("td", "", ...children); node.dataset.label = label; node.setAttribute("role", "cell"); return node;
}
function tableRow(...cells: HTMLElement[]) { const node = el("tr", "", ...cells); node.setAttribute("role", "row"); return node; }
interface DenseTable { node: HTMLTableElement; caption: HTMLElement; body: HTMLElement }
/** One table per list for the life of the view: only its caption text and its rows change. */
function denseTable(headers: string[], modifier = ""): DenseTable {
  const node = el("table", `sv-table ${modifier}`.trim()); node.setAttribute("role", "table");
  const head = el("thead", "", el("tr", "", ...headers.map((title) => { const th = el("th", "", title); th.scope = "col"; th.setAttribute("role", "columnheader"); return th; })));
  const body = el("tbody"); const caption = el("caption");
  for (const group of [head, body]) group.setAttribute("role", "rowgroup");
  head.firstElementChild?.setAttribute("role", "row");
  node.append(caption, head, body);
  return { node, caption, body };
}
function emptyState(title: string, description: string) { return el("div", "empty-state sv-empty", el("h3", "", title), el("p", "", description)); }
function definition(entries: [string, Node | string | null][]) {
  return el("dl", "sv-definition", ...entries.filter(([, value]) => value !== null).map(([name, value]) => el("div", "", el("dt", "", name), el("dd", "", value))));
}
/** The result line of a list. It can take the focus; it is not a live region: the banner region says its count. */
function summaryLine() { const node = el("p", "sv-summary"); node.tabIndex = -1; return node; }
const cadenceLabel = (value: number) => value === 0 ? "Manuel" : value >= 60000 ? `${value / 60000} min` : `${value / 1000} s`;

export class SupervisionView {
  readonly element = el("div", "sv");
  #host: SupervisionHost; #store: SupervisionStore; #preferences: SupervisionPreferences;
  #status = el("section", "sv-status"); #statusFacts = el("div", "sv-facts"); #access = el("details", "sv-access");
  /**
   * The only live region of the view: one short sentence when the state of a source changes, and the count of
   * a list when a read changed it or once the person paused after a search or a filter. A result line is
   * rewritten at every keystroke and is not a live region itself.
   */
  #announce = el("p", "sr-only sv-announce"); #announced = ""; #sentence: string | null = null;
  /**
   * A read the person asked for is still to be answered: when it ends, the region says how it ended even
   * if that read changed nothing. `#answering` is set once a read is seen in flight after the ask.
   * `#alerted`: an error line appeared or changed since the ask, and announced the failure as an alert.
   * `#failure`: the messages of the errors displayed, or null.
   */
  #asked = false; #answering = false; #alerted = false; #failure: string | null = null;
  /** The count of each list as displayed and as last said; empty while the list shows no count. */
  #counts: Record<List, string> = { workers: "", brokers: "", resources: "" }; #countsSaid: Record<List, string> = { workers: "", brokers: "", resources: "" };
  /** Lists whose search or filter the person just changed: their count is said by the pause, not by a read. */
  #paused = new Set<List>(); #pauseTimer: ReturnType<typeof setTimeout> | undefined;
  #sourcesBox = el("div", "sv-sources"); #errorsBox = el("div", "sv-errors"); #coverageBox = el("div", "sv-coverage");
  #refreshButton = reader("Actualiser maintenant", "sv-refresh", () => { void this.#ask(() => this.#store.refresh(false)); }); #autoNote = el("p", "muted sv-auto");
  #brokers = el("section", "panel sv-panel"); #workers = el("section", "panel sv-panel"); #resources = el("section", "panel sv-panel");
  #brokerTools = el("div", "sv-toolbar sv-toolbar-short"); #brokerSummary = summaryLine(); #brokerBody = el("div", "sv-body");
  #brokerTable = denseTable(["Broker", "État du bail", "Autorités déclarées", "Dernière réception observée", "Workers associés", "Détail"]);
  #brokerEmpty = el("div"); #brokerGroups = el("ul", "sv-groups"); #brokerFooter = el("div", "table-footer");
  #workerTools = el("div", "sv-toolbar"); #workerSummary = summaryLine(); #workerBody = el("div", "sv-body");
  #workerTable = denseTable(["Worker / session déclarée", "État du contrôle", "Dernière observation runtime", "Autorité du lancement", "Points d’attention", "Détail"], "sv-table-workers");
  #workerEmpty = el("div"); #workerFooter = el("div", "table-footer sv-footer");
  #renderMore = action("", "sv-render-more", () => this.#showMoreWorkers());
  #loadMore = reader("Charger 100 lignes de plus", "sv-load-more", () => { void this.#ask(() => this.#store.loadMoreWorkers()); });
  #workerCap = endNote("Fenêtre de cette vue atteinte (500 lignes)."); #workerEnd = endNote("Fin du registre à cette lecture");
  #openWorkers = action("Ouvrir le registre exact des workers", "sv-open-workers", () => this.#host.openRegistry("workers"));
  #resourceTools = el("div", "sv-toolbar sv-toolbar-short"); #resourceSummary = summaryLine(); #resourceBody = el("div", "sv-body");
  #resourceTable = denseTable(["Type", "Identifiant", "Propriétaire", "Droits de ce contexte", "Corrélation", "Détail"]);
  #resourceEmpty = el("div"); #resourceFooter = el("div", "table-footer sv-footer");
  #loadResources = reader("Charger 100 ressources de plus", "sv-load-resources", () => { void this.#ask(() => this.#store.loadMoreResources()); });
  #resourceCap = endNote("Fenêtre de cette vue atteinte (300 lignes).");
  #openResources = action("Ouvrir la vue Ressources (contenu, export)", "sv-open-resources", () => this.#host.openRegistry("resources"));
  #brokerSelect: HTMLSelectElement | null = null;
  #dialog: HTMLDialogElement | null = null; #detail: Detail | null = null; #dialogBody: HTMLElement | null = null;
  /** The only live region of an open detail: one short sentence when a read it asked for ends. */
  #dialogAnnounce: HTMLElement | null = null; #dialogSaid = "";
  /** Where the focus goes when the open detail closes because of an action taken inside it. */
  #afterDialog: HTMLElement | null = null;
  #ticker: ReturnType<typeof setInterval> | undefined;
  #closed = false;

  constructor(host: SupervisionHost) {
    this.#host = host; this.#preferences = host.preferences;
    this.#store = new SupervisionStore(host.reads, host.context, browserClock, () => { this.#render(); host.onReadStateChange(); }, () => host.onAccessDoubt());
    if (this.#preferences.cadence !== CADENCES[0]) this.#store.setCadence(this.#preferences.cadence);
    this.#announce.setAttribute("role", "status"); this.#announce.setAttribute("aria-live", "polite");
    this.#statusFacts.tabIndex = -1; this.#statusFacts.append(this.#sourcesBox, this.#errorsBox, this.#coverageBox);
    const access = this.#store.state.access;
    this.#buildStatusControls(); this.#buildAccess(); this.#buildBrokerPanel(); this.#buildWorkerPanel(); this.#buildResourcePanel();
    this.element.append(this.#status, this.#access);
    if (access.brokers || access.workers) this.element.append(this.#brokers);
    if (access.workers || access.workersUnavailable) this.element.append(this.#workers);
    this.element.append(this.#resources);
    this.#render();
  }
  start() {
    void this.#store.open();
    // Freshness is a function of time: re-evaluate the banner without any read. Nothing is rewritten while it says the same.
    this.#ticker = setInterval(() => { if (!this.#closed) this.#renderStatus(); }, 10000);
  }
  /** For the host, on an explicit action of the person. While a read is in flight it starts nothing: that read answers. */
  refresh() { return this.reading ? Promise.resolve() : this.#ask(() => this.#store.refresh(false)); }
  get reading() { return this.#reading() || this.#store.state.session?.phase === "loading"; }
  /** For the host itself, once it read the accesses again after a refusal: what that refusal withdrew is read again. */
  reread() { return this.#store.refresh(false); }
  #reading() { const state = this.#store.state; return [state.workers, state.brokers, state.resources].some((item) => item.busy); }
  /** Starts a read the person asked for: whatever it changes, the banner region says how it ended. */
  #ask(read: () => Promise<void>): Promise<void> {
    this.#asked = true; this.#alerted = false;
    const done = read();
    // A request that started no read, with none in flight, has no outcome to say.
    void done.then(() => { if (!this.#answering) this.#asked = false; });
    return done;
  }
  /** The durable read budget stopped: let the store record why its automatic cycle no longer runs. */
  budgetStopped() { void this.#store.refresh(true); }
  suspend() { this.#store.suspend(); }
  resume() { this.#store.resume(); }
  close() {
    this.#closed = true; clearInterval(this.#ticker); clearTimeout(this.#pauseTimer); this.#store.close();
    this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null; this.#detail = null;
    this.element.replaceChildren();
  }

  #buildStatusControls() {
    const access = this.#store.state.access;
    // The automatic cycle reads the registries only: without any of them there is no period to choose.
    const cadence = access.workers || access.brokers ? field("Rythme d’actualisation", select<string>(CADENCES.map((value): [string, string] => [String(value), cadenceLabel(value)]),
      String(this.#store.state.cadence), (value) => { this.#preferences.cadence = Number(value); this.#store.setCadence(Number(value)); }), "sv-cadence") : null;
    this.#status.append(this.#announce, this.#statusFacts, el("div", "sv-controls", cadence, this.#refreshButton, this.#autoNote));
  }
  #buildAccess() {
    const context = this.#host.context;
    this.#access.append(el("summary", "", icon("chevron"), el("span", "", "Accès de ce contexte")),
      el("p", "muted", `Capacités fournies par le contrôle : ${context.capabilities.join(", ") || "aucune"}. Le navigateur n’en déduit aucune autre ; chaque lecture est autorisée de nouveau par le serveur.`),
      el("ul", "sv-access-list", ...accessLines(context, this.#host.reads.readProfiles).map((line) => el("li", "", badge(line.granted ? "Accordé" : "Non disponible", line.granted ? "teal" : "neutral"),
        el("div", "", el("strong", "", line.label), el("p", "muted", line.detail))))));
  }
  #buildBrokerPanel() {
    const filter = this.#preferences.brokers;
    const search = searchInput(filter.text, "Identifiant, incarnation, état, autorité", (value) => { filter.text = value; this.#renderBrokers(); this.#countAfterPause("brokers"); });
    this.#brokerTools.append(field("Rechercher dans les brokers chargés", search, "sv-broker-search"));
    this.#brokers.append(el("div", "panel-heading", el("div", "", el("h2", "", "Brokers et autorités"),
      el("p", "", "Registre central du tenant : baux et autorités déclarées. Un bail courant ne prouve aucun worker en fonctionnement ; un bail périmé ou révoqué ne prouve ni arrêt ni libération de quota."))));
    if (this.#store.state.access.brokers) this.#brokers.append(this.#brokerTools, this.#brokerSummary);
    this.#brokers.append(this.#brokerBody);
    this.#brokerFooter.append(action("Ouvrir le registre exact des brokers", "sv-open-brokers", () => this.#host.openRegistry("brokers")));
  }
  #buildWorkerPanel() {
    if (this.#store.state.access.workersUnavailable) {
      this.#workers.append(el("div", "panel-heading", el("h2", "", "Workers · profil indisponible")),
        emptyState("Registre des workers indisponible pour ce profil", "La permission read_worker_registry est présente, mais le contrôle annonce worker_registry unavailable et federation_registry unavailable. Aucune route historique n’est appelée. L’activité et les associations restent inconnues ; les checkpoints ne remplacent pas ce registre."),
        el("p", "sv-note muted", "Faites vérifier par l’opérateur les profils de lecture pris en charge. Relire les accès vérifie une nouvelle annonce du contrôle ; cette action n’active aucune capacité."),
        action("Relire mes accès", "sv-profile-access", () => this.#host.onReviewAccess()));
      return;
    }
    const filter = this.#preferences.fleet;
    const search = searchInput(filter.text, "Identifiant, session, état, broker", (value) => { filter.text = value; this.#filtersChanged(); });
    this.#brokerSelect = select<string>([["all", "Tous"]], "all", (value) => { filter.broker = value; this.#filtersChanged(); });
    const flagged = action("Points d’attention", "sv-flagged", () => {
      filter.flagged = !filter.flagged; flagged.setAttribute("aria-pressed", String(filter.flagged)); this.#filtersChanged();
    });
    flagged.classList.add("sv-toggle"); flagged.setAttribute("aria-pressed", String(filter.flagged));
    const activity = select<FleetFilter["activity"]>([["all", "Tous"], ["open", activityLabel.open], ["uncertain", activityLabel.uncertain], ["ended", activityLabel.ended]],
      filter.activity, (value) => { filter.activity = value; this.#filtersChanged(); });
    const runtime = select<FleetFilter["runtime"]>([["all", "Tous"], ["docker", "docker"], ["podman", "podman"]], filter.runtime, (value) => { filter.runtime = value; this.#filtersChanged(); });
    const observed = select<FleetFilter["observed"]>([["all", "Toutes"], ["observed", "Relevée"], ["unknown", "Inconnue"]], filter.observed, (value) => { filter.observed = value; this.#filtersChanged(); });
    // A sort changes no count: nothing is announced for it.
    const order = select<WorkerOrder>([["activity", "État puis relevé récent"], ["observation", "Relevé récent"], ["identifier", "Identifiant"]], this.#preferences.order,
      (value) => { this.#preferences.order = value; this.#filtersChanged(false); });
    const reset = action("Réinitialiser", "sv-reset", () => {
      Object.assign(filter, defaultFleetFilter()); this.#preferences.order = "activity";
      search.value = ""; activity.value = "all"; runtime.value = "all"; observed.value = "all"; order.value = "activity";
      if (this.#brokerSelect) this.#brokerSelect.value = "all";
      flagged.setAttribute("aria-pressed", "false"); this.#filtersChanged(); search.focus();
    });
    this.#workerTools.append(field("Rechercher dans les lignes chargées", search, "sv-worker-search"), field("État du contrôle", activity, "sv-worker-activity"),
      field("Runtime déclaré", runtime, "sv-worker-runtime"), field("Broker ou autorité", this.#brokerSelect, "sv-worker-broker"),
      field("Observation runtime", observed, "sv-worker-observed"), field("Tri", order, "sv-worker-order"), el("div", "sv-tool-actions", flagged, reset));
    this.#workers.append(
      el("div", "panel-heading", el("div", "", el("h2", "", "Workers"),
        el("p", "", "Registre durable du tenant. L’état du contrôle et la dernière observation runtime restent deux faits distincts ; un cycle ouvert n’est pas une exécution observée. Consommation réelle inconnue, allocation non facturable."))),
      this.#workerTools, this.#workerSummary, this.#workerBody,
      el("p", "sv-note muted", "Le registre est trié par identifiant côté contrôle, sans filtre d’état : au-delà de la fenêtre chargée, des workers à cycle ouvert peuvent exister sans apparaître ici."));
  }
  #buildResourcePanel() {
    const filter = this.#preferences.resources;
    const search = searchInput(filter.text, "Identifiant ou propriétaire", (value) => { filter.text = value; this.#renderResources(); this.#countAfterPause("resources"); });
    const kind = select<ResourceFilter["kind"]>([["all", "Toutes"], ["session", "Sessions"], ["agent", "Agents"], ["sandbox", "Sandboxes"], ["project", "Projets"]], filter.kind,
      (value) => { filter.kind = value; this.#renderResources(); this.#countAfterPause("resources"); });
    this.#resourceTools.append(field("Rechercher dans les ressources chargées", search, "sv-resource-search"), field("Type", kind, "sv-resource-kind"));
    if (!this.#store.state.access.resources) {
      this.#resources.append(el("div", "panel-heading", el("div", "", el("h2", "", "Ressources autorisées"),
        el("p", "", "Ce contexte ne porte aucune capacité de métadonnées : sessions, agents, sandboxes, projets et leurs événements ne sont pas lus."))));
      return;
    }
    this.#resources.append(
      el("div", "panel-heading", el("div", "", el("h2", "", "Ressources autorisées"),
        el("p", "", "Sessions, agents, sandboxes et projets que ce contexte peut lire. Une ressource visible ne donne aucun droit implicite sur son contenu ou son export."))),
      this.#resourceTools, this.#resourceSummary, this.#resourceBody);
  }
  #filtersChanged(announce = true) {
    this.#preferences.rendered = RENDER_STEP; this.#renderWorkers(); this.#renderBrokers();
    if (announce) this.#countAfterPause("workers");
  }
  /** Shows the workers of one broker or association status, from a broker row or a detail. */
  #filterBroker(key: string) {
    this.#preferences.fleet.broker = key; if (this.#brokerSelect) this.#brokerSelect.value = key;
    // From a detail, its closing would hand the focus back to the broker row: the filtered result takes it instead.
    this.#afterDialog = this.#dialog?.open ? this.#workerSummary : null;
    // The result line takes the focus and is read from there: its count is not announced as well.
    this.#closeDialog(); this.#filtersChanged(false); this.#paused.delete("workers"); this.#countsSaid.workers = this.#counts.workers;
    this.#workerSummary.focus();
  }
  /** The person changed a search or a filter of this list: its count is said once, after they paused. */
  #countAfterPause(list: List) {
    this.#paused.add(list); clearTimeout(this.#pauseTimer);
    this.#pauseTimer = setTimeout(() => {
      this.#pauseTimer = undefined;
      if (this.#closed) return;
      const parts = LISTS.filter((key) => this.#paused.has(key)).map((key) => this.#countsSaid[key] = this.#counts[key]).filter(Boolean);
      this.#paused.clear();
      if (parts.length) announce(this.#announce, parts.join(" "));
    }, FILTER_ANNOUNCE_MS);
  }
  /**
   * After a publication of the store, in one write: the state of the sources when it changed, then the
   * count of each list that a read changed. Nothing is said while a read is in flight, nor for an
   * automatic refresh that changes neither a state nor a count. A read the person asked for always says
   * how it ended. When it changed nothing and no new alert reported it, the region says again the state
   * of the sources, or the error still displayed: a state alone would not tell a failure that leaves
   * the loaded rows as they were.
   */
  #announceRead() {
    if (this.#sentence === null) { if (this.#asked) this.#answering = true; return; }
    const outcome = this.#answering;
    if (outcome) { this.#asked = false; this.#answering = false; }
    const parts: string[] = [];
    if (this.#sentence !== this.#announced) { this.#announced = this.#sentence; parts.push(this.#sentence); }
    for (const key of LISTS) {
      if (this.#paused.has(key) || this.#counts[key] === this.#countsSaid[key]) continue;
      this.#countsSaid[key] = this.#counts[key];
      if (this.#counts[key]) parts.push(this.#counts[key]);
    }
    if (outcome && !parts.length && !this.#alerted) parts.push(this.#failure ?? this.#sentence);
    if (parts.length) announce(this.#announce, parts.join(" "));
  }
  /** Renders one more step of loaded rows, without any read, and continues on the first of them. */
  #showMoreWorkers() {
    const previous = this.#preferences.rendered; this.#preferences.rendered += RENDER_STEP; this.#renderWorkers();
    (this.#workerTable.body.children[previous]?.querySelector<HTMLElement>("[data-focus-key]") ?? this.#workerSummary).focus();
  }

  #render() {
    if (this.#closed) return;
    const scope = this.element.contains(document.activeElement) ? document.activeElement as HTMLElement : null;
    const key = scope?.dataset.focusKey; const region = scope?.closest<HTMLElement>(".sv-panel") ?? null; const footer = scope?.closest<HTMLElement>(".sv-footer") ?? null;
    this.#renderStatus(); this.#renderBrokers(); this.#renderWorkers(); this.#renderResources(); this.#renderDialog();
    this.#announceRead();
    if (scope && !scope.isConnected && !this.element.contains(document.activeElement)) this.#refocus(key, region, footer);
  }
  /** The first place of the view that can always take the focus. */
  #anchor(): HTMLElement { return [this.#workerSummary, this.#brokerSummary, this.#resourceSummary].find((node) => node.isConnected) ?? this.#statusFacts; }
  /** A control that left the page hands its focus to its replacement, else to what now stands where the person was. */
  #refocus(key: string | undefined, region: HTMLElement | null, footer: HTMLElement | null) {
    const replacement = key ? [...this.element.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === key && !node.matches(":disabled")) : null;
    if (replacement) { replacement.focus({ preventScroll: true }); return; }
    // A footer control with nothing more to read is replaced by a note; a row that left the window, by the result summary of its list.
    const end = footer?.isConnected ? footer.querySelector<HTMLElement>(".sv-end") : null;
    const summary = region?.isConnected ? region.querySelector<HTMLElement>(".sv-summary") : null;
    (end ?? summary ?? (this.#refreshButton.disabled ? this.#statusFacts : this.#refreshButton)).focus();
  }
  #freshnessBadge(label: string, state: Source<{ items: unknown[]; hasMore: boolean; oldestReadAt?: number }>) {
    const cadence = this.#store.state.cadence;
    // A window of several pages is as fresh as the oldest page it still holds.
    const since = state.loadedAt === null ? null : Math.min(state.loadedAt, state.value?.oldestReadAt ?? state.loadedAt);
    const tone = state.phase === "forbidden" ? "red" : state.error || state.stale ? "amber" : "neutral";
    const text = state.phase === "unavailable" ? "Profil indisponible" : state.phase === "not_granted" ? "Non accordé" : state.phase === "forbidden" ? "Lecture refusée" :
      state.phase === "loading" ? "Lecture en cours…" : state.phase === "idle" ? "Lecture requise" :
      state.phase === "error" ? state.error?.kind === "unreachable" ? "Contrôle injoignable" : state.error?.kind === "withdrawn" ? "Données retirées · accès à relire" : "Données indisponibles" :
      state.stale ? state.error?.kind === "unreachable" ? "Contrôle injoignable · dernière lecture conservée" : "Données périmées · dernière lecture conservée" :
      freshness({ loadedAt: since, now: Date.now(), cadence, stale: false }) === "aging" ? "Données à actualiser" : state.busy ? "Actualisation…" : "Lecture réussie";
    return el("span", "sv-source", el("strong", "", label), badge(text, state.phase === "ready" && !state.stale && !state.error && text === "Données à actualiser" ? "amber" : tone),
      el("span", "muted", state.loadedAt === null || since === null ? "Aucune lecture réussie" :
        state.loadedAt - since >= 1000 ? `Pages lues entre ${clockTime(since)} et ${clockTime(state.loadedAt)}` : `Lu le ${clockTime(state.loadedAt)}`));
  }
  #errorLine(error: SourceError | null, forbidden: boolean, key: string) {
    if (forbidden) return el("div", "control-error sv-error", el("p", "", "Consultation refusée par le contrôle. Les lignes, détails et positions de lecture ont été retirés. Relisez vos accès puis sélectionnez un contexte actuellement autorisé."),
      action("Relire mes accès", `${key}-access`, () => this.#host.onReviewAccess()));
    if (!error) return null;
    const node = el("div", "control-error sv-error", icon("alert"), el("p", "", error.message),
      error.kind === "budget" || this.#host.reads.readBudgetStopped ? null : reader("Réessayer", `${key}-retry`, () => { void this.#ask(() => this.#store.refresh(false)); }));
    node.setAttribute("role", "alert"); return node;
  }
  #renderStatus() {
    const state = this.#store.state;
    const summaries = { workers: summarize(state.workers), brokers: summarize(state.brokers), resources: summarize(state.resources) };
    const coverage = fleetCoverage({ ...summaries, contextsComplete: this.#host.contextsComplete() });
    const auto = state.autoPaused === "not_granted" ? state.access.workersUnavailable ?
      "Aucun registre disponible pour ce contexte : le profil workers est indisponible. Aucune lecture automatique ; l’activité reste inconnue." :
      "Aucun registre n’est accordé à ce contexte : rien n’est relu automatiquement. Les ressources sont relues par « Actualiser maintenant »." :
      state.cadence === 0 ? "Actualisation manuelle : aucune lecture sans action de votre part." :
      state.autoPaused === "hidden" ? "Actualisation automatique suspendue : onglet masqué." :
      state.autoPaused === "window" ? "Actualisation automatique suspendue : la fenêtre dépasse deux pages. Utilisez Actualiser pour relire toute la fenêtre." :
      state.autoPaused === "budget" ? "Actualisation automatique arrêtée : budget durable de lecture atteint." :
      state.autoPaused === "forbidden" ? "Actualisation automatique arrêtée : lecture refusée par le contrôle." :
      `Actualisation automatique de la flotte toutes les ${cadenceLabel(state.cadence)} lorsque cet onglet est visible ; les ressources sont relues sur action explicite.`;
    const fleet = [state.workers, state.brokers].filter((item) => !["not_granted", "unavailable"].includes(item.phase));
    const fleetError = fleet.find((item) => item.error)?.error ?? null;
    setText(this.#autoNote, auto);
    const reading = this.#reading();
    waiting(this.#refreshButton, reading, this.#host.reads.readBudgetStopped);
    replaceIfChanged(this.#sourcesBox, this.#freshnessBadge("Workers", state.workers), this.#freshnessBadge("Brokers", state.brokers), this.#freshnessBadge("Ressources", state.resources));
    // An error line is created when its message appears or changes, never again while it lasts: it is an alert.
    const focused = this.#errorsBox.contains(document.activeElement) ? (document.activeElement as HTMLElement).dataset.focusKey : undefined;
    const errorsChanged = replaceIfChanged(this.#errorsBox,
      this.#errorLine(fleetError, fleet.length > 0 && fleet.every((item) => item.phase === "forbidden"), "sv-fleet"),
      state.resources.phase === "not_granted" || state.resources.error && state.resources.error.message === fleetError?.message ? null :
        this.#errorLine(state.resources.error, state.resources.phase === "forbidden", "sv-resources"));
    if (errorsChanged && focused) [...this.#errorsBox.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === focused)?.focus({ preventScroll: true });
    if (errorsChanged && this.#errorsBox.childElementCount) this.#alerted = true;
    const failures = [...new Set([fleetError?.message, state.resources.error?.message].filter((message): message is string => !!message))];
    this.#failure = failures.length ? failures.join(" ") : null;
    // « Réessayer » keeps its line and the focus while a read is in flight. The mark is set on the displayed
    // node, outside the markup that decides whether the line is created again.
    for (const retry of this.#errorsBox.querySelectorAll<HTMLElement>('[data-focus-key$="-retry"]')) retry.setAttribute("aria-disabled", String(reading));
    replaceIfChanged(this.#coverageBox, badge("Couverture partielle", "amber"),
      el("ul", "sv-notes", ...coverage.notes.map((note) => { const item = el("li", "", note.text); item.dataset.tone = note.tone; return item; })));
    // Said by #announceRead once the lists are rendered, with the counts this read changed.
    this.#sentence = sourceAnnouncement({ workers: { ...summaries.workers, busy: state.workers.busy }, brokers: { ...summaries.brokers, busy: state.brokers.busy },
      resources: { ...summaries.resources, busy: state.resources.busy } });
  }

  #groups(): FleetGroup[] { return joinFleet(this.#store.state.brokers.value?.items ?? null, this.#store.state.workers.value?.items ?? []); }
  #brokerRow(group: FleetGroup, item: HumanBroker, windowNote: string, more: boolean) {
    const heartbeat = item.last_heartbeat; const loaded = !!this.#store.state.workers.value;
    const associated = group.workers.length ?
      el("div", "sv-links", el("p", "", `${group.workers.length} worker(s) associé(s) ${windowNote}`),
        el("p", "muted", "Association historique d’autorité · ne prouve pas l’exécution par ce broker."),
        action("Voir ces workers", `sv-broker-workers-${group.key}`, () => this.#filterBroker(group.key))) :
      el("p", "muted", loaded ? `Aucune association ${windowNote}${more ? " · autres pages inconnues" : ""}` : windowNote);
    return tableRow(cell("Broker", el("p", "mono sv-id", item.broker_id), el("p", "muted", `Incarnation ${item.incarnation_id}`)),
      cell("État du bail", badge(brokerStateLabel[item.state], item.state === "current" ? "neutral" : "amber"),
        item.state === "stale" ? el("p", "muted", "Broker déconnecté possible · dernier bail périmé, état runtime inconnu") : null,
        item.owned ? el("p", "muted", checkpointLabel(item)) : null,
        item.owned?.clock_uncertain && item.state !== "clock_uncertain" ? badge("Horloge incertaine du checkpoint", "amber") : null),
      cell("Autorités déclarées", el("ul", "sv-plain", ...item.authorities.map((authority) => el("li", "", el("span", "mono", brokerAuthorityLabel(authority)), ` · ${authority.runtime_kind}`)))),
      cell("Dernière réception", heartbeat ? el("p", "", formatBrokerDate(heartbeat.received_at, true)) : el("p", "muted", "Aucun heartbeat reçu"),
        heartbeat ? el("p", "muted", "Heartbeat déclaré · réception observée") : null,
        // The registry keeps this fact across a new grant: it is not the last contact of the current incarnation.
        heartbeatOfPreviousIncarnation(item) ? badge("Heartbeat d’une ancienne incarnation", "amber") : null),
      cell("Workers associés", associated),
      cell("Détail", action("Détails du broker", `sv-broker-${item.broker_id}`, () => this.#open({ kind: "broker", id: item.broker_id }))));
  }
  #renderBrokers() {
    const state = this.#store.state; const brokers = state.brokers; const groups = this.#groups();
    const windowNote = state.workers.phase === "unavailable" ? "Profil workers indisponible : associations inconnues" :
      state.workers.phase === "not_granted" ? "Registre des workers non accordé : associations inconnues" :
      state.workers.value ? "dans la fenêtre de workers chargée" : "fenêtre de workers non lue : associations inconnues";
    const more = !!state.workers.value?.hasMore;
    const listed = groups.filter((group) => group.broker);
    const matching = new Set(filterBrokers(listed.map((group) => group.broker!), this.#preferences.brokers.text).map((item) => item.broker_id));
    const shown = listed.filter((group) => matching.has(group.key));
    syncRows(this.#brokerTable.body, shown.map((group) => ({ key: group.key, signature: `${signatureOf(group.broker!)}\n${group.workers.length}\n${windowNote}\n${more}`,
      build: () => this.#brokerRow(group, group.broker!, windowNote, more) })));
    if (brokers.value) setText(this.#brokerTable.caption, `Brokers enregistrés · évaluation serveur du ${formatBrokerDate(brokers.value.evaluatedAt, true)}${
      brokers.value.profile ? ` · profil ${brokers.value.profile} · compte limité à la page · checkpoints déclarés partiels` : ""}${brokers.value.hasMore ? " · d’autres pages existent, voir le registre Brokers" : ""}`);
    this.#counts.brokers = brokers.value ? `${shownOfLoaded("brokers", shown.length, listed.length)}.` : "";
    setText(this.#brokerSummary, brokers.value ? `${windowSummary({ shown: shown.length, loaded: listed.length, hasMore: brokers.value.hasMore, noun: "brokers" })}${
      this.#preferences.brokers.text.trim() ? " · recherche appliquée aux seuls brokers chargés" : ""}` : "Aucun broker chargé : contenu du registre inconnu.");
    const empty = brokers.phase === "not_granted" ? el("p", "sv-note muted", "Registre des brokers non accordé à ce contexte : seules les associations portées par les lignes de workers sont affichées.") :
      !brokers.value ? emptyState(brokers.phase === "forbidden" ? "Brokers retirés" : brokers.phase === "error" ? "Brokers inconnus" : "Lecture des brokers…",
        brokers.phase === "forbidden" ? "La lecture a été refusée par le contrôle." : "Une lecture réussie est nécessaire avant de conclure. Une erreur n’est pas un registre vide.") :
      !brokers.value.items.length ? emptyState("Aucun broker enregistré à cette lecture", "Le registre a répondu sans ligne. L’inventaire runtime reste inconnu.") :
      !shown.length ? emptyState("Aucun broker chargé ne correspond", brokers.value.hasMore ? "D’autres pages existent au registre Brokers : un broker absent d’ici n’est pas absent du tenant." : "Aucun des brokers chargés ne correspond à la recherche.") : null;
    replaceIfChanged(this.#brokerEmpty, empty);
    const others = groups.filter((group) => !group.broker);
    const otherLabel = (group: FleetGroup) => group.kind === "unlisted_broker" ? `Broker ${group.key} référencé par des workers, absent de la page brokers lue` :
      group.kind === "unregistered" ? "Autorité de lancement non enregistrée dans le registre brokers" :
      group.kind === "authority_unknown" ? "Autorité de lancement inconnue" : "Registre brokers indisponible lors de la lecture du worker";
    replaceIfChanged(this.#brokerGroups, ...others.map((group) => el("li", "", el("div", "", el("strong", "", otherLabel(group)),
      el("p", "muted", `${group.workers.length} worker(s) dans la fenêtre chargée`)), action("Voir ces workers", `sv-group-${group.key}`, () => this.#filterBroker(group.key)))));
    place(this.#brokerBody, empty ? this.#brokerEmpty : this.#brokerTable.node, others.length ? this.#brokerGroups : null,
      brokers.value?.hasMore && this.#host.hasRegistry("brokers") ? this.#brokerFooter : null);
    this.#syncBrokerOptions(groups);
  }
  #syncBrokerOptions(groups: FleetGroup[]) {
    const node = this.#brokerSelect; if (!node) return;
    const wanted: [string, string][] = [["all", "Tous"], ...groups.map((group): [string, string] => [group.key,
      group.broker ? `${group.key.slice(0, 8)}… · ${brokerStateLabel[group.broker.state]}` : group.kind === "unlisted_broker" || group.kind === "broker" ? `${group.key.slice(0, 8)}… · hors page brokers` : associationLabel[group.kind]])];
    const selected = this.#preferences.fleet.broker;
    // A filter on a broker that left the window stays visible, so the empty result explains itself.
    if (!wanted.some(([key]) => key === selected)) wanted.push([selected, `${selected.slice(0, 8)}… · absent de la fenêtre`]);
    if (JSON.stringify(wanted) !== node.dataset.options) {
      node.replaceChildren(...wanted.map(([key, label]) => { const option = el("option", "", label); option.value = key; return option; }));
      node.dataset.options = JSON.stringify(wanted);
    }
    node.value = selected;
  }
  #association(link: HumanWorkerAssociation, runtime: string) {
    if (!link.broker) return el("p", "muted", `${associationLabel[link.status]} · ${runtime}`);
    return el("div", "", el("p", "mono sv-id", link.broker.broker_id),
      el("p", "muted", `${brokerStateLabel[link.broker.state]} · ${runtime}${link.runtime_matches === false ? " · runtime différent du broker" : ""}`));
  }
  #workerRow(item: HumanWorker, linked: boolean) {
    const flags = workerFlags(item); const observation = item.last_observation;
    return tableRow(cell("Worker", el("p", "mono sv-id", item.worker_id), el("p", "muted", `Session déclarée ${item.session_id}`),
        linked ? badge("Ressource autorisée correspondante", "teal") : null),
      cell("État du contrôle", el("p", "sv-state", badge(workerStateLabel[item.state], workerActivity(item.state) === "uncertain" ? "amber" : "neutral"), el("span", "mono muted", item.state)),
        el("p", "muted", `${activityLabel[workerActivity(item.state)]} · ${item.quota_held ? "quota réservé" : "quota non réservé"}`)),
      cell("Dernière observation", observation ? el("p", "", `Relevé : ${observation.status}`) : el("p", "muted", "Observation inconnue"),
        observation ? el("p", "muted", time(observation.observed_at, observation.observed_at_status)) : null),
      cell("Autorité du lancement", this.#association(item.launch_broker, item.runtime_kind)),
      cell("Points d’attention", flags.length ? el("div", "sv-flags", ...flags.map((flag) => badge(workerFlagLabel[flag], "amber"))) : el("p", "muted", "Aucun signalé par le registre")),
      cell("Détail", action("Détails", `sv-worker-${item.worker_id}`, () => this.#open({ kind: "worker", id: item.worker_id }))));
  }
  #renderWorkers() {
    const state = this.#store.state; const source = state.workers; const window = source.value;
    if (source.phase === "not_granted" || source.phase === "unavailable") return;
    const resources = state.resources.value?.items ?? [];
    const filtered = window ? sortWorkers(filterWorkers(window.items, this.#preferences.fleet), this.#preferences.order) : [];
    const shown = filtered.slice(0, this.#preferences.rendered);
    syncRows(this.#workerTable.body, shown.map((item) => {
      const linked = !!sessionResource(resources, item.session_id);
      return { key: item.worker_id, signature: `${signatureOf(item)}\n${linked}`, build: () => this.#workerRow(item, linked) };
    }));
    if (window) setText(this.#workerTable.caption, `Workers enregistrés · dernière évaluation serveur du ${formatDate(window.evaluatedAt, true)} · chaque page est un instantané distinct`);
    this.#counts.workers = window ? `${shownOfLoaded("workers", filtered.length, window.items.length)}.` : "";
    setText(this.#workerSummary, window ? `${windowSummary({ shown: filtered.length, loaded: window.items.length, hasMore: window.hasMore, noun: "workers" })}${
      isDefaultFleetFilter(this.#preferences.fleet) ? "" : " · recherche et filtres appliqués aux seules lignes chargées"}${shown.length < filtered.length ? ` · ${shown.length} lignes rendues` : ""}` :
      "Aucune ligne chargée : contenu du registre inconnu.");
    const empty = !window ? emptyState(source.phase === "forbidden" ? "Workers retirés" : source.phase === "error" ? "Workers inconnus" : "Lecture des workers…",
      source.phase === "forbidden" ? "La lecture a été refusée par le contrôle." : "La lecture doit réussir avant de conclure sur l’activité. Une erreur n’est pas une absence de worker.") :
      !window.items.length ? emptyState("Aucun worker enregistré à cette lecture", "Le registre a répondu sans ligne pour ce tenant. L’inventaire runtime reste inconnu.") :
      !filtered.length ? emptyState("Aucune ligne chargée ne correspond", window.hasMore ? "D’autres pages existent au contrôle : une ligne absente d’ici n’est pas absente du tenant. Élargissez les filtres ou chargez plus de lignes." : "Aucune des lignes chargées ne correspond à la recherche et aux filtres.") : null;
    replaceIfChanged(this.#workerEmpty, empty);
    let reveal = false;
    if (window) {
      setText(this.#renderMore, `Afficher ${Math.min(RENDER_STEP, filtered.length - shown.length)} lignes de plus`);
      reveal = settled(this.#loadMore, source.busy, this.#host.reads.readBudgetStopped);
      place(this.#workerFooter, shown.length < filtered.length ? this.#renderMore : null, window.hasMore && !window.capped ? this.#loadMore : null, window.capped ? this.#workerCap : null,
        (window.hasMore || window.capped) && this.#host.hasRegistry("workers") ? this.#openWorkers : null, !window.hasMore ? this.#workerEnd : null);
    }
    place(this.#workerBody, empty ? this.#workerEmpty : this.#workerTable.node, window ? this.#workerFooter : null);
    // The rows it loaded pushed the control down: it kept the focus, so it stays in sight for the next page.
    if (reveal && this.#loadMore.isConnected) this.#loadMore.scrollIntoView({ block: "nearest" });
  }
  #resourceRow(item: HumanResource, related: number, workersGranted: boolean) {
    return tableRow(cell("Type", badge(resourceKindLabel[item.resource.kind])),
      cell("Identifiant", el("p", "mono sv-id", item.resource.resource_id)),
      cell("Propriétaire", el("p", "muted", item.owner.kind === "human_local" ? `Compte local ${item.owner.account_id}` : `${item.owner.subject} · ${item.owner.issuer}`)),
      cell("Droits de ce contexte", el("p", "muted", [["métadonnées", item.permissions.metadata], ["contenu", item.permissions.content], ["export", item.permissions.export]]
        .map(([name, granted]) => `${name} ${granted ? "oui" : "non"}`).join(" · "))),
      cell("Corrélation", item.resource.kind === "session" ? el("p", "muted", !workersGranted ? "Registre des workers non accordé" :
        related ? `${related} worker(s) de la fenêtre chargée déclarent cette session` : "Aucun worker de la fenêtre chargée ne déclare cette session") :
        el("p", "muted", item.resource.kind === "agent" ? "Relation agent / session / sous-agent non fournie par le contrat nominatif · inconnue" : "Aucune relation fournie par le contrat nominatif")),
      cell("Détail", action("Détails", `sv-resource-${item.resource.kind}-${item.resource.resource_id}`, () => this.#open({ kind: "resource", resourceKind: item.resource.kind, id: item.resource.resource_id }))));
  }
  #renderResources() {
    const state = this.#store.state; const source = state.resources; const window = source.value;
    if (source.phase === "not_granted") return;
    const workers = state.workers.value?.items ?? []; const workersGranted = !["not_granted", "unavailable"].includes(state.workers.phase);
    const filtered = window ? filterResources(window.items, this.#preferences.resources) : [];
    syncRows(this.#resourceTable.body, filtered.map((item) => {
      const related = item.resource.kind === "session" ? workersOfSession(workers, item.resource.resource_id).length : 0;
      return { key: `${item.resource.kind}\n${item.resource.resource_id}`, signature: `${signatureOf(item)}\n${related}\n${workersGranted}`, build: () => this.#resourceRow(item, related, workersGranted) };
    }));
    setText(this.#resourceTable.caption, "Ressources explicitement autorisées · registre des droits du contrôle");
    this.#counts.resources = window ? `${shownOfLoaded("resources", filtered.length, window.items.length)}.` : "";
    setText(this.#resourceSummary, window ? windowSummary({ shown: filtered.length, loaded: window.items.length, hasMore: window.hasMore, noun: "resources" }) : "Aucune ressource chargée : contenu inconnu.");
    const empty = !window ? emptyState(source.phase === "forbidden" ? "Ressources retirées" : source.phase === "error" ? "Ressources inconnues" : "Lecture des ressources…",
      "La lecture doit réussir avant de conclure sur ce contexte.") :
      !window.items.length ? emptyState("Aucune ressource autorisée à cette lecture", "Aucune ressource explicitement autorisée pour ce contexte. Une ancienne trace seule n’accorde aucun accès.") :
      !filtered.length ? emptyState("Aucune ressource chargée ne correspond", "Modifiez la recherche ou le type.") : null;
    replaceIfChanged(this.#resourceEmpty, empty);
    let reveal = false;
    if (window) {
      reveal = settled(this.#loadResources, source.busy, this.#host.reads.readBudgetStopped);
      place(this.#resourceFooter, window.hasMore && !window.capped ? this.#loadResources : null, window.capped ? this.#resourceCap : null,
        this.#host.hasRegistry("resources") ? this.#openResources : null);
    }
    place(this.#resourceBody, empty ? this.#resourceEmpty : this.#resourceTable.node, window ? this.#resourceFooter : null);
    if (reveal && this.#loadResources.isConnected) this.#loadResources.scrollIntoView({ block: "nearest" });
  }

  #open(detail: Detail) {
    this.#closeDialog(); this.#store.clearSessionEvents(false);
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const heading = el("h2", "", detail.kind === "worker" ? "Worker" : detail.kind === "broker" ? "Broker" : resourceKindLabel[detail.resourceKind]); heading.id = "sv-detail-title";
    const dialog = el("dialog", "detail-dialog sv-dialog"); dialog.setAttribute("aria-labelledby", heading.id);
    const close = button("", "icon-button", () => dialog.close(), "x"); close.setAttribute("aria-label", "Fermer les détails");
    const body = el("div", "detail-body"); body.tabIndex = -1;
    const announce = el("p", "sr-only sv-announce"); announce.setAttribute("role", "status"); announce.setAttribute("aria-live", "polite");
    dialog.append(el("div", "detail-header", el("div", "", heading, el("p", "mono sv-id", detail.id)), close), body, announce);
    this.#dialog = dialog; this.#detail = detail; this.#dialogBody = body; this.#dialogAnnounce = announce; this.#dialogSaid = "";
    document.body.append(dialog); this.#renderDialog();
    if (this.#dialog !== dialog) return;
    dialog.showModal();
    dialog.addEventListener("close", () => {
      if (this.#dialog === dialog) { this.#dialog = null; this.#detail = null; this.#dialogBody = null; this.#dialogAnnounce = null; this.#store.clearSessionEvents(false); }
      dialog.remove();
      const requested = this.#afterDialog; this.#afterDialog = null;
      if (this.#closed || document.querySelector("dialog[open]")) return;
      if (requested?.isConnected) { requested.focus(); return; }
      const key = trigger?.dataset.focusKey;
      const replacement = key ? [...this.element.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === key) : null;
      (trigger?.isConnected ? trigger : replacement ?? this.#anchor()).focus({ preventScroll: true });
    }, { once: true });
  }
  #closeDialog() {
    const dialog = this.#dialog; if (!dialog) return;
    if (dialog.open) { dialog.close(); return; }
    // Never shown: there is no close event to wait for.
    dialog.remove(); this.#dialog = null; this.#detail = null; this.#dialogBody = null; this.#dialogAnnounce = null;
  }
  #events(sessionId: string) {
    const state = this.#store.state; const read = state.session?.sessionId === sessionId ? state.session : null;
    // The session is part of the markup: a kept section always acts on the session it names.
    const region = el("section", "sv-session"); region.dataset.session = sessionId;
    region.append(el("h3", "", "Événements de cette session"),
      el("p", "muted", "Lecture autorisée par le serveur pour cette session exacte : ses 25 événements les plus récents. Rien n’est lu avant votre demande ; chaque lecture laisse un reçu durable."),
      waiting(reader(read?.phase === "ready" ? "Relire les événements de cette session" : "Lire les événements de cette session", "sv-session-read",
        () => { void this.#store.readSessionEvents(sessionId); }), read?.phase === "loading", this.#host.reads.readBudgetStopped));
    if (!read) return region;
    // Not a live region: the detail announces the outcome in one sentence (see #renderDialog), never the list.
    const status = el("div", "sv-session-result"); status.tabIndex = -1;
    if (read.phase === "loading") status.append(el("p", "muted", "Lecture en cours…"));
    else if (read.phase !== "ready" || !read.page) status.append(el("p", read.phase === "error" ? "control-error" : "sv-refusal", read.message));
    else if (!read.page.items.length) status.append(el("p", "muted", "Aucun événement autorisé pour cette session à cette lecture."));
    else status.append(
      el("p", "muted", `${read.page.items.length} événement(s) reçus, ${read.order === "newest_first" ? "du plus récent au plus ancien" : "du plus ancien au plus récent (ce contrôle ne sert pas la lecture récente)"}${
        read.page.has_more ? read.order === "newest_first" ? " · d’autres, plus anciens, existent" : " · d’autres, plus récents, existent" : " · toute la sélection à cette lecture"} · reçu ${read.page.read_receipt.audit_event_id}`),
      ...(this.#host.openJournal && read.order === "newest_first" ? [action("Ouvrir cette session dans le Journal", "sv-session-journal", () => this.#host.openJournal?.({ kind: "session", resource_id: sessionId }))] : []),
      el("ol", "sv-events", ...read.page.items.map((item: HumanEvent) => el("li", "", el("p", "", el("strong", "", item.metadata.kind), ` · ${formatDate(item.metadata.timestamp, true)}`),
        el("p", "muted", `Source ${item.metadata.source} · provenance ${item.metadata.provenance} · couverture ${item.metadata.coverage} · ${item.metadata.decision ? `décision ${item.metadata.decision}` : "décision non fournie"}`)))));
    region.append(status);
    return region;
  }
  #workerDetail(item: HumanWorker): (HTMLElement | null)[] {
    const state = this.#store.state; const observation = item.last_observation; const flags = workerFlags(item);
    const resources = state.resources.value; const linked = resources ? sessionResource(resources.items, item.session_id) : null;
    const link = (title: string, value: HumanWorkerAssociation) => el("div", "sv-association", el("h3", "", title), el("p", "", associationLabel[value.status]),
      value.broker ? el("p", "mono sv-id", value.broker.broker_id) : null,
      value.broker ? el("p", "muted", `${brokerStateLabel[value.broker.state]} · incarnation courante ${value.broker.current_incarnation_id} · runtime ${value.runtime_kind} · correspondance ${value.runtime_matches ? "oui" : "non"} · dans le grant courant ${value.in_current_grant ? "oui" : "non"}`) : null,
      value.broker && state.brokers.value?.items.some((broker) => broker.broker_id === value.broker?.broker_id) ?
        action("Ouvrir ce broker", `sv-detail-broker-${title}`, () => this.#open({ kind: "broker", id: value.broker!.broker_id })) : null);
    return [
      definition([["État du contrôle", `${workerStateLabel[item.state]} · ${item.state}`], ["Groupe", `${activityLabel[workerActivity(item.state)]} · état du contrôle, sans preuve d’exécution`],
        ["Quota", item.quota_held ? "Réservé" : "Non réservé"], ["Allocation", `${item.cpu_millis} milliCPU · ${item.memory_bytes} octets · consommation réelle inconnue, non facturable`],
        ["Runtime déclaré", `${item.runtime_kind} · réseau ${item.network}`], ["Révision", item.revision], ["Demande", item.request_id], ["Tentative", item.attempt_id],
        ["Début historique", time(item.observed_started_at, item.observed_started_at_status)],
        ["Points d’attention", flags.length ? flags.map((flag) => workerFlagLabel[flag]).join(" · ") : "Aucun signalé par le registre"]]),
      el("h3", "", "Dernière observation runtime"),
      observation ? definition([["Statut relevé", observation.status], ["Relevé", time(observation.observed_at, observation.observed_at_status)],
        ["Début fourni", time(observation.started_at, observation.started_at_status)], ["Fin fournie", time(observation.finished_at, observation.finished_at_status)],
        ["Code de sortie", observation.exit_code ?? "Non fourni"], ["Arrêt mémoire", observation.oom_killed === null ? "Inconnu" : observation.oom_killed ? "Oui" : "Non"],
        ["Redémarrages", observation.restart_count === null ? "Inconnu" : String(observation.restart_count)], ["Provenance", "observed"]]) :
        el("p", "muted", "Observation inconnue : aucune inspection runtime n’est enregistrée pour ce worker."),
      link("Autorité du lancement", item.launch_broker), link("Autorité de l’inspection", item.observation_broker),
      el("p", "muted", "Une association d’autorité est historique et peut être postérieure au fait worker : elle ne prouve pas le broker qui a exécuté ou inspecté cette session."),
      el("h3", "", "Session déclarée"), el("p", "mono sv-id", item.session_id),
      el("p", "muted", state.resources.phase === "not_granted" ? "Les ressources ne sont pas lues dans ce contexte : aucune correspondance n’est recherchée." :
        !resources ? "Ressources non chargées : correspondance inconnue." :
        linked ? `Ressource autorisée de même identifiant exact : métadonnées ${linked.permissions.metadata ? "oui" : "non"} · contenu ${linked.permissions.content ? "oui" : "non"} · export ${linked.permissions.export ? "oui" : "non"}.` :
          `Aucune ressource de session de même identifiant parmi les ${resources.items.length} ressources chargées${resources.hasMore ? " ; d’autres pages existent" : ""}. Une référence déclarée n’attribue aucun propriétaire.`),
      this.#events(item.session_id),
      state.workers.value ? el("p", "muted", `Ligne lue le ${formatDate(state.workers.value.evaluatedAt, true)} · ${state.workers.value.receipts.length} reçu(s) durable(s) pour cette fenêtre. Un reçu atteste l’enregistrement de la lecture, pas une exécution.`) : null,
    ];
  }
  #brokerDetail(item: HumanBroker, group: FleetGroup | undefined): (HTMLElement | null)[] {
    const window = this.#store.state.brokers.value; const heartbeat = item.last_heartbeat; const count = group?.workers.length ?? 0;
    return [
      definition([["État du bail", brokerStateLabel[item.state]], ["Incarnation courante", item.incarnation_id], ["Révision de l’autorisation", item.grant_revision],
        ["Révision du heartbeat", item.heartbeat_revision], ["Autorisation accordée", formatBrokerDate(item.granted_at, true)], ["Expiration de l’autorisation", formatBrokerDate(item.not_after, true)],
        ["Révocation", item.revoked_at ? formatBrokerDate(item.revoked_at, true) : "Aucune révocation enregistrée"],
        ["Autorités déclarées", item.authorities.map((authority) => `${brokerAuthorityLabel(authority)} · ${authority.runtime_kind}`).join(" ; ")],
        ["Heartbeat déclaré", heartbeat ? `Séquence ${heartbeat.sequence} · incarnation ${heartbeat.incarnation_id}${heartbeatOfPreviousIncarnation(item) ? " · ancienne incarnation" : ""}` : "Aucun heartbeat reçu"],
        ["Réception observée", heartbeat ? formatBrokerDate(heartbeat.received_at, true) : "Inconnue"], ["Expiration du bail déclaré", heartbeat ? formatBrokerDate(heartbeat.lease_expires_at, true) : "Inconnue"]]),
      ...(item.owned ? [definition([["Profil", "human_broker_registry_owned_v1"], ["Durée du bail autorisée", `${item.owned.lease_seconds} s`],
        ["Horloge", item.owned.clock_uncertain ? "Incertaine pour le heartbeat ou le checkpoint" : "Aucune incertitude signalée par le contrôle"],
        ...checkpointFacts(item).map(([label, value]): [string, string] => [label, label === "Réception du checkpoint" || label === "Expiration du checkpoint" ? formatBrokerDate(value, true) : value])])] : []),
      el("p", "muted", this.#store.state.workers.phase === "unavailable" ? "Profil workers indisponible : associations et activité inconnues. Les références du checkpoint ne sont pas un inventaire de workers." :
        this.#store.state.workers.phase === "not_granted" ? "Registre des workers non accordé : associations inconnues." :
        `${count} worker(s) de la fenêtre chargée portent une association d’autorité vers ce broker. Elle est historique et ne prouve pas l’exécution par ce broker ; l’inventaire runtime reste inconnu.`),
      count ? action("Voir ces workers", "sv-detail-broker-workers", () => this.#filterBroker(item.broker_id)) : null,
      // The receipt changes with every read: on its own line, it is the only part of this detail a refresh replaces.
      window ? el("p", "muted", `Reçu durable de cette lecture : ${window.receipt.audit_event_id} · ${formatBrokerDate(window.receipt.received_at, true)}. Un reçu atteste l’enregistrement de la lecture, pas une exécution.`) : null,
    ];
  }
  #resourceDetail(item: HumanResource): (HTMLElement | null)[] {
    const state = this.#store.state; const session = item.resource.kind === "session";
    const related = session ? workersOfSession(state.workers.value?.items ?? [], item.resource.resource_id) : [];
    return [
      definition([["Type", resourceKindLabel[item.resource.kind]], ["Propriétaire", item.owner.kind === "human_local" ? `Compte local ${item.owner.account_id}` : `${item.owner.subject} · ${item.owner.issuer}`],
        ["Révision", item.revision], ["Métadonnées", item.permissions.metadata ? "Autorisées" : "Non autorisées"], ["Contenu", item.permissions.content ? "Autorisé" : "Non autorisé"],
        ["Export", item.permissions.export ? "Autorisé selon la projection" : "Non autorisé"]]),
      session ? el("h3", "", "Workers déclarant cette session") : null,
      !session ? el("p", "muted", item.resource.kind === "agent" ? "Relation agent ↔ session et agent ↔ sous-agent : non fournie par le contrat nominatif, donc inconnue. Elle n’est jamais déduite d’un nom ou d’un modèle." :
        "Aucune relation vers un worker ou une session n’est fournie par le contrat nominatif pour ce type de ressource.") :
        state.workers.phase === "unavailable" ? el("p", "muted", "Profil workers indisponible : aucune association ne peut être recherchée.") :
        state.workers.phase === "not_granted" ? el("p", "muted", "Registre des workers non accordé à ce contexte.") :
        related.length ? el("ul", "sv-plain", ...related.map((worker) => el("li", "", action(`Ouvrir le worker ${worker.worker_id}`, `sv-detail-worker-${worker.worker_id}`, () => this.#open({ kind: "worker", id: worker.worker_id })),
          el("span", "muted", ` ${workerStateLabel[worker.state]} · référence de session déclarée`)))) :
          el("p", "muted", `Aucun worker de la fenêtre chargée ne déclare cette session${state.workers.value?.hasMore ? " ; d’autres pages existent au contrôle" : ""}.`),
      session ? this.#events(item.resource.resource_id) : null,
    ];
  }
  #renderDialog() {
    const detail = this.#detail; const body = this.#dialogBody; if (!detail || !body || !this.#dialog) return;
    const state = this.#store.state;
    const active = body.contains(document.activeElement) ? document.activeElement as HTMLElement : null; const focusKey = active?.dataset.focusKey;
    let content: (HTMLElement | null)[] | null = null;
    if (detail.kind === "worker") { const item = state.workers.value?.items.find((row) => row.worker_id === detail.id); content = item ? this.#workerDetail(item) : null; }
    else if (detail.kind === "broker") { const item = state.brokers.value?.items.find((row) => row.broker_id === detail.id); content = item ? this.#brokerDetail(item, this.#groups().find((group) => group.key === detail.id)) : null; }
    else { const item = state.resources.value?.items.find((row) => row.resource.kind === detail.resourceKind && row.resource.resource_id === detail.id); content = item ? this.#resourceDetail(item) : null; }
    // A row that left the authorized window takes its detail with it.
    if (!content) { this.#closeDialog(); return; }
    // Only the sections whose content changed are replaced: a publication takes neither a text selection nor the
    // focus away from the others, and does not insert again what was already read.
    const changed = syncSections(body, ...content);
    // A read asked from this detail is announced when it ends; the same outcome is said again after a new read.
    const sentence = sessionAnnouncement(state.session);
    if (sentence === null) this.#dialogSaid = "";
    else if (sentence !== this.#dialogSaid && this.#dialogAnnounce) { this.#dialogSaid = sentence; announce(this.#dialogAnnounce, sentence); }
    if (!changed || !active || active.isConnected) return;
    // Focus never leaves the modal detail: the same control when it is still usable, else the result it produced.
    const same = focusKey ? [...body.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === focusKey && !node.matches(":disabled")) : undefined;
    (same ?? body.querySelector<HTMLElement>(".sv-session-result") ?? body).focus({ preventScroll: true });
  }
}
