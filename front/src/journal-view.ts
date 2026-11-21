// DOM of the Journal: every event this context may read, newest first, then live without replaying history.
// State and reads live in JournalStore. Text is always inserted as text nodes; no event is created here.
// Rows are kept across publications (see dom-sync): a live batch inserts its rows above those already displayed.
import { button, el, icon } from "./dom.ts";
import { announce, place, replaceIfChanged, setText, signatureOf, syncRows } from "./dom-sync.ts";
import { formatDate } from "./model.ts";
import { journalPolicyEventLabel } from "./journal-policy-event.ts";
import { JOURNAL_MAX_EVENTS, JournalStore, journalAnnouncement } from "./journal-store.ts";
import { FILTER_ANNOUNCE_MS, SEARCH_MAX_LENGTH, defaultEventFilter, filterEvents, resourceKindLabel, shownOfLoaded } from "./supervision-model.ts";
import "./supervision.css";
import type { HumanContext, HumanEvent, ResourceRef } from "./human-model.ts";
import type { JournalReads, JournalState } from "./journal-store.ts";
import type { EventFilter } from "./supervision-model.ts";
import type { SupervisionClock } from "./supervision-store.ts";

/** What the user typed or chose. No server row; it lives as long as the selected context. */
export interface JournalPreferences { filter: EventFilter; resource: ResourceRef | null; rendered: number }
export const JOURNAL_RENDER_STEP = 200;
/** The count of live events is announced at most this often, however dense the stream is. */
export const LIVE_COUNT_ANNOUNCE_MS = 15000;
export const defaultJournalPreferences = (): JournalPreferences => ({ filter: defaultEventFilter(), resource: null, rendered: JOURNAL_RENDER_STEP });
export interface JournalHost {
  reads: JournalReads; context: HumanContext; preferences: JournalPreferences;
  onReviewAccess: () => void;
  /** Updates the host's existing read command while this view owns a page read. */
  onReadStateChange: () => void;
  /** A positive page establishes the profile; false withdraws it after access loss. */
  onProfileMode: (mode: "snapshot_only" | null, confirmed: boolean) => void;
  /** Opens the unified view filtered on the workers that declare this session, when the context grants the worker registry. */
  openWorkers: ((sessionId: string) => void) | null;
}
const browserClock: SupervisionClock = { now: () => Date.now(), visible: () => document.visibilityState === "visible",
  setTimer: (run, milliseconds) => setTimeout(run, milliseconds), clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>) };
function badge(label: string, tone = "neutral") { return el("span", `badge badge-${tone}`, label); }
function action(label: string, key: string, call: () => void, disabled = false) {
  const node = button(label, "button button-secondary", call); node.dataset.focusKey = key; node.disabled = disabled; return node;
}
function field(label: string, control: HTMLInputElement | HTMLSelectElement, id: string) {
  control.id = id; const caption = el("label", "", label); caption.htmlFor = id; return el("div", "sv-field", caption, control);
}
function select<T extends string>(options: [T, string][], value: T, change: (value: T) => void) {
  const node = el("select", "select-input");
  for (const [key, label] of options) { const option = el("option", "", label); option.value = key; node.append(option); }
  node.value = value; node.addEventListener("change", () => change(node.value as T));
  return node;
}
const liveLabel: Record<JournalState["live"], [string, string]> = { off: ["Direct arrêté", "amber"], connecting: ["Connexion au direct…", "neutral"], live: ["Direct connecté", "teal"],
  retrying: ["Direct déconnecté · reconnexion", "amber"], suspended: ["Direct suspendu · onglet masqué", "neutral"], refused: ["Direct refusé", "amber"],
  unavailable: ["Direct indisponible pour ce profil", "neutral"] };
const decisionLabel = { allow: "Autorisé", deny: "Refusé", review: "À examiner" } as const;

