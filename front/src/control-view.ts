import { ApiClient, ApiError } from "./api.ts";
import { button, el, icon } from "./dom.ts";
import { asText, formatBytes, formatDate, formatMetric } from "./model.ts";
import {
  allocationDuration, controlPageUrl, formatExactDecimal, parseAllocationPage,
  parseQuota, parseWorker, parseWorkerPage, relatedWorkers, workerRelation, workerUrl,
} from "./control.ts";
import type { AllocationRecord, ControlPage, ControlView, QuotaRecord, WorkerRecord } from "./control.ts";
import type { Agent, Sandbox } from "./types.ts";

interface Resource<T> {
  value: T | null;
  loading: boolean;
  error: string;
  loadedAt: string | null;
  stale: boolean;
}
interface Collection<T> extends Resource<ControlPage<T>> {
  cursor: string | null;
  history: (string | null)[];
}
const resource = <T>(): Resource<T> => ({ value: null, loading: false, error: "", loadedAt: null, stale: false });
const collection = <T>(): Collection<T> => ({ ...resource<ControlPage<T>>(), cursor: null, history: [] });
function badge(text: string, tone = "neutral") { return el("span", `badge badge-${tone}`, text); }
function value(label: string, content: unknown) {
  return el("div", "key-value", el("dt", "", label), el("dd", "", asText(content)));
}
function memory(bytes: number) {
  return `${formatBytes(bytes)} (${formatMetric(bytes)} octets)`;
}
function action(label: string, key: string, callback: () => void, disabled = false) {
  const node = button(label, "button button-quiet", callback);
  node.dataset.focusKey = key;
  node.disabled = disabled;
  return node;
}
function state(worker: WorkerRecord) {
  const labels: Record<WorkerRecord["state"], [string, string]> = {
    reserved: ["Réservé", "blue"], preparing: ["Préparation", "blue"],
    created: ["Créé", "blue"], start_issued: ["Démarrage émis", "amber"],
    running: ["En cours", "teal"], stopping: ["Arrêt demandé", "amber"],
    terminal: ["Terminal observé", "neutral"], in_doubt: ["État incertain", "amber"],
    cancelled: ["Réservation annulée", "neutral"],
  };
  const [label, tone] = labels[worker.state];
  return badge(label, tone);
}
function failure(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 404) return "Cette lecture n’est pas disponible sur le collecteur connecté. Vérifiez sa version.";
    return error.message;
  }
  if (error instanceof Error && error.message.startsWith("Contrat")) return error.message;
  return "Lecture impossible. Vérifiez la connexion au collecteur puis réessayez.";
}
function table(caption: string, columns: string[], rows: HTMLElement[]) {
  return el("div", "table-scroll", el("table", "entity-table control-table",
    el("caption", "sr-only", caption),
    el("thead", "", el("tr", "", ...columns.map((label) => {
      const th = el("th", "", label);
      th.scope = "col";
      return th;
    }))), el("tbody", "", ...rows)));
}

/** Read-only control data, scoped exclusively by the authenticated ApiClient. */
export class ControlDashboard {
  #client: ApiClient;
  #onAuthError: () => void;
  #openDialog: (title: string, eyebrow: string) => { dialog: HTMLDialogElement; body: HTMLElement };
  #workers = collection<WorkerRecord>();
  #usage = collection<AllocationRecord>();
  #quota = resource<QuotaRecord>();
  #view: ControlView | null = null;
  #slot: HTMLElement | null = null;
  #closed = false;
  #connected = false;
  #generation = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #workerLinks: { slot: HTMLElement; entity: Pick<Agent, "session_id" | "sandbox_id" | "field_provenance"> } | null = null;
  #workerEntities: { slot: HTMLElement; worker: WorkerRecord } | null = null;
  #relatedEntities: (worker: WorkerRecord) => HTMLElement;

