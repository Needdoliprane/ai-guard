// State of the Journal: the newest authorized events first, then a live stream that starts exactly after
// the page that was read. No DOM and no CSS. One store lives for one selected context.
import { HumanApiError, HumanRecentUnavailable, HumanSuperseded } from "./human-api.ts";
import { sourceError } from "./supervision-store.ts";
import { HumanContractError } from "./human-model.ts";
import type { HumanEvent, HumanEventPage, HumanRecentEventPage, HumanSelection, ReadReceipt, ResourceRef } from "./human-model.ts";
import type { SourceError, SupervisionClock } from "./supervision-store.ts";

export interface JournalReads {
  readonly readBudgetStopped: boolean;
  recentEvents(selection: HumanSelection, signal?: AbortSignal): Promise<HumanRecentEventPage>;
  listen(selection: HumanSelection, signal: AbortSignal, onPage: (page: HumanEventPage, position: string) => void,
    onStatus: (status: "connecting" | "live" | "retrying" | "closed") => void): Promise<void>;
}
export const JOURNAL_PAGE = 100;
export const JOURNAL_MAX_EVENTS = 1000;
/** A stream the control refuses for a while ("retry") is reopened after 1, 2, 4, 8, 16 and 30 seconds, then stops. */
export const LIVE_RETRY_DELAYS = [1000, 2000, 4000, 8000, 16000, 30000] as const;
/** A stream that stayed connected this long was stable: its next refusal starts a new series of attempts. */
export const LIVE_STABLE_MS = 60000;
export type JournalPhase = "idle" | "loading" | "ready" | "error" | "forbidden" | "refused" | "unavailable";
export type LiveState = "off" | "connecting" | "live" | "retrying" | "suspended" | "refused" | "unavailable";
export interface JournalState {
  phase: JournalPhase;
  /** Server-side restriction to one authorized resource, or null for the whole authorized selection. */
  resource: ResourceRef | null;
  /** Newest first, without duplicate, at most JOURNAL_MAX_EVENTS. */
  items: HumanEvent[];
  loadedAt: number | null; stale: boolean; busy: boolean; error: SourceError | null;
  /** Older authorized events exist and can be read below this window. */
  hasOlder: boolean;
  /** The window reached its bound: its oldest events left it and the older position was dropped. */
  trimmed: boolean;
  live: LiveState; liveMessage: string;
  streamMode: "snapshot_only" | null;
  /** Events delivered by the live stream since the last read from the newest. */
  received: number;
  receipt: ReadReceipt | null;
  closed: boolean;
}
const contextCodes = ["context_required", "context_invalid", "context_expired", "context_revoked"];
const SNAPSHOT_DIRECT = "Direct indisponible pour ce profil.";
/**
 * One short sentence for assistive technology, or null while a read or a connection is in flight. It carries
 * neither a time nor a count: an unchanged state gives the same sentence, so a live batch announces nothing.
 */
export function journalAnnouncement(state: JournalState): string | null {
  if (state.closed || state.busy || state.phase === "idle" || state.phase === "loading" || state.live === "connecting") return null;
  if (state.phase !== "ready") return state.phase === "forbidden" ? "Journal : lecture refusée, événements retirés." :
    state.phase === "unavailable" ? "Journal : lecture récente indisponible sur ce contrôle." :
    state.phase === "refused" ? "Journal : ressource non accordée." : "Journal : lecture en échec, contenu inconnu.";
  const live = { off: "Direct arrêté.", live: "Direct connecté.", retrying: "Direct déconnecté, reconnexion en cours.", suspended: "Direct suspendu, onglet masqué.", refused: "Direct refusé.", unavailable: SNAPSHOT_DIRECT }[state.live];
  return `Journal : ${state.stale ? "données périmées, dernière lecture conservée" : "lecture réussie"}. ${live}`;
}
const initial = (resource: ResourceRef | null): JournalState => ({ phase: "idle", resource, items: [], loadedAt: null, stale: false, busy: false, error: null,
  hasOlder: false, trimmed: false, live: "off", liveMessage: "", streamMode: null, received: 0, receipt: null, closed: false });

export class JournalStore {
  #reads: JournalReads; #clock: SupervisionClock; #onChange: () => void;
  #state: JournalState;
  #generation = 0;
  #abort = new AbortController();
  #stream = new AbortController();
  #older: string | null = null;
  #suspended = false;
  /** Position after the last live batch delivered, from which a refused stream is reopened. */
  #position: string | null = null;
  #retries = 0;
  #retryTimer: unknown = null;
  #liveSince: number | null = null;

