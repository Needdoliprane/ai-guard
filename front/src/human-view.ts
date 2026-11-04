import { button, el, icon } from "./dom.ts";
import { formatDate } from "./model.ts";
import { HumanApiClient, HumanApiError, HumanSuperseded } from "./human-api.ts";
import { HumanContractError } from "./human-model.ts";
import "./human.css";
import type { HumanBroker, HumanBrokerPage, HumanContext, HumanEvent, HumanEventPage, HumanIdentity, HumanPage, HumanResource, HumanView } from "./human-model.ts";

interface ReadState<T> {
  value: T | null; loading: boolean; error: string; loadedAt: string | null; stale: boolean;
  cursor: string | null; history: (string | null)[];
}
interface HumanChrome { brand: () => HTMLElement; themeButton: () => HTMLButtonElement }
const readState = <T>(): ReadState<T> => ({ value: null, loading: false, error: "", loadedAt: null, stale: false, cursor: null, history: [] });
const modeName = (context: HumanContext) => context.mode === "administration" ? "Administration" : "Personnel";
const kindName: Record<HumanResource["resource"]["kind"], string> = { session: "Session", sandbox: "Sandbox", agent: "Agent", project: "Projet" };
const brokerStateName: Record<HumanBroker["state"], string> = { unseen: "Heartbeat non reçu", current: "Bail courant", stale: "Bail périmé", expired: "Autorisation expirée", revoked: "Autorisation révoquée", clock_uncertain: "Horloge incertaine" };
type DashboardMode = "contexts" | "resources" | "events" | "brokers";
function badge(label: string, tone = "neutral") { return el("span", `badge badge-${tone}`, label); }
function action(label: string, key: string, call: () => void, disabled = false) {
  const node = button(label, "button button-secondary", call); node.dataset.focusKey = key; node.disabled = disabled; return node;
}
function failure(error: unknown) {
  return error instanceof HumanApiError ? `${error.message}${error.retryAction === "contact_operator" ? " Action requise : contactez l’opérateur." : ""}` :
    error instanceof HumanContractError ? error.message :
    error instanceof Error && /^(Export arrêté|Réponse trop volumineuse)/.test(error.message) ? error.message :
      "Lecture impossible. Vérifiez la connexion au contrôle puis réessayez.";
}
function empty(title: string, description: string) {
  return el("div", "empty-state", el("h2", "", title), el("p", "", description));
}