  constructor(client: ApiClient, onAuthError: () => void,
    openDialog: (title: string, eyebrow: string) => { dialog: HTMLDialogElement; body: HTMLElement },
    relatedEntities: (worker: WorkerRecord) => HTMLElement) {
    this.#client = client;
    this.#onAuthError = onAuthError;
    this.#openDialog = openDialog;
    this.#relatedEntities = relatedEntities;
  }
  close() {
    this.#closed = true;
    clearTimeout(this.#timer);
    this.#slot = null;
    this.#workers = collection();
    this.#usage = collection();
    this.#quota = resource();
    this.#workerLinks = null;
    this.#workerEntities = null;
  }
  unmount() { this.#slot = null; this.#view = null; }
  mount(slot: HTMLElement, view: ControlView) {
    this.#slot = slot;
    this.#view = view;
    this.#render();
    const current = this.#current();
    if (!current.value || current.stale || current.error ||
      Date.now() - Date.parse(current.loadedAt ?? "") > 30000) void this.refresh();
  }
  stream(connected: boolean) {
    if (this.#connected === connected) return;
    const reconnected = !this.#connected && connected;
    this.#connected = connected;
    this.#render();
    if (reconnected) this.invalidate();
  }
  invalidate() {
    const alreadyStale = this.#current().stale && (!this.#hasWorkerLinks() || this.#workers.stale);
    this.#generation++;
    for (const item of [this.#workers, this.#usage, this.#quota]) item.stale = true;
    if (!alreadyStale) this.#render();
    this.#schedule();
  }
  relationshipsChanged() { this.#render(); }
  #hasWorkerLinks() {
    return !!this.#workerLinks?.slot.isConnected && !!this.#workerLinks.slot.closest("dialog")?.open;
  }
  #schedule() {
    if (this.#timer !== undefined || this.#closed || (!this.#view && !this.#hasWorkerLinks())) return;
    // A sustained event stream cannot postpone a refresh or issue one request per event.
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      if (this.#closed || (!this.#view && !this.#hasWorkerLinks())) return;
      void this.refresh();
    }, 1000);
  }
  #current(): Resource<unknown> {
    return this.#view === "workers" || (!this.#view && this.#hasWorkerLinks()) ? this.#workers : this.#view === "usage" ? this.#usage : this.#quota;
  }
  async refresh() {
    const reads: Promise<void>[] = [];
    // The modal registry has its own page and loading guard, independent of the view behind it.
    if (this.#view === "workers" || this.#hasWorkerLinks())
      reads.push(this.#load(this.#workers, this.#workers.cursor, this.#workers.history, "workers", parseWorkerPage));
    if (this.#view === "usage") reads.push(this.#load(this.#usage, this.#usage.cursor, this.#usage.history, "usage", parseAllocationPage));
    if (this.#view === "quota") reads.push(this.#read(this.#quota, "/api/v1/quota", parseQuota));
    await Promise.all(reads);
  }
  async #load<T>(target: Collection<T>, cursor: string | null, history: (string | null)[],
    kind: "workers" | "usage", parse: (input: unknown) => ControlPage<T>) {
    await this.#read(target, controlPageUrl(kind, cursor), parse, () => {
      target.cursor = cursor;
      target.history = history;
    });
  }
  async #read<T>(target: Resource<T>, path: string, parse: (input: unknown) => T, accepted?: () => void) {
    if (this.#closed || target.loading) return;
    const generation = this.#generation;
    target.loading = true;
    target.error = "";
    this.#render();
    try {
      const received = parse(await this.#client.get<unknown>(path));
      if (this.#closed) return;
      target.value = received;
      target.loadedAt = new Date().toISOString();
      target.stale = generation !== this.#generation;
      accepted?.();
    } catch (error) {
      if (this.#closed) return;
      if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
        this.#onAuthError();
        return;
      }
      target.error = failure(error);
      target.stale = true;
    } finally {
      target.loading = false;
      if (!this.#closed) {
        this.#render();
        if (target.stale && !target.error) this.#schedule();
      }
    }
  }
  #status<T>(item: Resource<T>): HTMLElement {
    const text = item.loading ? (item.value ? "Actualisation… · données de la dernière lecture" : "Chargement…") :
      item.error ? (item.value ? "Données périmées · dernière lecture conservée" : "Données indisponibles") :
      item.stale ? "Données à actualiser" : "Lecture réussie";
    const node = el("div", "control-status",
      badge(text, item.error || item.stale ? "amber" : "neutral"),
      el("span", "muted", item.loadedAt ? `Lu le ${formatDate(item.loadedAt, true)}` : "Aucune lecture réussie"),
      !this.#connected ? badge("Flux déconnecté · lectures ponctuelles seulement", "amber") : null);
    node.setAttribute("role", "status");
    node.setAttribute("aria-live", "polite");
    return node;
  }
  #error<T>(item: Resource<T>): HTMLElement | null {
    if (!item.error) return null;
    const node = el("div", "control-error", icon("alert"), el("p", "", item.error),
      action("Réessayer", `retry-${this.#view}`, () => { void this.refresh(); }, item.loading));
    node.setAttribute("role", "alert");
    return node;
  }
  #empty(title: string, description: string) {
    return el("div", "empty-state", el("span", "empty-icon", icon("box")),
      el("h3", "", title), el("p", "", description));
  }
  #pagination<T>(target: Collection<T>, kind: "workers" | "usage", parse: (input: unknown) => ControlPage<T>) {
    const summary = el("span", "control-page-summary", target.value ?
      `${target.value.items.length} résultat${target.value.items.length === 1 ? "" : "s"} sur cette page · page ${target.history.length + 1}${target.history.length || target.value.next_cursor ? " · lecture partielle" : ""}` : "Périmètre : client du jeton de lecture");
    summary.tabIndex = -1;
    summary.dataset.focusKey = `page-summary-${kind}`;
    return el("div", "table-footer control-pagination", summary,
      el("div", "control-page-actions",
        target.history.length ? action("Première page", `first-${kind}`, () => {
          void this.#load(target, null, [], kind, parse);
        }, target.loading) : null,
        target.history.length ? action("Page précédente", `previous-${kind}`, () => {
          void this.#load(target, target.history.at(-1) ?? null, target.history.slice(0, -1), kind, parse);
        }, target.loading) : null,
        target.value?.next_cursor ? action("Page suivante", `next-${kind}`, () => {
          void this.#load(target, target.value?.next_cursor ?? null, [...target.history, target.cursor], kind, parse);
        }, target.loading) : target.value ? el("span", "muted", "Fin des résultats à cette lecture") : null));
  }
  #render() {
    if (this.#closed) return;
    if (this.#hasWorkerLinks()) this.#renderWorkerLinks();
    if (this.#workerEntities?.slot.isConnected && this.#workerEntities.slot.closest("dialog")?.open) {
      const { slot, worker } = this.#workerEntities;
      const focused = slot.contains(document.activeElement) ? document.activeElement as HTMLElement : null;
      const key = focused?.dataset.focusKey;
      slot.replaceChildren(this.#relatedEntities(worker));
      if (focused) {
        const replacement = key ? [...slot.querySelectorAll<HTMLElement>("[data-focus-key]")]
          .find((node) => node.dataset.focusKey === key && node.isConnected && !node.matches(":disabled") &&
            !node.closest("[hidden], [inert]") && node.getClientRects().length > 0) : null;
        const fallback = slot.closest("dialog")?.querySelector<HTMLElement>(".detail-header button:not(:disabled)");
        (replacement ?? fallback)?.focus({ preventScroll: true });
      }
    }
    if (!this.#slot || !this.#view || this.#closed) return;
    const slot = this.#slot;
    const focused = slot.contains(document.activeElement) ?
      (document.activeElement as HTMLElement).dataset.focusKey : undefined;
    const current = this.#current();
    const content = this.#view === "workers" ? this.#renderWorkers() :
      this.#view === "quota" ? this.#renderQuota() : this.#renderUsage();
    slot.replaceChildren(this.#status(current), this.#error(current) ?? "", content);
    slot.setAttribute("aria-busy", String(current.loading));
    if (focused) {
      const restored = [...slot.querySelectorAll<HTMLElement>("[data-focus-key]")]
        .find((node) => node.dataset.focusKey === focused && !(node as HTMLButtonElement).disabled);
      (restored ?? slot.querySelector<HTMLElement>(".control-page-summary"))?.focus({ preventScroll: true });
    }
  }
  #renderWorkers() {
    const page = this.#workers.value;
    return el("section", "panel control-panel",
      el("div", "panel-heading", el("div", "", el("h2", "", "Registre des workers"),
        el("p", "", "État durable du contrôle. Le dernier état runtime reste une observation datée, distincte d’une disponibilité actuelle."),
        el("p", "control-scroll-hint", "Faites défiler le tableau horizontalement pour consulter toutes les colonnes."))),
      !page ? this.#empty(this.#workers.loading ? "Chargement des workers…" : "Registre indisponible",
        "La liste reste inconnue tant que cette lecture n’a pas abouti.") :
      !page.items.length ? this.#empty("Aucun worker sur cette page", "Aucun enregistrement dans ce périmètre de lecture. Les sessions s’autorisent depuis le CLI de confiance.") :
      table("Workers du client authentifié", ["Worker / session", "État du contrôle", "Dernière observation runtime", "Réservation", ""],
        page.items.map((worker) => el("tr", "",
          el("td", "", el("strong", "mono control-id", worker.worker_id),
            el("span", "muted mono control-id", worker.launch.session_id),
            el("span", "muted", `${worker.launch.runtime_kind} · ${worker.launch.provider} · ${worker.launch.profile}`)),
          el("td", "", state(worker), worker.unknown_reason ? el("p", "control-warning", worker.unknown_reason) : null,
            worker.allocation_inconsistent ? el("p", "control-warning", "Allocation incohérente · durée inconnue") : null),
          el("td", "", worker.last_observation ? el("div", "stacked",
            badge(asText(worker.last_observation.status)),
            el("span", "muted", formatDate(worker.last_observation.observed_at, true))) : el("span", "muted", "Inconnue · aucune observation")),
          el("td", "", badge(worker.quota_held ? "Quota retenu" : "Quota libéré", worker.quota_held ? "blue" : "neutral"),
            el("p", "muted", `${formatMetric(worker.launch.cpu_millis)} milliCPU · ${formatBytes(worker.launch.memory_bytes)}`)),
          el("td", "", action("Détails", `worker-${worker.worker_id}`, () => { void this.showWorker(worker.worker_id); }))))),
      this.#pagination(this.#workers, "workers", parseWorkerPage));
  }
  #renderQuota() {
    const quota = this.#quota.value;
    const row = (label: string, held: string, limit?: string) => el("div", "quota-row",
      el("dt", "", label), el("dd", "", el("strong", "", held), limit ? el("span", "muted", ` / ${limit}`) : null));
    return el("section", "panel control-panel",
      el("div", "panel-heading", el("div", "", el("h2", "", "Ressources réservées"),
        el("p", "", "Les réservations proviennent des transactions durables du client authentifié. Elles peuvent rester retenues lorsque l’état d’un worker est incertain."))),
      !quota ? this.#empty(this.#quota.loading ? "Chargement des quotas…" : "Quotas inconnus",
        "Aucune capacité ni réservation ne peut être déduite d’une lecture absente.") :
      el("dl", "quota-list",
        row("Workers actifs réservés", formatMetric(quota.held_workers), formatMetric(quota.policy.max_active_workers)),
        row("Capacité CPU réservée", `${formatMetric(quota.held_cpu_millis)} milliCPU`, `${formatMetric(quota.policy.max_cpu_millis)} milliCPU`),
        row("Mémoire réservée", memory(quota.held_memory_bytes), memory(quota.policy.max_memory_bytes)),
        row("Durée maximale par worker", `${formatMetric(quota.policy.max_timeout_seconds)} s`)),
      el("div", "control-explanation", icon("info"), el("p", "", "Les valeurs représentent des ressources allouées, sans mesure de CPU réellement consommé. Les limites sont appliquées par le contrôle ; cette vue permet leur consultation.")));
  }
  #renderUsage() {
    const page = this.#usage.value;
    return el("section", "panel control-panel",
      el("div", "panel-heading", el("div", "", el("h2", "", "Temps d’allocation observé"),
        el("p", "", "Méthode 1 · non facturable (billable=false). CPU et mémoire alloués × durée observée, sans prix ni mesure de CPU consommé. Une observation terminale absente ne vaut jamais zéro."),
        el("p", "control-scroll-hint", "Faites défiler le tableau horizontalement pour consulter toutes les colonnes."))),
      !page ? this.#empty(this.#usage.loading ? "Chargement des allocations…" : "Allocations inconnues",
        "La lecture doit aboutir avant de présenter un relevé.") :
      !page.items.length ? this.#empty("Aucune allocation terminale sur cette page",
        "Aucune durée attestée dans ce périmètre. Un worker actif ou sans observation terminale conserve une durée inconnue.") :
      table("Allocations terminales non facturables", ["Worker / tentative", "Période runtime observée", "Durée", "CPU alloué × temps", "Mémoire allouée × temps", ""],
        page.items.map((item) => el("tr", "",
          el("td", "", el("span", "mono control-id", item.worker_id), el("span", "muted mono control-id", item.attempt_id)),
          el("td", "", el("span", "control-id", formatDate(item.started_at, true)), el("span", "muted control-id", formatDate(item.finished_at, true)),
            badge("Observé", "teal"), el("span", "muted control-id", `${item.source.runtime_kind} · ${item.source.authority_id}`)),
          el("td", "mono", allocationDuration(item)),
          el("td", "mono control-unit", `${formatExactDecimal(item.cpu_millicore_nanoseconds, 9)} milliCPU·s`),
          el("td", "mono control-unit", `${formatExactDecimal(item.memory_byte_nanoseconds)} octet·ns`),
          el("td", "", action("Worker", `allocation-${item.attempt_id}`, () => { void this.showWorker(item.worker_id); }))))),
      this.#pagination(this.#usage, "usage", parseAllocationPage));
  }
  agentWorkers(agent: Agent): HTMLElement {
    return this.#entityWorkers(agent);
  }
  sandboxWorkers(sandbox: Sandbox): HTMLElement {
    return this.#entityWorkers({ session_id: sandbox.session_id, sandbox_id: sandbox.id });
  }
  #entityWorkers(entity: Pick<Agent, "session_id" | "sandbox_id" | "field_provenance">): HTMLElement {
    const slot = el("section", "agent-worker-links");
    slot.setAttribute("aria-label", "Workers liés explicitement");
    this.#workerLinks = { slot, entity };
    this.#renderWorkerLinks();
    // The caller inserts the section in its modal before a request can re-render it.
    queueMicrotask(() => {
      if (this.#closed || !this.#hasWorkerLinks()) return;
      if (!this.#workers.value || this.#workers.stale || this.#workers.error ||
        Date.now() - Date.parse(this.#workers.loadedAt ?? "") > 30000)
        void this.#load(this.#workers, this.#workers.cursor, this.#workers.history, "workers", parseWorkerPage);
    });
    return slot;
  }
  #renderWorkerLinks() {
    const current = this.#workerLinks;
    if (!current) return;
    const { slot, entity } = current;
    const focused = slot.contains(document.activeElement) ? (document.activeElement as HTMLElement).dataset.focusKey : undefined;
    const page = this.#workers.value;
    const workers = relatedWorkers(entity, page?.items ?? []);
    const error = this.#workers.error ? el("div", "control-error", el("p", "", this.#workers.error),
      action("Réessayer le registre", "retry-related-workers", () => {
        void this.#load(this.#workers, this.#workers.cursor, this.#workers.history, "workers", parseWorkerPage);
      }, this.#workers.loading)) : null;
    error?.setAttribute("role", "alert");
    slot.replaceChildren(el("h3", "", "Workers liés explicitement"), this.#status(this.#workers),
      el("p", "muted", "Source : registre durable /api/v1/workers du client authentifié. Recherche limitée à la page chargée ; aucun total du client n’est déduit. Une session identique ne prouve pas l’exécution d’un agent."),
      error ?? "",
      !page ? el("p", "muted", this.#workers.loading ? "Chargement des liens du registre…" : "Les liens restent inconnus tant que le registre n’a pas été lu.") :
        workers.length ? el("ul", "resource-link-list", ...workers.map((worker) => el("li", "",
          action(worker.worker_id, `agent-worker-${worker.worker_id}`, () => { void this.showWorker(worker.worker_id); }),
          ...workerRelation(entity, worker).map((reason) => {
            const provenance = entity.field_provenance?.[reason === "Session identique" ? "session_id" : "sandbox_id"];
            return el("span", "resource-link-evidence", el("span", "muted", reason),
              badge(provenance === "declared" ? "Identifiant déclaré" : provenance === "observed" ? "Identifiant observé" :
                provenance === "inferred" ? "Identifiant inféré" : "Provenance de l’identifiant non fournie"));
          }), state(worker)))) :
          el("p", "muted", "Aucune correspondance sur cette page du registre. D’autres pages ou des observations absentes peuvent contenir des liens."),
      this.#pagination(this.#workers, "workers", parseWorkerPage));
    slot.setAttribute("aria-busy", String(this.#workers.loading));
    if (focused) {
      const replacement = [...slot.querySelectorAll<HTMLElement>("[data-focus-key]")]
        .find((node) => node.dataset.focusKey === focused && !(node as HTMLButtonElement).disabled);
      (replacement ?? slot.querySelector<HTMLElement>(".control-page-summary"))?.focus({ preventScroll: true });
    }
  }
  async showWorker(id: string) {
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const triggerKey = trigger?.dataset.focusKey;
    const { dialog, body } = this.#openDialog(id, "WORKER · LECTURE DU REGISTRE");
    dialog.addEventListener("close", () => {
      if (this.#closed || document.querySelector("dialog[open]")) return;
      const replacement = triggerKey ? [...this.#slot?.querySelectorAll<HTMLElement>("[data-focus-key]") ?? []]
        .find((node) => node.dataset.focusKey === triggerKey && !(node as HTMLButtonElement).disabled) : null;
      const target = trigger?.isConnected ? trigger : replacement;
      const fallback = this.#slot?.querySelector<HTMLElement>(".control-page-summary") ??
        document.querySelector<HTMLElement>("#main-content h1");
      (target ?? fallback)?.focus({ preventScroll: true });
    }, { once: true });
    const load = async () => {
      body.replaceChildren(this.#empty("Lecture du worker…", "Récupération de l’état durable pour cet identifiant."));
      body.setAttribute("aria-busy", "true");
      try {
        const worker = parseWorker(await this.#client.get<unknown>(workerUrl(id)));
        if (this.#closed || !dialog.open || !dialog.isConnected) return;
        const observation = worker.last_observation;
        const yesNo = (input: boolean | null | undefined) => input == null ? "Inconnu" : input ? "Oui" : "Non";
        const raw = el("pre", "code-block");
        raw.textContent = JSON.stringify(worker, null, 2);
        const entities = el("div", "worker-entity-links");
        this.#workerEntities = { slot: entities, worker };
        entities.append(this.#relatedEntities(worker));
        body.replaceChildren(el("div", "detail-badges", state(worker),
          badge(worker.quota_held ? "Quota retenu" : "Quota libéré")),
          el("p", "muted", `État lu le ${formatDate(new Date().toISOString(), true)}. Dernière observation runtime datée ci-dessous.`),
          worker.unknown_reason ? el("p", "control-warning", worker.unknown_reason) : "",
          el("dl", "detail-grid", value("Worker", worker.worker_id), value("Session", worker.launch.session_id),
            value("Tentative", worker.attempt.attempt_id), value("Révision durable", worker.revision),
            value("Premier démarrage observé", formatDate(worker.observed_started_at, true)),
            value("Allocation incohérente constatée", worker.allocation_inconsistent ? "Oui · aucune durée déduite" : "Non signalée"),
            value("Runtime", worker.launch.runtime_kind), value("Profil", worker.launch.profile),
            value("Fournisseur", worker.launch.provider), value("Conteneur", worker.attempt.container_id),
            value("Nom runtime stable", worker.attempt.stable_name), value("Volume de travail", worker.attempt.volume_name),
            value("Capacité CPU allouée", `${formatMetric(worker.launch.cpu_millis)} milliCPU`),
            value("Mémoire allouée", memory(worker.launch.memory_bytes)),
            value("Limite de processus", worker.launch.pids_limit), value("Timeout", `${formatMetric(worker.launch.timeout_seconds)} s`),
            value("Réseau configuré", worker.launch.network)),
          el("h3", "", "Dernière observation runtime"),
          !observation ? el("p", "muted", "Aucune observation reçue. L’état et la durée runtime restent inconnus.") :
            el("dl", "detail-grid", value("Observé le", formatDate(observation.observed_at, true)),
              value("État inspecté", observation.status), value("En cours lors de l’inspection", yesNo(observation.running)),
              value("Début observé", formatDate(observation.started_at, true)), value("Fin observée", formatDate(observation.finished_at, true)),
              value("Code de sortie", observation.exit_code), value("Arrêt mémoire (OOM)", yesNo(observation.oom_killed)),
              value("Redémarrages observés", observation.restart_count)),
          el("h3", "", "Nettoyage"), el("dl", "detail-grid", value("Retrait constaté", yesNo(worker.cleanup_removed)), value("Erreur de nettoyage", worker.cleanup_error)),
          entities, el("h3", "", "Champs de consultation reçus"), raw);
      } catch (error) {
        if (this.#closed || !dialog.open || !dialog.isConnected) return;
        if (error instanceof ApiError && (error.status === 401 || error.status === 403)) { this.#onAuthError(); return; }
        const message = el("p", "control-warning", error instanceof ApiError && error.status === 404 ?
          "Worker introuvable dans le périmètre du jeton, ou lecture indisponible sur ce collecteur." : failure(error));
        message.setAttribute("role", "alert");
        body.replaceChildren(message, action("Réessayer", "retry-worker", () => { void load(); }));
      } finally {
        body.setAttribute("aria-busy", "false");
      }
    };
    await load();
  }
}