export class JournalView {
  readonly element = el("div", "sv sv-journal");
  #host: JournalHost; #store: JournalStore; #preferences: JournalPreferences;
  #status = el("section", "sv-status"); #panel = el("section", "panel sv-panel");
  /**
   * The only live region of the view: one short sentence on a change of state, the live count at a bounded
   * rate, and the result of what the person asked for: a search or a filter once they paused, a read once it
   * ended. The result line itself is not a live region: it changes with every live batch.
   */
  #announce = el("p", "sr-only sv-announce"); #stateSaid = ""; #countSaid = 0; #countAt = 0; #countTimer: ReturnType<typeof setTimeout> | undefined;
  #resultTimer: ReturnType<typeof setTimeout> | undefined;
  /**
   * A read the person asked for ended and its outcome is still to be said, with the state it settles in.
   * Since that read was asked: `#stateSpoken`, the state was said; `#alerted`, an error line appeared or
   * changed and announced the failure as an alert.
   */
  #asked = 0; #resultDue = false; #stateSpoken = false; #alerted = false;
  #sourcesBox = el("div", "sv-sources"); #errorsBox = el("div", "sv-errors");
  #directHelp = el("p", "muted sv-auto");
  #coverageHelp = el("p", "");
  #caption = el("caption", "");
  // Like every control of this view that starts a read, it stays focusable while a read is in flight:
  // announced as unavailable, it ignores activation and keeps the focus.
  #reload = action("Relire depuis les plus récents", "jr-reload", () => { void this.#readAgain(); });
  /** « Réessayer » was activated and its read is in flight: its error line stays, so that it keeps the focus. */
  #retrying = false;
  #tools = el("div", "sv-toolbar sv-toolbar-journal"); #limit = el("div", "sv-limit"); #summary = el("p", "sv-summary"); #body = el("div", "sv-body");
  #table = el("table", "sv-table sv-table-journal"); #rows = el("tbody"); #empty = el("div"); #footer = el("div", "table-footer sv-footer");
  #renderMore = action("", "jr-render-more", () => this.#showMore());
  #older = action("Charger 100 événements plus anciens", "jr-older", () => { if (this.#older.getAttribute("aria-disabled") !== "true") this.#sayAfter(this.#store.loadOlder()); });
  #end = el("span", "muted sv-end", "Début de la sélection autorisée à cette lecture");
  #closed = false;