  constructor(reads: JournalReads, clock: SupervisionClock, onChange: () => void, resource: ResourceRef | null = null) {
    this.#reads = reads; this.#clock = clock; this.#onChange = onChange; this.#state = initial(resource);
  }
  /** Live state. Callers read it and never mutate it. */
  get state(): JournalState { return this.#state; }
  open(): Promise<void> { return this.reload(); }
  #publish() { if (!this.#state.closed) this.#onChange(); }
  #selection(cursor: string | null): HumanSelection {
    return { view: "metadata", limit: JOURNAL_PAGE, ...(this.#state.resource ? { resource: this.#state.resource } : {}), ...(cursor ? { cursor } : {}) };
  }
  /** Returns true when the failure belongs to a superseded read or closed the store. */
  #settled(error: unknown, generation: number): boolean {
    if (generation !== this.#generation || this.#state.closed || error instanceof HumanSuperseded) return true;
    if (error instanceof HumanApiError && (error.status === 401 || contextCodes.includes(error.code))) { this.close(); return true; }
    return false;
  }
  #stopStream(state: LiveState, message = "") {
    this.#stream.abort(); this.#stream = new AbortController();
    if (this.#retryTimer !== null) { this.#clock.clearTimer(this.#retryTimer); this.#retryTimer = null; }
    this.#liveSince = null;
    this.#state.live = state; this.#state.liveMessage = message;
  }
  /** Withdraw data whose current access could not be revalidated. */
  #withdraw(clearProfile = true) {
    const state = this.#state;
    const profile = clearProfile ? null : state.streamMode;
    state.items = []; state.loadedAt = null; state.receipt = null; state.hasOlder = false;
    state.streamMode = profile; state.received = 0; state.trimmed = false; state.stale = false; this.#older = null;
    this.#stopStream(profile === "snapshot_only" ? "unavailable" : "off", profile === "snapshot_only" ? SNAPSHOT_DIRECT : "");
  }

  /** Reads the newest page again and restarts the live stream right after it. */
  async reload(): Promise<void> {
    const state = this.#state;
    if (state.closed) return;
    const generation = ++this.#generation;
    // Reading from the newest in a visible tab ends a suspension, even when the host never called `resume`.
    if (this.#clock.visible()) this.#suspended = false;
    this.#abort.abort(); this.#abort = new AbortController();
    this.#stopStream(state.streamMode === "snapshot_only" ? "unavailable" : "off", state.streamMode === "snapshot_only" ? SNAPSHOT_DIRECT : "");
    this.#retries = 0; this.#position = null;
    state.busy = true; state.error = null;
    if (!state.items.length) state.phase = "loading";
    this.#publish();
    try {
      const page = await this.#reads.recentEvents(this.#selection(null), this.#abort.signal);
      if (generation !== this.#generation || state.closed) return;
      state.phase = "ready"; state.items = page.items; state.loadedAt = this.#clock.now(); state.stale = false; state.trimmed = false;
      state.received = 0; state.receipt = page.read_receipt; state.hasOlder = page.has_more; this.#older = page.next_cursor;
      state.streamMode = page.stream_mode ?? null;
      state.busy = false;
      if (state.streamMode === "snapshot_only") this.#stopStream("unavailable", SNAPSHOT_DIRECT);
      else this.#listen(page.live_cursor, generation);
    } catch (error) {
      if (this.#settled(error, generation)) return;
      if (error instanceof HumanRecentUnavailable && error.ambiguous) {
        // A filtered 404 before the route ever answered: only a read without filter tells an absent route
        // from a resource this context may not read. Its page is never displayed under the requested limit.
        try {
          await this.#reads.recentEvents({ view: "metadata", limit: 1 }, this.#abort.signal);
          if (generation !== this.#generation || state.closed) return;
          error = new HumanApiError(404, "not_found", "Aucune ressource autorisée ne correspond.", "review_permissions");
        } catch (probe) {
          if (this.#settled(probe, generation)) return;
          error = probe;
        }
      }
      state.busy = false;
      // A snapshot has no live authorization signal. Failed revalidation
      // withdraws rows, while a transient failure keeps the restrictive profile
      // fact of this same context. A503 never creates or changes that fact.
      if (state.streamMode === "snapshot_only") this.#withdraw(false);
      if (error instanceof HumanRecentUnavailable) {
        // Never open the stream without a start position: it would replay the whole authorized history.
        state.phase = "unavailable"; state.items = []; state.hasOlder = false; this.#older = null;
        state.error = { kind: "server", message: error.message };
      } else if (error instanceof HumanApiError && error.status === 403) {
        this.#withdraw(); state.phase = "forbidden";
      } else if (error instanceof HumanApiError && error.code === "not_found") {
        state.phase = "refused"; state.items = []; state.hasOlder = false; this.#older = null;
        state.error = { kind: "server", message: "Aucune ressource autorisée de ce contexte ne correspond à cette limite. Le contrôle ne distingue pas une ressource absente d’une ressource non accordée." };
      } else {
        state.error = sourceError(error); state.stale = state.items.length > 0; state.phase = state.items.length ? "ready" : "error";
      }
    }
    if (generation === this.#generation) this.#publish();
  }
  #listen(cursor: string | null, generation: number) {
    const state = this.#state;
    if (!cursor || state.closed || state.streamMode === "snapshot_only") return;
    this.#position = cursor;
    if (this.#suspended || !this.#clock.visible()) { state.live = "suspended"; return; }
    if (this.#reads.readBudgetStopped) { state.live = "refused"; state.liveMessage = "Budget durable de lecture atteint : le direct est arrêté."; return; }
    const signal = this.#stream.signal; const current = () => generation === this.#generation && !signal.aborted && !state.closed;
    // A reopened stream stays "retrying" until it is connected again.
    if (!this.#retries) state.live = "connecting";
    void this.#reads.listen(this.#selection(cursor), signal, (page, position) => {
      if (!current()) return;
      this.#position = position; this.#retries = 0;
      this.#receive(page.items); this.#publish();
    }, (status) => {
      if (!current() || status === "connecting" && this.#retries) return;
      state.live = status === "closed" ? "off" : status;
      // A reconnection resumes from the last delivered position; until then the window may miss events.
      if (status === "retrying") { state.stale = true; this.#liveSince = null; }
      else if (status === "live") { state.stale = !!state.error; state.liveMessage = ""; this.#liveSince = this.#clock.now(); }
      this.#publish();
    }).catch((error: unknown) => {
      if (!current() || this.#settled(error, generation)) return;
      state.stale = true;
      if (error instanceof HumanApiError && error.retryAction === "retry") { this.#retryLive(generation, sourceError(error).message); return; }
      state.live = "refused";
      state.liveMessage = error instanceof HumanApiError && error.retryAction === "close_unused_streams" ?
        "Trop de flux ouverts pour ce compte. Fermez les onglets inutilisés, puis relisez depuis les plus récents." : sourceError(error).message;
      this.#publish();
    });
  }
  /**
   * The control answered "retry" (gateway unreachable through the dashboard, capacity full for a moment):
   * a disconnection, not a refusal. The stream is reopened from the last delivered position with a growing
   * delay; after the last attempt it stops and says so, and the person restarts from the newest.
   */
  #retryLive(generation: number, reason: string) {
    const state = this.#state;
    if (this.#liveSince !== null && this.#clock.now() - this.#liveSince >= LIVE_STABLE_MS) this.#retries = 0;
    this.#liveSince = null;
    const delay = LIVE_RETRY_DELAYS[this.#retries];
    if (delay === undefined) {
      state.live = "off"; this.#retries = 0;
      state.liveMessage = `Le direct n’a pas pu être rétabli après ${LIVE_RETRY_DELAYS.length} tentatives. ${reason} Relisez depuis les plus récents pour le rouvrir.`;
      this.#publish(); return;
    }
    this.#retries++; state.live = "retrying"; state.liveMessage = reason;
    this.#retryTimer = this.#clock.setTimer(() => {
      this.#retryTimer = null;
      if (generation !== this.#generation || state.closed) return;
      this.#stream.abort(); this.#stream = new AbortController();
      this.#listen(this.#position, generation);
      if (generation === this.#generation) this.#publish();
    }, delay);
    this.#publish();
  }
  /** A live batch arrives oldest first; the window keeps the newest first and no duplicate. */
  #receive(batch: HumanEvent[]) {
    const state = this.#state; const known = new Set(state.items.map((item) => item.metadata.id));
    const fresh = batch.filter((item) => !known.has(item.metadata.id) && known.add(item.metadata.id)).reverse();
    if (!fresh.length) return;
    state.items = [...fresh, ...state.items]; state.received += fresh.length;
    if (state.items.length > JOURNAL_MAX_EVENTS) {
      state.items = state.items.slice(0, JOURNAL_MAX_EVENTS);
      // The position of older pages no longer follows the window: restart from the newest to read further back.
      state.trimmed = true; state.hasOlder = false; this.#older = null;
    }
  }

  async loadOlder(): Promise<void> {
    const state = this.#state;
    if (state.closed || state.busy || !this.#older || state.items.length >= JOURNAL_MAX_EVENTS) return;
    const generation = this.#generation; state.busy = true; this.#publish();
    try {
      const page = await this.#reads.recentEvents(this.#selection(this.#older), this.#abort.signal);
      if (generation !== this.#generation || state.closed) return;
      if ((page.stream_mode ?? null) !== state.streamMode) throw new HumanContractError();
      const known = new Set(state.items.map((item) => item.metadata.id));
      const merged = [...state.items, ...page.items.filter((item) => !known.has(item.metadata.id))];
      // Events cut off by the bound are a partial coverage even when this page was the last one of the control.
      const cut = merged.length > JOURNAL_MAX_EVENTS;
      state.items = merged.slice(0, JOURNAL_MAX_EVENTS);
      this.#older = cut ? null : page.next_cursor; state.hasOlder = page.has_more && state.items.length < JOURNAL_MAX_EVENTS;
      state.trimmed = cut || page.has_more && state.items.length >= JOURNAL_MAX_EVENTS; state.error = null;
      if (state.streamMode === "snapshot_only") { state.receipt = page.read_receipt; state.loadedAt = this.#clock.now(); }
    } catch (error) {
      if (this.#settled(error, generation)) return;
      if (error instanceof HumanApiError && error.status === 403) { this.#withdraw(); state.phase = "forbidden"; }
      else {
        if (state.streamMode === "snapshot_only") { this.#withdraw(false); state.phase = "error"; }
        state.error = sourceError(error);
      }
    } finally {
      if (generation === this.#generation) { state.busy = false; this.#publish(); }
    }
  }
  /** Restricts the journal to one resource on the server, or removes the restriction. */
  setResource(resource: ResourceRef | null): Promise<void> {
    if (this.#state.closed) return Promise.resolve();
    this.#state.resource = resource; this.#state.items = []; this.#state.hasOlder = false; this.#state.loadedAt = null; this.#older = null;
    if (this.#state.streamMode === "snapshot_only") { this.#state.receipt = null; this.#state.received = 0; }
    return this.reload();
  }
  /** The tab is hidden: the stream is closed and nothing is read until `resume`. */
  suspend() {
    if (this.#state.closed) return;
    this.#suspended = true;
    if (this.#state.phase === "ready") {
      this.#stopStream(this.#state.streamMode === "snapshot_only" ? "unavailable" : "suspended", this.#state.streamMode === "snapshot_only" ? SNAPSHOT_DIRECT : "");
      this.#publish();
    }
  }
  /**
   * Back in the foreground: events may have been missed, so the window restarts from the newest. Called on
   * a tab that is hidden again, it reads nothing and the next return resumes.
   */
  resume() {
    const state = this.#state;
    if (state.closed || !this.#suspended && state.live !== "suspended") return;
    if (!this.#clock.visible()) { this.#suspended = true; return; }
    this.#suspended = false;
    if (state.phase !== "ready") return;
    if (this.#reads.readBudgetStopped) {
      this.#stopStream(state.streamMode === "snapshot_only" ? "unavailable" : "refused", state.streamMode === "snapshot_only" ? `${SNAPSHOT_DIRECT} Budget durable de lecture atteint.` : "Budget durable de lecture atteint : le direct est arrêté.");
      this.#publish();
    }
    else void this.reload();
  }
  close() {
    if (this.#state.closed) return;
    const snapshot = this.#state.streamMode === "snapshot_only";
    this.#generation++; this.#abort.abort(); this.#stopStream("off"); this.#older = null;
    this.#state = { ...initial(this.#state.resource), closed: true };
    if (snapshot) this.#onChange();
  }
}