/** Human-only UI. No legacy tenant read, runner authority, or locally invented administration mode. */
export class HumanDashboard {
  #api: HumanApiClient;
  #root: HTMLElement;
  #chrome: HumanChrome;
  #onLogout: (message: string) => void;
  #identity: HumanIdentity | null = null;
  #context: HumanContext | null = null;
  #resources = readState<HumanPage<HumanResource>>();
  #events = readState<HumanEventPage>();
  #brokers = readState<HumanBrokerPage>();
  #brokerAbort = new AbortController();
  #brokerGeneration = 0;
  #brokerForbidden = false;
  #brokerLastLaunch = 0;
  #selection: { item: HumanResource; view: HumanView } | null = null;
  #mode: DashboardMode = "contexts";
  #generation = 0;
  #selectionGeneration = 0;
  #invalidation = 0;
  #scope = new AbortController();
  #selectionAbort = new AbortController();
  #streamAbort = new AbortController();
  #exportAbort = new AbortController();
  #detailAbort = new AbortController();
  #streamStatus = "Flux arrêté";
  #streamError = "";
  #message = "";
  #identityError = "";
  #identityLoading = false;
  #exporting = false;
  #exportMessage = "";
  #exportPages = 0;
  #exportBytes = 0;
  #downloadUrl: string | null = null;
  #dialog: HTMLDialogElement | null = null;
  #detailGeneration = 0;
  #page: HTMLElement | null = null;
  #notice: HTMLElement | null = null;
  #scopeLabel: HTMLElement | null = null;
  #streamLabel: HTMLElement | null = null;
  #navigation: HTMLElement | null = null;
  #poll: ReturnType<typeof setInterval> | undefined;
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  #closed = false;
  #mounted = false;
  #visibility = () => {
    if (document.visibilityState !== "visible") { this.#streamAbort.abort(); return; }
    if (this.#closed || this.#api.readBudgetStopped || this.#mode === "brokers" && this.#brokerForbidden) return;
    const generation = this.#generation;
    void this.#loadIdentity().then((success) => {
      if (!success || this.#closed || generation !== this.#generation || !this.#context) return;
      if (["resources", "events"].includes(this.#mode)) this.#startStream();
      this.#refreshActive(true);
    });
  };
  constructor(root: HTMLElement, token: string, chrome: HumanChrome, onLogout: (message: string) => void) {
    this.#root = root; this.#chrome = chrome; this.#onLogout = onLogout;
    this.#api = new HumanApiClient(token, {
      onAuthentication: (error) => { if (this.#mounted && !this.#closed) this.logout(error.message); },
      onContextInvalidated: (error) => {
        if (!this.#mounted || this.#closed) return;
        const readingIdentity = this.#identityLoading;
        this.#purgeContext(); this.#message = error.message; this.#mode = "contexts"; this.#render();
        if (!readingIdentity && !this.#api.readBudgetStopped) void this.#loadIdentity();
      },
      onReadBudgetExhausted: () => {
        this.#streamAbort.abort(); this.#exportAbort.abort(); clearTimeout(this.#refreshTimer); this.#refreshTimer = undefined;
        this.#resources.stale = true; this.#events.stale = true; this.#brokers.stale = true;
        this.#streamStatus = "Lecture automatique suspendue"; this.#render();
      },
    });
  }
  async start() {
    this.#identity = await this.#api.identity();
    if (this.#closed) throw new HumanSuperseded();
    this.#mounted = true; this.#shell(); this.#render();
    this.#poll = setInterval(() => this.#refreshActive(true), 30000);
    document.addEventListener("visibilitychange", this.#visibility);
  }
  close() {
    this.#closed = true; this.#purgeContext(); this.#api.close(); this.#identity = null;
    clearInterval(this.#poll); this.#page?.replaceChildren(); if (this.#mounted) this.#root.replaceChildren();
    document.removeEventListener("visibilitychange", this.#visibility);
  }
  logout(message?: string) {
    const detail = message ?? (this.#identity?.authentication.method === "oidc_access_token" ?
      "Session OIDC locale fermée." : "Session personnelle fermée. Le credential local reste administré par le contrôle.");
    this.close(); this.#onLogout(detail);
  }
  #purgeContext() {
    this.#generation++; this.#selectionGeneration++; this.#invalidation++;
    this.#scope.abort(); this.#scope = new AbortController();
    this.#selectionAbort.abort(); this.#selectionAbort = new AbortController();
    this.#streamAbort.abort(); this.#exportAbort.abort(); this.#detailAbort.abort();
    clearTimeout(this.#refreshTimer); this.#refreshTimer = undefined;
    this.#context = null; this.#selection = null; this.#resources = readState(); this.#events = readState();
    this.#purgeBrokers(); this.#brokerForbidden = false; this.#brokerLastLaunch = 0;
    if (this.#identity) this.#identity = { ...this.#identity, contexts: [] };
    this.#identityLoading = false;
    this.#exporting = false; this.#exportMessage = ""; this.#exportPages = 0; this.#exportBytes = 0;
    this.#streamStatus = "Flux arrêté"; this.#streamError = "";
    this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
    if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl); this.#downloadUrl = null;
  }
  #canResources() {
    return !!this.#context && (this.#context.capabilities.includes("read_metadata") ||
      this.#context.mode === "administration" && this.#context.capabilities.includes("read_tenant_metadata"));
  }
  #canBrokers() { return this.#context?.mode === "administration" && this.#context.capabilities.includes("read_broker_registry"); }
  #purgeBrokers() {
    this.#brokerGeneration++; this.#brokerAbort.abort(); this.#brokerAbort = new AbortController(); this.#brokers = readState();
    if (this.#mode === "brokers") { this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null; }
  }
  #stopViewReads() {
    this.#generation++; this.#selectionGeneration++; this.#scope.abort(); this.#scope = new AbortController();
    this.#selectionAbort.abort(); this.#selectionAbort = new AbortController(); this.#streamAbort.abort();
    this.#exportAbort.abort(); this.#detailAbort.abort(); this.#detailGeneration++;
    this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
    clearTimeout(this.#refreshTimer); this.#refreshTimer = undefined;
    if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl); this.#downloadUrl = null;
    this.#exporting = false; this.#exportMessage = ""; this.#exportPages = 0; this.#exportBytes = 0;
  }
  #openView(mode: DashboardMode) {
    if (mode === "contexts") {
      this.#api.clearContext(); this.#purgeContext(); this.#mode = mode;
      if (!this.#api.readBudgetStopped) void this.#loadIdentity();
    } else if (mode === "brokers" && this.#canBrokers()) {
      if (this.#mode !== mode) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#mode = mode;
      }
      this.#streamStatus = "Registre · sans flux temps réel"; this.#refreshActive(false);
    } else if (mode === "resources" && this.#canResources()) {
      this.#stopViewReads(); this.#purgeBrokers(); this.#mode = mode; this.#resources = readState();
      this.#events = readState(); this.#selection = null;
      this.#startStream(); this.#refreshActive(false);
    } else if (mode === "events" && this.#selection && this.#canResources()) {
      this.#mode = mode; this.#startStream(); this.#refreshActive(false);
    }
    this.#render(); this.#page?.querySelector<HTMLElement>("h1")?.focus({ preventScroll: true });
  }
  #refreshActive(automatic: boolean) {
    if (this.#closed || this.#api.readBudgetStopped || automatic && document.visibilityState !== "visible") return;
    if (this.#mode === "brokers" && this.#canBrokers() && !this.#brokerForbidden) void this.#loadBrokers(undefined, undefined, automatic);
    else if (this.#mode === "resources" && this.#canResources()) void this.#loadResources();
    else if (this.#mode === "events" && this.#selection && this.#canResources()) void this.#loadEvents();
  }
  #shell() {
    const logout = action("Se déconnecter", "human-logout", () => this.logout());
    logout.classList.add("header-logout"); logout.replaceChildren(icon("logout"), el("span", "", "Se déconnecter")); logout.setAttribute("aria-label", "Se déconnecter");
    this.#scopeLabel = el("strong", "authenticated-scope", this.#identity?.display_name);
    this.#streamLabel = el("span", "stream-pill", "Aucun contexte sélectionné");
    this.#navigation = el("nav", "human-toolbar"); this.#navigation.setAttribute("aria-label", "Espace personnel");
    this.#notice = el("div", "notice"); this.#notice.setAttribute("role", "alert");
    this.#page = el("main", "page human-page"); this.#page.id = "human-content";
    const skip = el("a", "skip-link", "Aller au contenu"); skip.href = "#human-content";
    this.#root.replaceChildren(skip, el("div", "app-shell human-shell",
      el("header", "topbar", el("div", "header-inner", this.#chrome.brand(),
        el("div", "header-scope", this.#scopeLabel, this.#streamLabel),
        el("div", "topbar-actions", this.#chrome.themeButton(), logout))),
      this.#navigation, el("div", "main-shell", this.#notice, this.#page)));
  }
  #render() {
    if (this.#closed || !this.#page || !this.#notice || !this.#navigation) return;
    const focusScope = this.#navigation.contains(document.activeElement) ? this.#navigation :
      this.#page.contains(document.activeElement) ? this.#page : null;
    const active = focusScope ? document.activeElement as HTMLElement : null;
    const key = active?.dataset.focusKey;
    this.#scopeLabel!.textContent = `${this.#identity?.display_name ?? "Compte personnel"}${this.#context ? ` · ${modeName(this.#context)} · ${this.#context.tenant_id}` : ""}`;
    this.#streamLabel!.textContent = this.#context ? this.#streamStatus : "Aucun contexte sélectionné";
    this.#notice.hidden = !this.#message; this.#notice.textContent = this.#message;
    const destinations: DashboardMode[] = ["contexts", "resources", "events", ...(this.#canBrokers() ? ["brokers" as const] : [])];
    const nav = destinations.map((mode) => {
      const label = { contexts: "Contextes", resources: "Ressources", events: "Événements", brokers: "Brokers" }[mode];
      const node = action(label, `human-nav-${mode}`, () => this.#openView(mode),
        mode === "resources" && !this.#canResources() || mode === "events" && (!this.#selection || !this.#canResources()));
      node.className = "button human-tab"; if (mode === this.#mode) node.setAttribute("aria-current", "page"); return node;
    });
    this.#navigation.replaceChildren(el("div", "human-nav", ...nav), action("Actualiser", "human-refresh", () => {
      if (this.#mode === "contexts") void this.#loadIdentity(); else this.#refreshActive(false);
    }, this.#identityLoading || this.#api.readBudgetStopped || this.#mode === "brokers" && (this.#brokers.loading || this.#brokerForbidden)));
    const heading = el("h1", "", { contexts: "Choisir un contexte.", resources: "Ressources autorisées.", events: "Événements autorisés.", brokers: "Brokers enregistrés du tenant." }[this.#mode]);
    heading.tabIndex = -1;
    this.#page.replaceChildren(el("div", "page-heading", el("p", "eyebrow", this.#identity?.authentication.method === "oidc_access_token" ?
      "COMPTE OIDC / CONSULTATION NOMINATIVE" : "COMPTE LOCAL / CONSULTATION NOMINATIVE"), heading,
      el("p", "page-description", this.#mode === "brokers" ? "Infrastructure de tout le tenant · autorisation read_broker_registry. Les permissions et l’évaluation du registre proviennent du contrôle." :
        "Les permissions sont vérifiées par le serveur. Une ressource visible ne donne aucun droit implicite sur son contenu ou son export.")),
      ...(this.#api.readBudgetStopped ? [el("div", "control-error human-budget-stop", el("p", "", "Lecture automatique suspendue : budget durable de lecture atteint. Contactez l’opérateur ; changer de vue ou de contexte ne réinitialise pas ce budget."),
        action("Relire après intervention de l’opérateur", "human-operator-retry", () => {
          this.#api.allowReadAfterOperatorAction();
          void this.#refreshAfterOperatorAction();
        }, this.#identityLoading || this.#brokers.loading || this.#resources.loading || this.#events.loading))] : []),
      this.#context ? el("div", "info-strip human-context-summary", icon("shield"), el("div", "",
        el("strong", "", `${modeName(this.#context)} · ${this.#context.tenant_id}`),
        el("p", "", `Organisation : ${this.#context.organization_scope}`),
        el("p", "", `Contexte valable jusqu’au ${formatDate(this.#context.expires_at, true)}. Source : identité authentifiée du contrôle.`))) : "",
      this.#mode === "contexts" ? this.#renderContexts() : this.#mode === "resources" ? this.#renderResources() : this.#mode === "brokers" ? this.#renderBrokers() : this.#renderEvents());
    if (active && focusScope) {
      const replacement = key ? [...focusScope.querySelectorAll<HTMLElement>("[data-focus-key]")]
        .find((node) => node.dataset.focusKey === key && node.isConnected && !node.matches(":disabled") &&
          !node.closest("[hidden], [inert]") && node.getClientRects().length > 0) : null;
      const fallback = focusScope === this.#navigation ?
        this.#navigation.querySelector<HTMLElement>('[aria-current="page"]:not(:disabled)') : null;
      (replacement ?? fallback ?? heading).focus({ preventScroll: true });
    }
  }
  async #refreshAfterOperatorAction() {
    if (this.#mode === "brokers" && this.#canBrokers() && !this.#brokerForbidden) {
      await this.#loadBrokers(undefined, undefined, false, true); return;
    }
    const generation = this.#generation; const mode = this.#mode;
    const success = mode === "resources" && this.#canResources() ? await this.#loadResources(undefined, undefined, true) :
      mode === "events" && this.#selection && this.#canResources() ? await this.#loadEvents(undefined, undefined, true) : null;
    if (success && !this.#closed && generation === this.#generation && mode === this.#mode && !this.#api.readBudgetStopped && this.#canResources()) {
      this.#startStream(); return;
    }
    if (success !== null || this.#closed) return;
    // A refused destination must regain a current offered context before another page read.
    this.#api.clearContext(); this.#purgeContext(); this.#mode = "contexts"; this.#render();
    await this.#loadIdentity(true);
  }
  async #loadIdentity(operatorRetry = false) {
    if (this.#closed || this.#identityLoading || this.#api.readBudgetStopped && !operatorRetry) return;
    this.#identityLoading = true; this.#render(); const generation = this.#generation;
    try {
      const identity = await this.#api.identity();
      if (this.#closed || generation !== this.#generation && this.#context) return;
      this.#identity = identity; this.#identityError = "";
      return true;
    } catch (error) {
      if (!this.#closed && generation === this.#generation && !(error instanceof HumanSuperseded)) {
        this.#identityError = failure(error); this.#resources.stale = true; this.#events.stale = true; this.#brokers.stale = true;
        this.#message = this.#context ? `Revalidation des accès impossible. ${failure(error)}` : this.#message;
      }
      return false;
    }
    finally { if (!this.#closed && (generation === this.#generation || !this.#context)) { this.#identityLoading = false; this.#render(); } }
  }
  #renderContexts() {
    const contexts = this.#identity?.contexts ?? [];
    const partial = this.#identity?.contexts_complete === false;
    const coverage = partial ? el("div", "control-status",
      badge("Liste des contextes partielle", "amber"),
      el("p", "", "Certaines sources ne peuvent pas être consultées. D’autres contextes peuvent manquer. Réessayez pour actualiser la liste."),
      action("Actualiser les contextes", "human-contexts-retry", () => { void this.#loadIdentity(); }, this.#identityLoading)) : null;
    coverage?.setAttribute("role", "status");
    return el("section", "human-contexts",
      el("p", "muted", `Identité authentifiée : ${this.#identity?.display_name ?? "Inconnue"}. Les contextes personnel et administration sont proposés par le serveur selon vos appartenances.`),
      this.#identityLoading ? el("p", "control-status", "Lecture des accès…") : "",
      this.#identityError ? this.#error(this.#identityError, () => { void this.#loadIdentity(); }) : "",
      coverage,
      !this.#context && !contexts.length && this.#identity?.contexts_complete === true && !this.#identityLoading && !this.#identityError ? empty("Aucun contexte autorisé", "Votre compte est authentifié mais aucun contexte actif n’est proposé. Faites vérifier vos appartenances par l’opérateur.") : "",
      ...contexts.map((context, index) => el("article", "panel human-context-card",
        el("h2", "", `${modeName(context)} · ${context.tenant_id}`), el("p", "muted", `Organisation : ${context.organization_scope}`),
        el("p", "muted", `Valable jusqu’au ${formatDate(context.expires_at, true)}`),
        el("p", "mono", `Capacités serveur : ${context.capabilities.join(", ") || "aucune"}`),
        action(`Ouvrir ${modeName(context).toLowerCase()} · ${context.tenant_id}`, `context-${index}`, () => {
          try {
            this.#purgeContext(); this.#context = this.#api.selectContext(context.id); this.#message = "";
            if (this.#canResources()) this.#openView("resources");
            else if (this.#canBrokers()) this.#openView("brokers");
            else { this.#mode = "contexts"; this.#message = "Ce contexte n’offre aucune vue de consultation disponible avec ses capacités. Faites vérifier vos accès."; this.#render(); }
          } catch (error) { this.#identityError = failure(error); this.#render(); }
        }, this.#identityLoading || Date.parse(context.expires_at) <= Date.now()))),
      el("p", "field-help", "La déconnexion efface ce jeton de l’onglet. Sa révocation et ses permissions restent contrôlées côté serveur."));
  }
  #error(message: string, retry: () => void) {
    const node = el("div", "control-error", icon("alert"), el("p", "", message), this.#api.readBudgetStopped ? null : action("Réessayer", "human-retry", retry));
    node.setAttribute("role", "alert"); return node;
  }
  #status<T extends { read_receipt: { received_at: string } }>(state: ReadState<T>) {
    const stale = state.stale || !!state.loadedAt && Date.now() - Date.parse(state.loadedAt) > 30000;
    const node = el("div", "control-status", badge(state.loading ? "Lecture en cours…" : state.error ? state.value ? "Données périmées · dernière lecture conservée" : "Données indisponibles" : stale ? "Données à actualiser" : state.value ? "Lecture réussie" : "Lecture requise", state.error || stale ? "amber" : "neutral"),
      el("span", "muted", state.loadedAt ? `Lu le ${formatDate(state.loadedAt, true)}` : "Aucune lecture réussie"),
      state.value ? el("span", "muted", `Reçu du contrôle : ${formatDate(state.value.read_receipt.received_at, true)}`) : null);
    node.setAttribute("role", "status"); return node;
  }
  #pagination<T extends { items: unknown[]; has_more: boolean; next_cursor: string | null }>(state: ReadState<T>, load: (cursor: string | null, history: (string | null)[]) => void) {
    const blocked = state.loading || this.#api.readBudgetStopped;
    return el("div", "table-footer control-pagination", el("span", "control-page-summary",
      state.value ? `${state.value.items.length} résultat(s) sur cette page · page ${state.history.length + 1}${state.value.has_more || state.history.length ? " · lecture partielle" : ""}` : "Sélection autorisée inconnue"),
      el("div", "control-page-actions",
        state.history.length || state.error ? action("Première page", "human-first", () => load(null, []), blocked) : null,
        state.history.length ? action("Page précédente", "human-previous", () => load(state.history.at(-1) ?? null, state.history.slice(0, -1)), blocked) : null,
        state.value?.next_cursor ? action("Page suivante", "human-next", () => load(state.value?.next_cursor ?? null, [...state.history, state.cursor]), blocked || state.history.length >= 100) :
          state.value ? el("span", "muted", "Fin de cette sélection à cette lecture") : null));
  }
  async #loadBrokers(cursor = this.#brokers.cursor, history = this.#brokers.history, automatic = false, operatorRetry = false) {
    if (!this.#canBrokers() || this.#mode !== "brokers" || this.#closed || this.#brokers.loading || this.#brokerForbidden ||
      this.#api.readBudgetStopped && !operatorRetry || automatic && (document.visibilityState !== "visible" || Date.now() - this.#brokerLastLaunch < 30000)) return;
    const state = this.#brokers; const generation = this.#generation; const request = this.#brokerGeneration;
    this.#brokerLastLaunch = Date.now(); state.loading = true; this.#render();
    try {
      const page = await this.#api.brokers(cursor, this.#brokerAbort.signal);
      if (this.#closed || generation !== this.#generation || request !== this.#brokerGeneration || this.#mode !== "brokers" || !this.#canBrokers()) return;
      state.value = page; state.cursor = cursor; state.history = [...history]; state.loadedAt = new Date().toISOString(); state.error = ""; state.stale = false;
    } catch (error) {
      if (this.#closed || generation !== this.#generation || request !== this.#brokerGeneration || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) {
        this.#purgeBrokers(); this.#brokerForbidden = true; this.#brokers.error = failure(error); this.#brokers.stale = true;
      } else { state.error = failure(error); state.stale = true; }
    } finally {
      state.loading = false;
      if (!this.#closed && generation === this.#generation && this.#mode === "brokers") this.#render();
    }
  }
  #brokerAuthorities(item: HumanBroker) {
    return el("ul", "human-broker-authorities", ...item.authorities.map((authority) => el("li", "",
      el("span", "mono", authority.authority_id_redacted ? "Identifiant masqué" : authority.authority_id),
      el("span", "muted", ` · ${authority.runtime_kind}`))));
  }
  #renderBrokers() {
    const state = this.#brokers;
    const table = el("table", "human-broker-table");
    const caption = el("caption", "", "Registre central du tenant · métadonnées enregistrées");
    const headers = ["Broker / incarnation", "État du bail", "Autorités déclarées", "Dernière réception observée", "Expiration", "Détails"].map((label) => {
      const cell = el("th", "", label); cell.scope = "col"; return cell;
    });
    table.append(caption, el("thead", "", el("tr", "", ...headers)), el("tbody", "", ...(state.value?.items ?? []).map((item, index) => {
      const heartbeat = item.last_heartbeat;
      return el("tr", "", el("td", "", el("p", "mono control-id", item.broker_id), el("p", "muted", "Incarnation courante"), el("p", "mono control-id", item.incarnation_id)),
        el("td", "", badge(brokerStateName[item.state], item.state === "current" ? "neutral" : "amber")),
        el("td", "", this.#brokerAuthorities(item)),
        el("td", "", heartbeat ? el("div", "", el("p", "", formatDate(heartbeat.received_at, true)), el("p", "muted", "Heartbeat déclaré · réception observée"),
          heartbeat.incarnation_id !== item.incarnation_id ? badge("Heartbeat d’une ancienne incarnation", "amber") : null) : el("p", "muted", "Aucun heartbeat reçu dans le registre")),
        el("td", "", el("p", "muted", `Autorisation : ${formatDate(item.not_after, true)}`),
          heartbeat ? el("p", "muted", `Bail déclaré : ${formatDate(heartbeat.lease_expires_at, true)}`) : null),
        el("td", "", action("Détails du broker", `broker-detail-${index}`, () => this.#showBroker(item))));
    })));
    const scroll = el("div", "human-broker-scroll", table); scroll.tabIndex = 0; scroll.setAttribute("role", "region"); scroll.setAttribute("aria-label", "Tableau des brokers enregistrés, défilement horizontal");
    return el("section", "panel human-broker-panel", el("div", "panel-heading", el("div", "", el("h2", "", "Registre des brokers"),
      el("p", "", "Source : registre central du tenant. Inventaire runtime inconnu ; aucun worker ni état de fonctionnement n’est déduit du bail."),
      el("p", "", "Lecture live paginée : les lignes peuvent changer entre deux pages. Le flux des ressources ne couvre pas ce registre."))),
      this.#status(state), state.value ? el("p", "human-broker-evaluation muted", `Évaluation serveur : ${formatDate(state.value.evaluated_at, true)} · source tenant_broker_registry · cohérence live_keyset.`) : null,
      this.#brokerForbidden ? el("div", "control-error", el("p", "", state.error), action("Relire mes accès", "human-broker-access", () => this.#openView("contexts"))) :
        state.error ? this.#error(state.error, () => { void this.#loadBrokers(); }) : null,
      !state.value ? empty(state.loading ? "Chargement du registre…" : "Registre indisponible", this.#brokerForbidden ? "L’accès a été refusé. Les données, détails et positions de lecture ont été retirés." :
        "Une lecture réussie est nécessaire pour connaître les brokers enregistrés. L’inventaire runtime reste inconnu.") :
        !state.value.items.length ? empty(state.history.length ? "Fin de cette traversée live" : "Aucun broker enregistré à cette lecture", "Cette page décrit le registre à l’heure d’évaluation. L’inventaire runtime reste inconnu.") : scroll,
      this.#brokerForbidden ? null : this.#pagination(state, (next, previous) => { void this.#loadBrokers(next, previous); }),
      el("p", "human-broker-footnote muted", "Actualisation automatique au plus toutes les 30 secondes lorsque cette vue est visible. Ce rythme ne garantit aucun délai de réponse, notification ou retrait des droits."));
  }
  #showBroker(item: HumanBroker) {
    const page = this.#brokers.value;
    if (!page || this.#mode !== "brokers" || !this.#canBrokers() || this.#brokerForbidden) return;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.#dialog?.close(); this.#dialog?.remove();
    const heading = el("h2", "", "Détails du broker"); heading.id = "human-broker-detail-title";
    const dialog = el("dialog", "detail-dialog human-broker-dialog"); dialog.setAttribute("aria-labelledby", heading.id);
    const close = button("", "icon-button", () => dialog.close(), "x"); close.setAttribute("aria-label", "Fermer les détails");
    const detail = (name: string, value: string | HTMLElement) => el("div", "", el("dt", "", name), el("dd", "", value));
    const h = item.last_heartbeat;
    dialog.append(el("div", "detail-header", heading, close), el("div", "detail-body",
      el("p", "muted", `Métadonnées de la page évaluée le ${formatDate(page.evaluated_at, true)}. Inventaire runtime inconnu.`),
      el("dl", "human-broker-details", detail("Broker", item.broker_id), detail("Incarnation courante", item.incarnation_id),
        detail("État du bail", brokerStateName[item.state]), detail("Révision de l’autorisation", item.grant_revision), detail("Révision du heartbeat", item.heartbeat_revision),
        detail("Autorisation accordée", formatDate(item.granted_at, true)), detail("Expiration de l’autorisation", formatDate(item.not_after, true)),
        detail("Révocation", item.revoked_at ? formatDate(item.revoked_at, true) : "Aucune révocation enregistrée"), detail("Autorités déclarées", this.#brokerAuthorities(item)),
        detail("Heartbeat déclaré", h ? `Séquence ${h.sequence} · incarnation ${h.incarnation_id}${h.incarnation_id !== item.incarnation_id ? " · ancienne incarnation" : ""}` : "Aucun heartbeat reçu"),
        detail("Réception observée", h ? formatDate(h.received_at, true) : "Inconnue"), detail("Expiration du bail déclaré", h ? formatDate(h.lease_expires_at, true) : "Inconnue"),
        detail("Demande de lecture", page.read_receipt.request_id), detail("Événement du reçu", page.read_receipt.audit_event_id),
        detail("Reçu durable du contrôle", formatDate(page.read_receipt.received_at, true))),
      el("p", "muted", "Le reçu atteste l’enregistrement de la lecture, sans prouver sa livraison ni une exécution runtime. Kubernetes reste une autorité déclarée.")));
    document.body.append(dialog); this.#dialog = dialog; dialog.showModal();
    dialog.addEventListener("close", () => {
      if (this.#dialog === dialog) this.#dialog = null; dialog.remove();
      if (this.#closed || document.querySelector("dialog[open]")) return;
      const key = trigger?.dataset.focusKey;
      const replacement = key ? [...this.#page?.querySelectorAll<HTMLElement>("[data-focus-key]") ?? []].find((node) => node.dataset.focusKey === key) : null;
      (trigger?.isConnected ? trigger : replacement ?? this.#page?.querySelector<HTMLElement>("h1"))?.focus({ preventScroll: true });
    }, { once: true });
  }
  async #loadResources(cursor = this.#resources.cursor, history = this.#resources.history, operatorRetry = false) {
    if (!this.#canResources() || this.#mode !== "resources" || this.#closed || this.#resources.loading || this.#api.readBudgetStopped && !operatorRetry) return;
    const state = this.#resources; const generation = this.#generation; const invalidation = this.#invalidation;
    state.loading = true; this.#render();
    try {
      const page = await this.#api.resources(cursor, this.#scope.signal);
      if (this.#closed || generation !== this.#generation) return;
      state.value = page; state.cursor = cursor; state.history = history; state.loadedAt = new Date().toISOString();
      state.error = ""; state.stale = invalidation !== this.#invalidation;
      const selected = this.#selection;
      const updated = selected && page.items.find((item) => item.resource.kind === selected.item.resource.kind && item.resource.resource_id === selected.item.resource.resource_id);
      if (updated && selected && (!updated.permissions.metadata || selected.view === "content" && !updated.permissions.content)) this.#denySelection("Les droits de cette ressource ont changé. Choisissez une consultation autorisée.");
      else if (updated && this.#selection) {
        this.#selection.item = updated;
        if (!updated.permissions.export) {
          this.#exportAbort.abort();
          if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl); this.#downloadUrl = null;
        }
      }
      return true;
    } catch (error) {
      if (this.#closed || generation !== this.#generation || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.code === "permission_denied") {
        state.value = null; if (this.#selection) this.#denySelection(error.message);
      }
      state.error = failure(error); state.stale = true;
    } finally { state.loading = false; if (!this.#closed && generation === this.#generation) { this.#render(); if (state.stale && !state.error) this.#schedule(); } }
  }
  #renderResources() {
    const state = this.#resources;
    return el("section", "panel human-resource-panel", el("div", "panel-heading", el("div", "", el("h2", "", "Ressources de ce contexte"),
      el("p", "", "Source : registre des droits explicites du contrôle. Une page n’est pas un total du tenant et le journal n’est pas un inventaire runtime."))),
      this.#status(state), state.error ? this.#error(state.error, () => { void this.#loadResources(); }) : "",
      this.#streamError ? this.#error(this.#streamError, () => this.#startStream()) : "",
      !state.value ? empty(state.loading ? "Chargement des ressources…" : "Ressources inconnues", "La lecture doit réussir avant de conclure sur ce contexte.") :
        !state.value.items.length ? empty("Aucune ressource sur cette page", "Aucune ressource explicitement autorisée dans cette sélection. Une ancienne trace seule n’accorde aucun accès.") :
          el("div", "human-resource-list", ...state.value.items.map((item, index) => el("article", "human-resource-row",
            el("div", "", el("h3", "", kindName[item.resource.kind]), el("p", "mono control-id", item.resource.resource_id),
              el("p", "muted", item.owner.kind === "human_local" ? `Propriétaire local : ${item.owner.account_id}` :
                `Propriétaire OIDC : ${item.owner.subject} · ${item.owner.issuer}`), el("p", "mono", `Révision ${item.revision}`)),
            el("div", "human-resource-actions",
              action("Métadonnées", `resource-metadata-${index}`, () => this.#select(item, "metadata"), !item.permissions.metadata),
              action("Contenu", `resource-content-${index}`, () => this.#select(item, "content"), !item.permissions.metadata || !item.permissions.content),
              badge(item.permissions.export ? "Export autorisé selon la projection" : "Export non autorisé"))))),
      this.#pagination(state, (next, previous) => { void this.#loadResources(next, previous); }));
  }
  #select(item: HumanResource, view: HumanView) {
    if (!this.#canResources() || this.#mode !== "resources" || !item.permissions.metadata || view === "content" && !item.permissions.content) return;
    this.#selectionGeneration++; this.#selectionAbort.abort(); this.#selectionAbort = new AbortController();
    this.#streamAbort.abort(); this.#exportAbort.abort(); this.#detailAbort.abort(); this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
    if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl); this.#downloadUrl = null;
    this.#events = readState(); this.#exporting = false; this.#exportMessage = "";
    this.#selection = { item, view }; this.#mode = "events"; this.#message = "";
    this.#render(); this.#startStream(); void this.#loadEvents();
  }
  #denySelection(message: string) {
    this.#selectionGeneration++; this.#selectionAbort.abort(); this.#selectionAbort = new AbortController();
    this.#streamAbort.abort(); this.#exportAbort.abort(); this.#detailAbort.abort(); this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
    if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl); this.#downloadUrl = null;
    this.#selection = null; this.#events = readState(); this.#exporting = false; this.#exportMessage = "";
    this.#message = message; this.#mode = this.#canResources() ? "resources" : "contexts"; this.#render(); this.#startStream();
  }
  async #loadEvents(cursor = this.#events.cursor, history = this.#events.history, operatorRetry = false) {
    const selection = this.#selection;
    if (!selection || this.#mode !== "events" || !this.#canResources() || this.#closed || this.#events.loading || this.#api.readBudgetStopped && !operatorRetry) return;
    const state = this.#events; const generation = this.#generation; const selected = this.#selectionGeneration; const invalidation = this.#invalidation;
    state.loading = true; this.#render();
    try {
      const page = await this.#api.events({ view: selection.view, resource: selection.item.resource, cursor }, this.#selectionAbort.signal);
      if (this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration) return;
      state.value = page; state.cursor = cursor; state.history = history; state.error = ""; state.loadedAt = new Date().toISOString(); state.stale = invalidation !== this.#invalidation;
      return true;
    } catch (error) {
      if (this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && ["permission_denied", "not_found"].includes(error.code)) {
        this.#denySelection(error.message); void this.#loadResources(); return;
      }
      state.error = failure(error); state.stale = true;
    } finally { state.loading = false; if (!this.#closed && generation === this.#generation && selected === this.#selectionGeneration) { this.#render(); if (state.stale && !state.error) this.#schedule(); } }
  }
  #renderEvents() {
    const selection = this.#selection; const state = this.#events;
    if (!selection) return empty("Sélection requise", "Choisissez une ressource autorisée.");
    return el("section", "panel human-event-panel", el("div", "panel-heading", el("div", "", el("h2", "", `${kindName[selection.item.resource.kind]} · ${selection.view === "metadata" ? "Métadonnées" : "Contenu autorisé"}`),
      el("p", "mono control-id", selection.item.resource.resource_id),
      el("p", "", "Portée : cette ressource, dans le contexte sélectionné. Les événements proviennent du journal autorisé ; aucune chaîne complète ni observation runtime n’est déduite."))),
      this.#status(state), this.#streamError ? this.#error(this.#streamError, () => this.#startStream()) : "",
      state.error ? this.#error(state.error, () => { void this.#loadEvents(); }) : "",
      !state.value ? empty(state.loading ? "Chargement des événements…" : "Événements inconnus", "Une lecture absente ou refusée ne constitue pas une absence d’activité.") :
        !state.value.items.length ? empty("Aucun événement sur cette page", "Aucune trace reçue pour cette sélection autorisée, à cette lecture.") :
          el("div", "human-event-list", ...state.value.items.map((item, index) => el("article", "human-event-row",
            el("div", "", el("h3", "", item.metadata.kind), el("p", "muted", formatDate(item.metadata.timestamp, true)),
              el("p", "muted", `Source : ${item.metadata.source} · Reçu le ${formatDate(item.metadata.received_at, true)}`),
              el("div", "detail-badges", badge(`Provenance : ${item.metadata.provenance}`), badge(`Couverture : ${item.metadata.coverage}`),
                item.metadata.decision ? badge(`Décision : ${item.metadata.decision}`) : badge("Décision non fournie"))),
            action("Détails", `human-event-${index}`, () => { void this.#showEvent(item); })))),
      this.#pagination(state, (next, previous) => { void this.#loadEvents(next, previous); }), this.#renderExport());
  }
  #startStream() {
    this.#streamAbort.abort(); this.#streamAbort = new AbortController(); this.#streamError = "";
    if (!this.#canResources() || !["resources", "events"].includes(this.#mode) || this.#closed || this.#api.readBudgetStopped || document.visibilityState !== "visible") return;
    const generation = this.#generation; const selected = this.#selectionGeneration;
    const mode = this.#mode;
    const signal = this.#streamAbort.signal;
    const selection = this.#selection ? { view: this.#selection.view, resource: this.#selection.item.resource } : { view: "metadata" as const };
    void this.#api.listen(selection, signal, () => {
      if (signal.aborted || this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || mode !== this.#mode || this.#api.readBudgetStopped) return;
      this.#invalidation++; this.#resources.stale = true; this.#events.stale = true; this.#render(); this.#schedule();
    }, (status) => {
      if (signal.aborted || this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || mode !== this.#mode || this.#api.readBudgetStopped) return;
      this.#streamStatus = ({ connecting: "Connexion au flux autorisé", live: "Flux autorisé connecté", retrying: "Flux déconnecté · reconnexion", closed: "Flux arrêté" })[status];
      if (status === "retrying") { this.#resources.stale = true; this.#events.stale = true; }
      this.#render();
    }).catch((error: unknown) => {
      if (signal.aborted || this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || error instanceof HumanSuperseded) return;
      this.#streamStatus = "Flux déconnecté · lectures ponctuelles seulement";
      if (this.#selection && error instanceof HumanApiError && ["permission_denied", "not_found"].includes(error.code)) {
        this.#denySelection(error.message); void this.#loadResources(); return;
      }
      this.#streamError = failure(error); this.#resources.stale = true; this.#events.stale = true; this.#render();
    });
  }
  #schedule() {
    if (this.#refreshTimer !== undefined || this.#closed || this.#api.readBudgetStopped || !this.#canResources() || !["resources", "events"].includes(this.#mode)) return;
    const generation = this.#generation; const mode = this.#mode;
    this.#refreshTimer = setTimeout(() => {
      this.#refreshTimer = undefined; if (generation !== this.#generation || mode !== this.#mode) return;
      this.#refreshActive(true);
    }, 1000);
  }
  async #showEvent(item: HumanEvent) {
    const selection = this.#selection; if (!selection || this.#mode !== "events" || !this.#canResources() || this.#api.readBudgetStopped) return;
    this.#detailAbort.abort(); this.#detailAbort = new AbortController(); const request = ++this.#detailGeneration;
    this.#dialog?.remove();
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = el("dialog", "detail-dialog"); dialog.setAttribute("aria-labelledby", "human-detail-title");
    const heading = el("h2", "", item.metadata.kind); heading.id = "human-detail-title";
    const close = button("", "icon-button", () => dialog.close(), "x"); close.setAttribute("aria-label", "Fermer les détails");
    const body = el("div", "detail-body", empty("Lecture du détail…", "Vérification du contexte et des permissions côté serveur."));
    dialog.append(el("div", "detail-header", heading, close), body); document.body.append(dialog); this.#dialog = dialog; dialog.showModal();
    dialog.addEventListener("close", () => {
      if (this.#dialog === dialog) { this.#detailAbort.abort(); this.#detailGeneration++; this.#dialog = null; }
      dialog.remove();
      if (this.#closed || document.querySelector("dialog[open]")) return;
      const key = trigger?.dataset.focusKey;
      const replacement = key ? [...this.#page?.querySelectorAll<HTMLElement>("[data-focus-key]") ?? []].find((node) => node.dataset.focusKey === key) : null;
      (trigger?.isConnected ? trigger : replacement ?? this.#page?.querySelector<HTMLElement>("h1"))?.focus({ preventScroll: true });
    }, { once: true });
    const generation = this.#generation; const selected = this.#selectionGeneration;
    try {
      const page = await this.#api.event(item.metadata.id, selection.view, this.#detailAbort.signal);
      if (this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || request !== this.#detailGeneration || !dialog.open) return;
      const received = page.items[0]!;
      const data = el("pre", "code-block"); data.textContent = JSON.stringify(received, null, 2);
      body.replaceChildren(el("p", "muted", `${selection.view === "metadata" ? "Métadonnées uniquement" : "Contenu autorisé"} · Reçu du contrôle le ${formatDate(page.read_receipt.received_at, true)}. Ce reçu atteste l’enregistrement de la lecture, pas une chaîne d’audit complète.`), data);
    } catch (error) {
      if (this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || request !== this.#detailGeneration || !dialog.open || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && ["permission_denied", "not_found"].includes(error.code)) { this.#denySelection(error.message); void this.#loadResources(); return; }
      body.replaceChildren(this.#error(failure(error), () => { void this.#showEvent(item); }));
    }
  }
  #renderExport() {
    const permitted = !!this.#selection?.item.permissions.export;
    const status = el("p", "human-export-status", this.#exporting ? `${this.#exportPages} page(s) reçue(s), ${this.#exportBytes} octets · export encore partiel, aucun téléchargement.` : this.#exportMessage);
    status.setAttribute("role", "status");
    return el("section", "human-export", el("h3", "", "Exporter cette sélection"),
      el("p", "muted", "JSON · ressource et projection affichées ci-dessus. Limites de cet onglet : 25 pages, 4 Mio, 60 secondes. Le fichier est proposé seulement à la fin de la sélection autorisée, avec ses reçus de lecture."),
      action(`Exporter ${this.#selection?.view === "content" ? "le contenu" : "les métadonnées"}`, "human-export", () => { void this.#export(); }, !permitted || this.#exporting),
      this.#exporting ? action("Annuler l’export", "human-export-cancel", () => this.#exportAbort.abort()) : null,
      !permitted ? el("p", "muted", "Le contrôle n’accorde pas le droit d’export pour cette ressource.") : null, status);
  }
  async #export() {
    const selection = this.#selection; if (!selection || this.#mode !== "events" || !this.#canResources() || !selection.item.permissions.export || this.#exporting || this.#api.readBudgetStopped) return;
    this.#exportAbort.abort(); this.#exportAbort = new AbortController(); const signal = this.#exportAbort.signal;
    const generation = this.#generation; const selected = this.#selectionGeneration;
    this.#exporting = true; this.#exportPages = 0; this.#exportBytes = 0; this.#exportMessage = ""; this.#render();
    try {
      const blob = await this.#api.exportSelection({ view: selection.view, resource: selection.item.resource }, signal, (pages, bytes) => {
        if (this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration) return;
        this.#exportPages = pages; this.#exportBytes = bytes; this.#render();
      });
      if (this.#closed || signal.aborted || generation !== this.#generation || selected !== this.#selectionGeneration) return;
      if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl);
      const url = URL.createObjectURL(blob); this.#downloadUrl = url;
      const name = selection.item.resource.resource_id.replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 48) || "selection";
      const link = el("a"); link.href = url; link.download = `ai-guard-${selection.item.resource.kind}-${name}-${selection.view}-${new Date().toISOString().slice(0, 10)}.json`;
      document.body.append(link); link.click(); link.remove();
      setTimeout(() => { URL.revokeObjectURL(url); if (this.#downloadUrl === url) this.#downloadUrl = null; }, 1000);
      this.#exportMessage = "Sélection autorisée entièrement reçue ; téléchargement proposé. Aucun total de tenant n’est déduit.";
    } catch (error) {
      if (this.#closed || generation !== this.#generation || selected !== this.#selectionGeneration || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && ["permission_denied", "not_found"].includes(error.code)) { this.#denySelection(error.message); void this.#loadResources(); return; }
      this.#exportMessage = signal.aborted ? "Export annulé. Aucun fichier partiel n’a été téléchargé." :
        error instanceof DOMException && error.name === "TimeoutError" ? "Export arrêté après 60 secondes. Aucun fichier partiel n’a été téléchargé." : failure(error);
    } finally { if (!this.#closed && generation === this.#generation && selected === this.#selectionGeneration) { this.#exporting = false; this.#render(); } }
  }
}
