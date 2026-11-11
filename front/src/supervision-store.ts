// Asynchronous state of the unified supervision view. No DOM and no CSS: importable by the Node tests.
// One store lives for one selected context. It owns every read it started and forgets them all on close.
import { HumanApiError, HumanRecentUnavailable, HumanSuperseded } from "./human-api.ts";
import { HumanContractError } from "./human-model.ts";
import { supervisionAccess } from "./supervision-model.ts";
import type { HumanBroker, HumanBrokerPage, HumanContext, HumanEvent, HumanEventPage, HumanRecentEventPage, HumanFederationSelection, HumanFederationWorkerPage, HumanPage, HumanResource, HumanSelection, HumanWorker, HumanWorkerPage, ReadReceipt } from "./human-model.ts";
import type { SourcePhase, SourceSummary, SupervisionAccess } from "./supervision-model.ts";

/** The reads of HumanApiClient this view may use. Nothing else of the client is reachable from here. */
export interface SupervisionReads {
  readonly readBudgetStopped: boolean;
  federationWorkers(selection: HumanFederationSelection, signal?: AbortSignal): Promise<HumanFederationWorkerPage>;
  workers(cursor: string | null, signal?: AbortSignal, limit?: number): Promise<HumanWorkerPage>;
  brokers(cursor: string | null, signal?: AbortSignal, limit?: number): Promise<HumanBrokerPage>;
  resources(cursor: string | null, signal?: AbortSignal, limit?: number): Promise<HumanPage<HumanResource>>;
  events(selection: HumanSelection, signal?: AbortSignal): Promise<HumanEventPage>;
  recentEvents(selection: HumanSelection, signal?: AbortSignal): Promise<HumanRecentEventPage>;
}
export interface SupervisionClock {
  now(): number; visible(): boolean;
  setTimer(run: () => void, milliseconds: number): unknown; clearTimer(handle: unknown): void;
}
export const SUPERVISION_PAGE = 100;
export const MAX_WORKER_PAGES = 5;
export const MAX_RESOURCE_PAGES = 3;
/** Beyond this window a cycle would cost too many durable read receipts: refresh becomes explicit. */
export const AUTO_REFRESH_MAX_PAGES = 2;
export const SESSION_EVENTS_LIMIT = 25;
export const CADENCES = [30000, 120000, 0] as const;

/** "withdrawn": rows removed after a refusal whose reason could not be read, until the accesses are read again. */
export interface SourceError { kind: "unreachable" | "operator" | "capacity" | "budget" | "contract" | "server" | "withdrawn"; message: string }
/** `loadedAt` is the time of the last successful read of the source. */
export interface Source<T> { phase: SourcePhase; value: T | null; loadedAt: number | null; stale: boolean; busy: boolean; error: SourceError | null }
/** `oldestReadAt`: read time of the oldest page still in the window. Its freshness is that of this page. */
export interface WorkerWindow { items: HumanWorker[]; pages: number; hasMore: boolean; capped: boolean; evaluatedAt: string; receipts: ReadReceipt[]; oldestReadAt: number }
/** With several worker pages, the broker snapshot of the last page read is the one kept. */
export interface BrokerWindow { items: HumanBroker[]; hasMore: boolean; evaluatedAt: string; receipt: ReadReceipt }
export interface ResourceWindow { items: HumanResource[]; pages: number; hasMore: boolean; capped: boolean; receipt: ReadReceipt; oldestReadAt: number }
export interface SessionEvents { items: HumanEvent[]; has_more: boolean; read_receipt: ReadReceipt }
/** `order` is "oldest_first" only when the control does not serve the newest-first read. */
export interface SessionRead { sessionId: string; phase: "loading" | "ready" | "refused" | "error" | "not_granted"; page: SessionEvents | null; order: "newest_first" | "oldest_first"; message: string }
export interface SupervisionState {
  access: SupervisionAccess;
  workers: Source<WorkerWindow>; brokers: Source<BrokerWindow>; resources: Source<ResourceWindow>;
  session: SessionRead | null;
  /** Automatic refresh period of the fleet in milliseconds; 0 means explicit refresh only. */
  cadence: number;
  /** Why the automatic refresh is not running, when it is not. "not_granted": this context grants no registry, so nothing was refused. */
  autoPaused: "" | "hidden" | "window" | "budget" | "forbidden" | "not_granted";
  closed: boolean;
}
/**
 * One short sentence for assistive technology about the events read for a session, or null while nothing was
 * read or a read is in flight. It carries no event, no identifier and no receipt.
 */