  constructor(host: JournalHost) {
    this.#host = host; this.#preferences = host.preferences;
    this.#store = new JournalStore(host.reads, browserClock, () => this.#changed(), this.#preferences.resource);
    this.#announce.setAttribute("role", "status"); this.#announce.setAttribute("aria-live", "polite");
    this.#summary.tabIndex = -1; this.#end.tabIndex = -1;
    this.#buildTools(); this.#buildTable();
    this.#status.append(this.#announce, this.#sourcesBox, this.#errorsBox, el("div", "sv-controls", this.#reload, this.#directHelp));
    this.#panel.append(el("div", "panel-heading", el("div", "", el("h2", "", "Événements autorisés, les plus récents d’abord"),
      el("p", "", "Métadonnées des événements rattachés aux ressources que ce contexte peut lire. Les reçus de lecture et les événements de ressources non accordées n’y figurent pas ; cette liste n’est ni l’audit complet ni un inventaire runtime."),
      this.#coverageHelp)),
      this.#tools, this.#limit, this.#summary, this.#body);
    this.element.append(this.#status, this.#panel);
    this.#render();
  }
  start() { void this.#store.open(); }
  /** For the host, on an explicit action of the person only: the result of this read is announced. */
  reload() { return this.#readAgain(); }
  get reading() { return this.#store.state.busy; }
  suspend() { this.#store.suspend(); }
  resume() { this.#store.resume(); }
  close() { this.#closed = true; clearTimeout(this.#countTimer); clearTimeout(this.#resultTimer); this.#store.close(); this.element.replaceChildren(); }

  #changed() {
    if (this.#closed) return;
    this.#render();
    const state = this.#store.state;
    if (!state.closed && (state.streamMode === "snapshot_only" || state.phase === "ready" && state.receipt !== null)) {
      this.#host.onProfileMode(state.streamMode, true);
    } else if (state.closed || state.phase === "forbidden") {
      this.#host.onProfileMode(null, false);
    }
    this.#host.onReadStateChange();
  }

  #buildTools() {
    const filter = this.#preferences.filter; const changed = () => { this.#preferences.rendered = JOURNAL_RENDER_STEP; this.#renderList(); this.#announceResult(); };
    const search = el("input", "search-input sv-search"); search.type = "search"; search.maxLength = SEARCH_MAX_LENGTH; search.value = filter.text;
    search.autocomplete = "off"; search.spellcheck = false; search.placeholder = "Type, ressource, source, identifiant";
    search.addEventListener("input", () => { filter.text = search.value; changed(); });
    const decision = select<EventFilter["decision"]>([["all", "Toutes"], ["allow", "Autorisé"], ["deny", "Refusé"], ["review", "À examiner"], ["none", "Non fournie"]], filter.decision, (value) => { filter.decision = value; changed(); });
    const provenance = select<EventFilter["provenance"]>([["all", "Toutes"], ["observed", "Observé"], ["declared", "Déclaré"], ["inferred", "Inféré"]], filter.provenance, (value) => { filter.provenance = value; changed(); });
    const kind = select<EventFilter["kind"]>([["all", "Toutes"], ["session", "Sessions"], ["agent", "Agents"], ["sandbox", "Sandboxes"], ["project", "Projets"]], filter.kind, (value) => { filter.kind = value; changed(); });
    const reset = action("Réinitialiser", "jr-reset", () => {
      Object.assign(filter, defaultEventFilter()); search.value = ""; decision.value = "all"; provenance.value = "all"; kind.value = "all"; changed(); search.focus();
    });
    this.#tools.append(field("Rechercher dans les événements chargés", search, "jr-search"), field("Décision", decision, "jr-decision"),
      field("Provenance", provenance, "jr-provenance"), field("Type de ressource", kind, "jr-kind"), el("div", "sv-tool-actions", reset));
  }
  /** One table for the life of the view: only its rows change. */
  #buildTable() {
    this.#table.setAttribute("role", "table");
    const head = el("thead", "", el("tr", "", ...["Reçu par le contrôle", "Type et source", "Ressource", "Provenance et couverture", "Décision", "Action"].map((title) => { const th = el("th", "", title); th.scope = "col"; th.setAttribute("role", "columnheader"); return th; })));
    head.setAttribute("role", "rowgroup"); this.#rows.setAttribute("role", "rowgroup"); head.firstElementChild?.setAttribute("role", "row");
    this.#table.append(this.#caption, head, this.#rows);
  }
  /** Retries an older page if it failed temporarily, or reads the newest events. One read at a time. */
  #readAgain(retry = false): Promise<void> {
    if (this.#store.state.busy) return Promise.resolve();
    this.#retrying = retry;
    const state = this.#store.state;
    const read = retry && !state.error && state.olderError?.retry === "same_page" ? this.#store.loadOlder() : this.#store.reload();
    this.#sayAfter(read); return read;
  }
  #limitTo(resource: ResourceRef | null) {
    this.#preferences.resource = resource; this.#preferences.rendered = JOURNAL_RENDER_STEP;
    this.#sayAfter(this.#store.setResource(resource)); this.#summary.focus({ preventScroll: true });
  }
  /** Renders one more step of loaded events, without any read, and continues on the first of them. */
  #showMore() {
    const previous = this.#preferences.rendered; this.#preferences.rendered += JOURNAL_RENDER_STEP; this.#renderList();
    const row = this.#rows.children[previous] as HTMLElement | undefined;
    (row?.querySelector<HTMLElement>("[data-focus-key]") ?? row ?? this.#summary).focus();
  }
  #render() {
    if (this.#closed) return;
    const active = this.element.contains(document.activeElement) ? document.activeElement as HTMLElement : null; const key = active?.dataset.focusKey;
    const footer = active?.closest<HTMLElement>(".sv-footer") ?? null;
    this.#renderStatus(); this.#renderList();
    if (active && !active.isConnected && !this.element.contains(document.activeElement)) {
      const replacement = key ? [...this.element.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === key && !node.matches(":disabled")) : null;
      if (replacement) { replacement.focus({ preventScroll: true }); return; }
      // A footer control with nothing more to read is replaced by a note, where the person was; else the result summary.
      ((footer?.isConnected ? footer.querySelector<HTMLElement>(".sv-end") : null) ?? this.#summary).focus();
    }
  }
  #say(text: string) { announce(this.#announce, text); }
  #readError(state: JournalState) {
    return state.error?.message ?? (state.olderError ? `Page plus ancienne indisponible. ${state.olderError.message}` : null);
  }
  #result(state: JournalState) { return `${shownOfLoaded("events", filterEvents(state.items, this.#preferences.filter).length, state.items.length)}.`; }
  /** Says how many loaded events match, once the person stopped typing or choosing. Never called by a read or a live batch. */
  #announceResult() {
    clearTimeout(this.#resultTimer);
    this.#resultTimer = setTimeout(() => {
      const state = this.#store.state;
      if (this.#closed || state.phase !== "ready") return;
      this.#resultDue = false; this.#say(this.#result(state));
    }, FILTER_ANNOUNCE_MS);
  }
  /**
   * A read the person asked for (an older page, a limit to one resource or its removal, a reload) says its
   * outcome once, when it ends: its result when it succeeded, else its failure. A failure is told by the
   * state when it changes and by its error line when that line appears; a failure that leaves both as
   * they were says its error again. Never called for a live batch, nor for a read the view starts by itself.
   */
  #sayAfter(read: Promise<void>) {
    const asked = ++this.#asked; this.#resultDue = false; this.#stateSpoken = false; this.#alerted = false;
    void read.then(() => {
      const state = this.#store.state;
      // Only the latest read asked speaks, and only once the window it produced is the one displayed.
      if (this.#closed || asked !== this.#asked || state.busy) return;
      this.#resultDue = true; this.#announceState(state);
    });
  }
  /**
   * Says the state when it changed and, in the same sentence, the result of a read the person asked for.
   * Nothing is said while a read or the connection of the direct is in flight: a reload reopens the direct,
   * and a result written before its state would be replaced by it at once.
   */
  #announceState(state: JournalState) {
    const sentence = journalAnnouncement(state);
    if (sentence === null) return;
    const parts: string[] = [];
    if (sentence !== this.#stateSaid) { this.#stateSaid = sentence; this.#stateSpoken = true; parts.push(sentence); }
    if (this.#resultDue) {
      this.#resultDue = false; clearTimeout(this.#resultTimer);
      const error = this.#readError(state);
      if (state.phase === "ready" && !error) parts.push(this.#result(state));
      // The read failed and neither a new state nor a new alert told it: the error still displayed is said again.
      else if (!this.#stateSpoken && !this.#alerted) parts.push(error ?? sentence);
    }
    if (parts.length) this.#say(parts.join(" "));
  }
  #renderStatus() {
    const state = this.#store.state; const [live, tone] = liveLabel[state.live];
    const snapshot = state.streamMode === "snapshot_only";
    const positive = state.phase === "ready" && state.receipt !== null;
    setText(this.#reload, snapshot ? "Actualiser le Journal" : "Relire depuis les plus récents");
    setText(this.#directHelp, snapshot ?
      "Historique de métadonnées à l’heure de lecture. Actualiser relit les plus récents ; le retour dans l’onglet conserve une revalidation ponctuelle, sans flux ni actualisation périodique." :
      positive ? "Le direct part exactement après la page lue : il ne rejoue pas l’historique. Chaque page et chaque lot en direct laissent un reçu durable de lecture." :
        "Le mode de suivi est indiqué après une lecture réussie. Une lecture en échec ne signifie pas une absence d’activité.");
    setText(this.#coverageHelp, snapshot ? "Métadonnées uniquement pour ce profil : contenu, détail d’événement et export indisponibles, même si les droits correspondants sont accordés." :
      positive ? "Pour le contenu ou l’export d’une ressource précise, utilisez l’onglet Événements depuis Ressources." :
        "La lecture précisera le profil disponible pour ce contexte. Les permissions restent vérifiées par le contrôle.");
    setText(this.#caption, snapshot || !positive ? "Les plus récents en tête · historique à l’heure de lecture" :
      "Les plus récents en tête · les nouveaux événements s’ajoutent en haut de la liste");
    const read = state.phase === "loading" ? "Lecture en cours…" : state.phase === "forbidden" ? "Lecture refusée" : state.phase === "unavailable" ? "Lecture récente indisponible" :
      state.phase === "refused" ? "Ressource non accordée" : state.phase === "error" ? state.error?.kind === "unreachable" ? "Contrôle injoignable" : "Données indisponibles" :
      state.phase === "idle" ? "Lecture requise" : state.stale ? "Données périmées · dernière lecture conservée" : state.busy ? "Lecture en cours…" :
      state.olderError ? "Page plus ancienne indisponible" : "Lecture réussie";
    const detail = state.live === "live" ? `${state.received} événement(s) reçu(s) depuis la dernière lecture` :
      state.live === "refused" || state.live === "off" || state.live === "unavailable" ? state.liveMessage :
      state.live === "retrying" ? `${state.liveMessage ? `${state.liveMessage} ` : ""}Les événements manquants seront repris à la reconnexion, sans doublon.` :
      state.live === "suspended" ? "Le retour sur l’onglet relit depuis les plus récents." : "";
    const readError = this.#readError(state);
    replaceIfChanged(this.#sourcesBox,
      el("span", "sv-source", el("strong", "", "Journal"), badge(read, readError || state.stale || state.phase === "forbidden" ? "amber" : "neutral"),
        el("span", "muted", state.loadedAt === null ? snapshot ? "Aucune lecture réussie conservée" : "Aucune lecture réussie" :
          `${snapshot ? "Dernière page lue le" : "Lu le"} ${formatDate(new Date(state.loadedAt).toISOString(), true)}`),
        snapshot && state.receipt ? el("span", "muted", `Lecture auditée le ${formatDate(state.receipt.received_at, true)} · reçu ${state.receipt.audit_event_id}`) : null),
      el("span", "sv-source", el("strong", "", "Direct"), badge(live, tone), el("span", "muted", detail)));
    // An error line is created when its message appears or changes, never again while it lasts: it is an alert.
    const error = state.phase === "forbidden" ?
      el("div", "control-error sv-error", el("p", "", "Consultation refusée par le contrôle. Les événements affichés ont été retirés. Relisez vos accès puis sélectionnez un contexte actuellement autorisé."),
        action("Relire mes accès", "jr-access", () => this.#host.onReviewAccess())) :
      readError ? el("div", "control-error sv-error", icon("alert"), el("p", "", readError),
        state.phase === "unavailable" || this.#host.reads.readBudgetStopped ? null :
          action(state.olderError?.retry === "from_newest" ? "Reprendre depuis les plus récents" : "Réessayer", "jr-retry", () => { void this.#readAgain(true); })) : null;
    error?.setAttribute("role", "alert");
    if (!state.busy) this.#retrying = false;
    // The store forgets an error when a read starts. The line « Réessayer » was activated from stays until
    // that read ends: the button keeps the focus, and the same failure is not created again as a new alert.
    if (!(this.#retrying && this.#errorsBox.querySelector('[data-focus-key="jr-retry"]'))) {
      const focused = this.#errorsBox.contains(document.activeElement) ? (document.activeElement as HTMLElement).dataset.focusKey : undefined;
      if (replaceIfChanged(this.#errorsBox, error)) {
        if (error) this.#alerted = true;
        if (focused) [...this.#errorsBox.querySelectorAll<HTMLElement>("[data-focus-key]")].find((node) => node.dataset.focusKey === focused)?.focus({ preventScroll: true });
      }
    }
    // The marks are set on the displayed nodes, outside the markup that decides whether the line is created again.
    this.#errorsBox.querySelector('[data-focus-key="jr-retry"]')?.setAttribute("aria-disabled", String(state.busy));
    this.#reload.setAttribute("aria-disabled", String(state.busy)); this.#reload.disabled = this.#host.reads.readBudgetStopped;
    this.#announceState(state);
    this.#announceCount(state);
  }
  /** Dense live traffic is announced as a count, at most once per period, never batch by batch. */
  #announceCount(state: JournalState) {
    if (state.received < this.#countSaid) this.#countSaid = state.received;
    if (state.live !== "live" || state.received <= this.#countSaid || this.#countTimer !== undefined) return;
    this.#countTimer = setTimeout(() => {
      this.#countTimer = undefined;
      const received = this.#store.state.received;
      if (this.#closed || this.#store.state.live !== "live" || received <= this.#countSaid) return;
      this.#countSaid = received; this.#countAt = Date.now();
      this.#say(`${received} événement${received > 1 ? "s reçus" : " reçu"} en direct depuis la dernière lecture.`);
    }, Math.max(0, LIVE_COUNT_ANNOUNCE_MS - (Date.now() - this.#countAt)));
  }
  #row(item: HumanEvent, limited: boolean) {
    const meta = item.metadata; const cell = (label: string, ...children: (Node | string | null)[]) => { const node = el("td", "", ...children); node.dataset.label = label; node.setAttribute("role", "cell"); return node; };
    const received = formatDate(meta.received_at, true); const stamped = formatDate(meta.timestamp, true);
    const policyLabel = journalPolicyEventLabel(meta);
    const typeSource = policyLabel
      ? el("p", "muted", `Registre des politiques · Source ${meta.source} · `, el("code", "mono", meta.kind))
      : el("p", "muted", `Source ${meta.source}`);
    const decision = meta.decision
      ? badge(decisionLabel[meta.decision], meta.decision === "allow" ? "teal" : meta.decision === "deny" ? "red" : "amber")
      : el("p", "muted", policyLabel ? "Aucune décision de connexion" : "Non fournie");
    // Compact controls with their full purpose as accessible name: a dense journal stays two lines per event.
    const compact = (label: string, purpose: string, key: string, call: () => void) => { const node = action(label, key, call); node.setAttribute("aria-label", purpose); node.title = purpose; return node; };
    const tr = el("tr", "", cell("Reçu le", el("p", "mono", received), stamped === received ? null : el("p", "muted", `Source : ${stamped}`)),
      cell("Type", el("p", "", el("strong", "", policyLabel ?? meta.kind)), typeSource),
      cell("Ressource", el("p", "", resourceKindLabel[meta.resource.kind]), el("p", "mono sv-id", meta.resource.resource_id)),
      cell("Provenance", el("p", "muted", `${meta.provenance} · couverture ${meta.coverage}${policyLabel ? " du registre" : ""}`)),
      cell("Décision", decision),
      cell("Action", el("div", "sv-row-actions",
        limited ? null : compact("Limiter", "Limiter à cette ressource", `jr-limit-${meta.id}`, () => this.#limitTo({ kind: meta.resource.kind, resource_id: meta.resource.resource_id })),
        meta.resource.kind === "session" && this.#host.openWorkers ? compact("Workers", "Voir les workers de cette session", `jr-workers-${meta.id}`, () => this.#host.openWorkers?.(meta.resource.resource_id)) : null)));
    tr.setAttribute("role", "row"); tr.dataset.eventId = meta.id; tr.tabIndex = -1; return tr;
  }
  #renderList() {
    const state = this.#store.state; const resource = state.resource; const limited = !!resource;
    const snapshot = state.streamMode === "snapshot_only";
    const olderNeedsReload = state.olderError?.retry === "from_newest";
    const canLoadOlder = state.hasOlder && !olderNeedsReload;
    replaceIfChanged(this.#limit, resource ? el("p", "", "Limité côté serveur à ", el("strong", "", `${resourceKindLabel[resource.kind]} `), el("span", "mono sv-id", resource.resource_id), ".") :
      el("p", "muted", "Toutes les ressources autorisées. « Limiter » relit le journal d’une seule ressource côté serveur."),
      resource ? action("Retirer la limite", "jr-unlimit", () => this.#limitTo(null)) : null);
    const filtered = filterEvents(state.items, this.#preferences.filter); const shown = filtered.slice(0, this.#preferences.rendered);
    setText(this.#summary, state.phase === "ready" ? `${shownOfLoaded("events", filtered.length, state.items.length)} · ${
      state.trimmed ? `fenêtre de ${JOURNAL_MAX_EVENTS} événements atteinte : relisez depuis les plus récents ou limitez à une ressource pour remonter plus loin` :
        olderNeedsReload ? "relisez depuis les plus récents pour poursuivre la lecture de l’historique" :
          state.hasOlder ? "des événements plus anciens existent au contrôle" : "début de la sélection autorisée atteint"}${shown.length < filtered.length ? ` · ${shown.length} lignes rendues` : ""}` :
      "Aucun événement chargé : contenu de la sélection inconnu.");
    // An event never changes: a row keeps its node, and a live batch only inserts the rows it brings.
    syncRows(this.#rows, state.phase === "ready" ? shown.map((item) => ({ key: item.metadata.id, signature: `${signatureOf(item)}\n${limited}`, build: () => this.#row(item, limited) })) : []);
    const empty = (title: string, description: string) => el("div", "empty-state sv-empty", el("h3", "", title), el("p", "", description));
    const notice = state.phase === "unavailable" ? empty("Lecture récente indisponible sur ce contrôle", "Le gateway ne sert pas encore cette lecture. Mettez à jour gateway et dashboard ensemble. D’ici là, l’onglet Événements lit une ressource choisie, du plus ancien au plus récent.") :
      state.phase === "forbidden" ? empty("Journal retiré", "La lecture a été refusée par le contrôle.") :
      state.phase === "refused" ? empty("Ressource non accordée", "Aucune ressource autorisée de ce contexte ne correspond à cette limite. Retirez la limite pour revenir à toute la sélection.") :
      state.phase === "error" ? empty("Événements inconnus", "La lecture a échoué. Une erreur n’est pas une absence d’activité.") :
      state.phase !== "ready" ? empty("Lecture des événements…", "La lecture doit réussir avant de conclure.") :
      !state.items.length ? empty("Aucun événement autorisé à cette lecture", snapshot ?
        "Aucune trace reçue pour cette sélection à cette lecture. Actualisez explicitement pour relire l’historique." :
        "Aucune trace reçue pour cette sélection autorisée. Les événements à venir apparaîtront ici tant que le direct est connecté.") :
      !filtered.length ? empty("Aucun événement chargé ne correspond", canLoadOlder ? "Des événements plus anciens existent au contrôle : élargissez les filtres ou chargez-en davantage." : "Modifiez la recherche ou les filtres.") : null;
    replaceIfChanged(this.#empty, notice);
    let reveal = false;
    if (state.phase === "ready") {
      setText(this.#renderMore, `Afficher ${Math.min(JOURNAL_RENDER_STEP, filtered.length - shown.length)} lignes de plus`);
      reveal = this.#older.getAttribute("aria-disabled") === "true" && !state.busy && document.activeElement === this.#older;
      this.#older.setAttribute("aria-disabled", String(state.busy)); this.#older.disabled = this.#host.reads.readBudgetStopped;
      place(this.#footer, shown.length < filtered.length ? this.#renderMore : null, canLoadOlder ? this.#older : null, !state.hasOlder && !state.trimmed ? this.#end : null);
    }
    place(this.#body, notice ? this.#empty : this.#table, state.phase === "ready" ? this.#footer : null);
    // The events it loaded pushed the control down: it kept the focus, so it stays in sight for the next page.
    if (reveal && this.#older.isConnected) this.#older.scrollIntoView({ block: "nearest" });
  }
}