export function sessionAnnouncement(read: SessionRead | null): string | null {
  if (!read || read.phase === "loading") return null;
  if (read.phase !== "ready" || !read.page) return read.message;
  if (!read.page.items.length) return "Aucun événement autorisé pour cette session à cette lecture.";
  return `${read.page.items.length} événement(s) reçus pour cette session, ${read.order === "newest_first" ? "du plus récent au plus ancien" : "du plus ancien au plus récent"}${
    read.page.has_more ? " ; d’autres existent" : ""}.`;
}
const source = <T>(granted: boolean): Source<T> => ({ phase: granted ? "idle" : "not_granted", value: null, loadedAt: null, stale: false, busy: false, error: null });
const contextCodes = ["context_required", "context_invalid", "context_expired", "context_revoked"];

export function sourceError(error: unknown): SourceError {
  if (error instanceof HumanApiError) {
    if (error.status === 429 && error.code === "audit_read_budget_exhausted")
      return { kind: "budget", message: "Budget durable de lecture atteint. Les lectures automatiques sont arrêtées ; contactez l’opérateur." };
    if (error.status === 429) return { kind: "capacity", message: `Capacité temporairement atteinte côté contrôle. ${error.message} Réessayez dans un instant.` };
    if (error.retryAction === "contact_operator") return { kind: "operator", message: `${error.message} Action requise : contactez l’opérateur.` };
    if (error.code === "identity_unavailable") return { kind: "unreachable", message: `Contrôle injoignable depuis le dashboard. ${error.message}` };
    return { kind: "server", message: error.message };
  }
  if (error instanceof HumanContractError) return { kind: "contract", message: error.message };
  if (error instanceof Error && /^Réponse trop volumineuse/.test(error.message)) return { kind: "server", message: error.message };
  return { kind: "unreachable", message: "Contrôle injoignable ou délai dépassé. Vérifiez la connexion au dashboard puis réessayez." };
}
export function summarize(state: Source<{ items: unknown[]; hasMore: boolean; capped?: boolean }>): SourceSummary {
  return { phase: state.phase, loaded: state.value?.items.length ?? 0, hasMore: state.value?.hasMore ?? false, stale: state.stale, capped: state.value?.capped ?? false,
    withdrawn: state.error?.kind === "withdrawn" };
}

interface Fleet { workers: WorkerWindow | null; brokers: BrokerWindow | null; cursor: string | null }

export class SupervisionStore {
  #reads: SupervisionReads; #clock: SupervisionClock; #onChange: () => void; #onAccessDoubt: () => void;
  #state: SupervisionState;
  #generation = 0;
  #abort = new AbortController();
  #detailAbort = new AbortController();
  #detailRequest = 0;
  #workerCursor: string | null = null;
  #resourceCursor: string | null = null;
  #timer: unknown = null;
  #suspended = false;

  /**
   * `onAccessDoubt` is called once when a read was refused without a readable reason: the host reads the
   * identity again, which alone tells a lost context from a refused registry, then refreshes this store.
   */
  constructor(reads: SupervisionReads, context: HumanContext, clock: SupervisionClock, onChange: () => void, onAccessDoubt: () => void = () => {}) {
    this.#reads = reads; this.#clock = clock; this.#onChange = onChange; this.#onAccessDoubt = onAccessDoubt;
    const access = supervisionAccess(context);
    this.#state = { access, workers: source(access.workers), brokers: source(access.brokers), resources: source(access.resources),
      session: null, cadence: CADENCES[0], autoPaused: access.workers || access.brokers ? "" : "not_granted", closed: false };
  }
  /** Live state. Callers read it and never mutate it. */
  get state(): SupervisionState { return this.#state; }
  open(): Promise<void> { return this.refresh(false); }

  #publish() { if (!this.#state.closed) this.#onChange(); }
  #fleetGranted() { return this.#state.access.workers || this.#state.access.brokers; }
  #fleetSources() { return [this.#state.workers, this.#state.brokers].filter((item) => item.phase !== "not_granted"); }
  #fleetReadable() { return this.#fleetGranted() && this.#fleetSources().every((item) => item.phase !== "forbidden"); }
  #begin(sources: Source<unknown>[]) {
    for (const item of sources) { item.busy = true; if (!item.value) item.phase = "loading"; }
  }
  #fail(sources: Source<unknown>[], error: unknown) {
    const failure = sourceError(error);
    for (const item of sources) { item.error = failure; item.stale = !!item.value; item.phase = item.value ? "ready" : "error"; }
  }
  #forbid(sources: Source<unknown>[]) {
    for (const item of sources) { item.phase = "forbidden"; item.value = null; item.loadedAt = null; item.stale = false; item.error = null; }
  }
  /**
   * The composed read publishes its 403 without reading the body, so that rows leave at once. The reason is
   * therefore unknown here: a revoked or expired context as well as a refused registry. Nothing read under
   * this context stays displayed, and the host reads the identity once to decide.
   */
  #refuseFleet(sources: Source<unknown>[]) {
    this.#forbid(sources); this.#workerCursor = null; this.clearSessionEvents(false);
    if (!this.#state.access.composed) return;
    const resources = this.#state.resources;
    if (resources.phase !== "not_granted" && resources.phase !== "forbidden") {
      resources.value = null; resources.loadedAt = null; resources.stale = false; this.#resourceCursor = null;
      // A resource read still in flight keeps its own outcome: the control answers it for this same context.
      if (resources.busy) resources.phase = "loading";
      else {
        resources.phase = "error";
        resources.error = { kind: "withdrawn", message: "Ressources retirées : le contrôle a refusé la lecture des registres sans motif lisible. Vos accès sont relus ; réessayez si elles ne reviennent pas." };
      }
    }
    this.#onAccessDoubt();
  }
  /** Returns true when the failure belongs to a superseded read or closed the store. */
  #settled(error: unknown, generation: number): boolean {
    if (generation !== this.#generation || this.#state.closed || error instanceof HumanSuperseded) return true;
    if (error instanceof HumanApiError && (error.status === 401 || contextCodes.includes(error.code))) {
      // The client already notified the dashboard, which owns the context choice. Nothing stays displayed.
      this.close(); return true;
    }
    return false;
  }

  async #fleetPage(cursor: string | null, signal: AbortSignal): Promise<{ workers: HumanWorkerPage | null; brokers: HumanBrokerPage | null }> {
    const access = this.#state.access;
    if (access.composed) {
      const page = await this.#reads.federationWorkers({ workerLimit: SUPERVISION_PAGE, brokerLimit: SUPERVISION_PAGE, workerCursor: cursor }, signal);
      return { workers: page.workers, brokers: page.brokers };
    }
    if (access.workers) return { workers: await this.#reads.workers(cursor, signal, SUPERVISION_PAGE), brokers: null };
    return { workers: null, brokers: await this.#reads.brokers(null, signal, SUPERVISION_PAGE) };
  }
  #brokerWindow(page: HumanBrokerPage | null): BrokerWindow | null {
    return page && { items: page.items, hasMore: page.has_more, evaluatedAt: page.evaluated_at, receipt: page.read_receipt };
  }
  /** Reads `pages` worker pages from the start. The result is published only as a whole. */
  async #readFleet(pages: number, signal: AbortSignal): Promise<Fleet> {
    let cursor: string | null = null; let brokers: BrokerWindow | null = null; let workers: WorkerWindow | null = null;
    for (let index = 0; index < pages; index++) {
      const page = await this.#fleetPage(cursor, signal);
      brokers = this.#brokerWindow(page.brokers);
      if (!page.workers) return { workers: null, brokers, cursor: null };
      workers = this.#append(workers, page.workers);
      cursor = page.workers.next_cursor;
      if (!cursor) break;
    }
    return { workers, brokers, cursor };
  }
  #append(loaded: WorkerWindow | null, page: HumanWorkerPage): WorkerWindow {
    const rows = new Map((loaded?.items ?? []).map((item) => [item.worker_id, item]));
    // Pages are independent snapshots: a row seen again keeps its position and takes its latest content.
    for (const item of page.items) rows.set(item.worker_id, item);
    const pages = (loaded?.pages ?? 0) + 1;
    return { items: [...rows.values()], pages, hasMore: page.has_more, capped: page.has_more && pages >= MAX_WORKER_PAGES,
      evaluatedAt: page.evaluated_at, receipts: [...(loaded?.receipts ?? []), page.read_receipt], oldestReadAt: loaded?.oldestReadAt ?? this.#clock.now() };
  }
  #publishFleet(fleet: Fleet) {
    const now = this.#clock.now();
    const ready = <T>(target: Source<T>, value: T | null) => {
      if (target.phase === "not_granted" || value === null) return;
      target.phase = "ready"; target.value = value; target.loadedAt = now; target.stale = false; target.error = null;
    };
    ready(this.#state.workers, fleet.workers); ready(this.#state.brokers, fleet.brokers);
    this.#workerCursor = fleet.cursor;
  }
  async #loadFleet(pages: number): Promise<void> {
    const sources = this.#fleetSources();
    if (!this.#fleetReadable() || sources.some((item) => item.busy)) return;
    const generation = this.#generation; this.#begin(sources); this.#publish();
    try {
      const fleet = await this.#readFleet(pages, this.#abort.signal);
      if (generation !== this.#generation || this.#state.closed) return;
      this.#publishFleet(fleet);
    } catch (error) {
      if (this.#settled(error, generation)) return;
      if (error instanceof HumanApiError && error.status === 403) this.#refuseFleet(sources);
      else this.#fail(sources, error);
    } finally {
      if (generation === this.#generation) { for (const item of sources) item.busy = false; this.#publish(); }
    }
  }
  async #loadResources(pages: number): Promise<void> {
    const target = this.#state.resources;
    if (["not_granted", "forbidden"].includes(target.phase) || target.busy) return;
    const generation = this.#generation; this.#begin([target]); this.#publish();
    try {
      let cursor: string | null = null; let loaded: ResourceWindow | null = null;
      for (let index = 0; index < pages; index++) {
        const page = await this.#reads.resources(cursor, this.#abort.signal, SUPERVISION_PAGE);
        loaded = this.#appendResources(loaded, page); cursor = page.next_cursor;
        if (!cursor) break;
      }
      if (generation !== this.#generation || this.#state.closed) return;
      target.phase = "ready"; target.value = loaded; target.loadedAt = this.#clock.now(); target.stale = false; target.error = null;
      this.#resourceCursor = cursor;
    } catch (error) {
      if (this.#settled(error, generation)) return;
      if (error instanceof HumanApiError && error.status === 403) { this.#forbid([target]); this.#resourceCursor = null; }
      else this.#fail([target], error);
    } finally {
      if (generation === this.#generation) { target.busy = false; this.#publish(); }
    }
  }
  #appendResources(loaded: ResourceWindow | null, page: HumanPage<HumanResource>): ResourceWindow {
    const key = (item: HumanResource) => `${item.resource.kind}\n${item.resource.resource_id}`;
    const rows = new Map((loaded?.items ?? []).map((item) => [key(item), item]));
    for (const item of page.items) rows.set(key(item), item);
    const pages = (loaded?.pages ?? 0) + 1;
    return { items: [...rows.values()], pages, hasMore: page.has_more, capped: page.has_more && pages >= MAX_RESOURCE_PAGES, receipt: page.read_receipt,
      oldestReadAt: loaded?.oldestReadAt ?? this.#clock.now() };
  }

  /**
   * Re-reads the whole loaded window from its first page. An automatic cycle reads the fleet only,
   * and only while the tab is visible, the window is small and the durable read budget is open.
   */
  async refresh(automatic = false): Promise<void> {
    if (this.#state.closed) return;
    if (automatic) {
      const paused = this.#autoBlock();
      if (paused) { this.#pause(paused); return; }
    } else this.#explicit();
    // A read that starts in a visible tab ends the "hidden tab" notice at once, not when its answer arrives.
    if (this.#state.autoPaused === "hidden" && this.#clock.visible()) { this.#state.autoPaused = ""; this.#publish(); }
    this.#cancelTimer();
    const fleetPages = Math.max(1, this.#state.workers.value?.pages ?? 1);
    const resourcePages = Math.max(1, this.#state.resources.value?.pages ?? 1);
    if (automatic) await this.#loadFleet(fleetPages);
    else if (this.#reads.readBudgetStopped && this.#fleetReadable()) {
      // After an operator intervention the client grants one attempt. The fleet takes it; the resources are
      // read only once that attempt reopened the budget, instead of failing locally beside it.
      await this.#loadFleet(fleetPages);
      if (!this.#reads.readBudgetStopped) await this.#loadResources(resourcePages);
    } else await Promise.all([this.#loadFleet(fleetPages), this.#loadResources(resourcePages)]);
    this.#arm();
  }
  /** An action of the person in a visible tab ends a suspension, even when the host never called `resume`. */
  #explicit() { if (this.#clock.visible()) this.#suspended = false; }
  #autoBlock(): SupervisionState["autoPaused"] {
    if (!this.#fleetGranted()) return "not_granted";
    if (this.#suspended || !this.#clock.visible()) return "hidden";
    if (this.#reads.readBudgetStopped) return "budget";
    if (!this.#fleetReadable()) return "forbidden";
    if ((this.#state.workers.value?.pages ?? 1) > AUTO_REFRESH_MAX_PAGES) return "window";
    return "";
  }
  #pause(reason: SupervisionState["autoPaused"]) {
    this.#cancelTimer();
    if (this.#state.autoPaused !== reason) { this.#state.autoPaused = reason; this.#publish(); }
  }
  #cancelTimer() { if (this.#timer !== null) { this.#clock.clearTimer(this.#timer); this.#timer = null; } }
  #arm() {
    this.#cancelTimer();
    if (this.#state.closed) return;
    const paused = !this.#fleetGranted() ? "not_granted" : this.#state.cadence === 0 ? "" : this.#autoBlock();
    if (this.#state.autoPaused !== paused) { this.#state.autoPaused = paused; this.#publish(); }
    if (this.#state.cadence === 0 || paused) return;
    this.#timer = this.#clock.setTimer(() => { this.#timer = null; void this.refresh(true); }, this.#state.cadence);
  }
  setCadence(milliseconds: number) {
    if (!(CADENCES as readonly number[]).includes(milliseconds)) throw new Error("Unsupported refresh cadence.");
    if (this.#state.closed) return;
    this.#explicit(); this.#state.cadence = milliseconds; this.#arm(); this.#publish();
  }
  /** The tab is hidden: no read is started until `resume`. */
  suspend() {
    if (this.#state.closed) return;
    this.#suspended = true; this.#pause("hidden");
  }
  /**
   * The tab is visible again: one fleet read, then the normal cycle. Called on a tab that is hidden again,
   * it reads nothing and the next return resumes; called twice, it reads once.
   */
  resume() {
    if (this.#state.closed || !this.#suspended && this.#state.autoPaused !== "hidden") return;
    if (!this.#clock.visible()) { this.#suspended = true; this.#pause("hidden"); return; }
    this.#suspended = false;
    if (this.#state.cadence === 0) { this.#arm(); return; }
    void this.refresh(true);
  }

  async loadMoreWorkers(): Promise<void> {
    const target = this.#state.workers; const loaded = target.value;
    if (this.#state.closed || !loaded || !loaded.hasMore || loaded.capped || !this.#workerCursor || this.#fleetSources().some((item) => item.busy)) return;
    const generation = this.#generation; const sources = this.#fleetSources();
    for (const item of sources) item.busy = true;
    this.#publish();
    try {
      const page = await this.#fleetPage(this.#workerCursor, this.#abort.signal);
      if (generation !== this.#generation || this.#state.closed || !page.workers) return;
      const now = this.#clock.now();
      // The rows already loaded keep the read time of their own page: see WorkerWindow.oldestReadAt.
      target.value = this.#append(loaded, page.workers); target.loadedAt = now; target.error = null;
      const brokers = this.#brokerWindow(page.brokers);
      if (brokers && this.#state.brokers.phase !== "not_granted") { this.#state.brokers.value = brokers; this.#state.brokers.loadedAt = now; this.#state.brokers.error = null; }
      this.#workerCursor = page.workers.next_cursor;
    } catch (error) {
      if (this.#settled(error, generation)) return;
      if (error instanceof HumanApiError && error.status === 403) this.#refuseFleet(sources);
      // The loaded window is unchanged and exactly as fresh as before: only the failure is reported.
      else { const failure = sourceError(error); for (const item of sources) item.error = failure; }
    } finally {
      if (generation === this.#generation) { for (const item of sources) item.busy = false; this.#arm(); this.#publish(); }
    }
  }
  async loadMoreResources(): Promise<void> {
    const target = this.#state.resources; const loaded = target.value;
    if (this.#state.closed || !loaded || !loaded.hasMore || loaded.capped || !this.#resourceCursor || target.busy) return;
    const generation = this.#generation; target.busy = true; this.#publish();
    try {
      const page = await this.#reads.resources(this.#resourceCursor, this.#abort.signal, SUPERVISION_PAGE);
      if (generation !== this.#generation || this.#state.closed) return;
      target.value = this.#appendResources(loaded, page); target.loadedAt = this.#clock.now(); target.error = null;
      this.#resourceCursor = page.next_cursor;
    } catch (error) {
      if (this.#settled(error, generation)) return;
      if (error instanceof HumanApiError && error.status === 403) { this.#forbid([target]); this.#resourceCursor = null; }
      else target.error = sourceError(error);
    } finally {
      if (generation === this.#generation) { target.busy = false; this.#publish(); }
    }
  }

  /**
   * Server-authorized read of the events of one declared session. A refusal stays in this detail:
   * the control does not distinguish an absent resource from one this context may not read.
   */
  async readSessionEvents(sessionId: string): Promise<void> {
    if (this.#state.closed) return;
    this.#detailAbort.abort(); this.#detailAbort = new AbortController(); const request = ++this.#detailRequest;
    if (!this.#state.access.journal) {
      this.#state.session = { sessionId, phase: "not_granted", page: null, order: "newest_first",
        message: "Ce contexte ne porte aucune capacité de métadonnées : les événements de session ne sont pas lus." };
      this.#publish(); return;
    }
    const generation = this.#generation;
    this.#state.session = { sessionId, phase: "loading", page: null, order: "newest_first", message: "" }; this.#publish();
    const current = () => generation === this.#generation && request === this.#detailRequest && !this.#state.closed;
    try {
      const selection: HumanSelection = { view: "metadata", limit: SESSION_EVENTS_LIMIT, resource: { kind: "session", resource_id: sessionId } };
      const signal = AbortSignal.any([this.#abort.signal, this.#detailAbort.signal]);
      let page: SessionEvents; let order: SessionRead["order"] = "newest_first";
      try { page = await this.#reads.recentEvents(selection, signal); }
      catch (error) {
        // An older control: the forward read stays the authority, including for the refusal of this resource.
        if (!(error instanceof HumanRecentUnavailable) || !current()) throw error;
        order = "oldest_first"; page = await this.#reads.events(selection, signal);
      }
      if (!current()) return;
      this.#state.session = { sessionId, phase: "ready", page, order, message: "" };
    } catch (error) {
      if (request !== this.#detailRequest || this.#settled(error, generation)) return;
      if (error instanceof HumanRecentUnavailable) return;
      const refused = error instanceof HumanApiError && (error.code === "not_found" || error.status === 403);
      this.#state.session = { sessionId, phase: refused ? "refused" : "error", page: null, order: "newest_first",
        message: error instanceof HumanApiError && error.code === "not_found" ?
          "Aucune ressource autorisée de ce contexte ne correspond à cette session déclarée. Le contrôle ne distingue pas une ressource absente d’une ressource non accordée." :
          refused ? "Lecture des événements de cette session refusée par le contrôle pour ce contexte." : sourceError(error).message };
    }
    if (current()) this.#publish();
  }
  clearSessionEvents(publish = true) {
    this.#detailAbort.abort(); this.#detailRequest++;
    if (this.#state.session === null) return;
    this.#state.session = null;
    if (publish) this.#publish();
  }

  /** Aborts every read and timer and drops every row. Late responses are ignored. */
  close() {
    if (this.#state.closed) return;
    this.#generation++; this.#abort.abort(); this.#detailAbort.abort(); this.#detailRequest++;
    this.#cancelTimer(); this.#workerCursor = null; this.#resourceCursor = null;
    const access = this.#state.access;
    this.#state = { access, workers: source(access.workers), brokers: source(access.brokers), resources: source(access.resources),
      session: null, cadence: this.#state.cadence, autoPaused: "", closed: true };
  }
}
