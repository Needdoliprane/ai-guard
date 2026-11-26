import { button, el, icon } from "./dom.ts";
import { formatDate } from "./model.ts";
import { HumanApiClient, HumanApiError, HumanSuperseded } from "./human-api.ts";
import { HumanContractError, hasCanonicalEventId } from "./human-model.ts";
import { ADMIN_DESTINATIONS, accessExplanation, adminDestinationState, can, destinationState } from "./access-model.ts";
import type { AdminDestination } from "./access-model.ts";
import { ConfigurationView, MembershipsView, UsageView } from "./admin-views.ts";
import "./overview.css";
import { RENDER_STEP, SupervisionView, defaultSupervisionPreferences } from "./supervision-view.ts";
import { JournalView, defaultJournalPreferences } from "./journal-view.ts";
import { OverviewView, defaultOverviewPreferences } from "./overview-view.ts";
import type { OverviewPreferences } from "./overview-view.ts";
import { brokerAuthorityLabel, defaultFleetFilter, grantKey, landingDestination, supervisionAccess } from "./supervision-model.ts";
import { GrantEditor, MembershipEditor } from "./delegation-views.ts";
import { SessionLaunchView } from "./session-launch-view.ts";
import { sessionLaunchAvailable } from "./session-launch-model.ts";
import { CAPABILITY_LABELS, DELEGATED_OUTCOME_UNKNOWN, DelegatedChangeRecovery, delegatedContextLost, delegatedReplayRefusal,
  delegationOffers } from "./delegation-model.ts";
import type { Membership } from "./admin-model.ts";
import { captureIntent, deferredReadAllowed, resumeCandidate, resumeRetryDelay } from "./human-continuity.ts";
import type { ResumeIntent } from "./human-continuity.ts";
import "./human.css";
import type { SupervisionPreferences } from "./supervision-view.ts";
import type { JournalPreferences } from "./journal-view.ts";
import type { HumanOperation, Principal, ResourceRef } from "./human-model.ts";
import type { HumanBroker, HumanBrokerPage, HumanWorker, HumanWorkerPage, HumanWorkerAssociation, HumanWorkerTimeStatus, HumanFederationWorkerPage, HumanContext, HumanEvent, HumanEventPage, HumanIdentity, HumanPage, HumanResource, HumanView } from "./human-model.ts";
import { checkpointFacts, checkpointLabel, formatBrokerDate } from "./human-owned-brokers.ts";

interface ReadState<T> {
  value: T | null; loading: boolean; error: string; loadedAt: string | null; stale: boolean;
  cursor: string | null; history: (string | null)[];
}
interface HumanChrome { brand: () => HTMLElement; themeButton: () => HTMLButtonElement }
type FederationLeg = "workers" | "brokers";
interface PagePosition { cursor: string | null; history: (string | null)[] }
const readState = <T>(): ReadState<T> => ({ value: null, loading: false, error: "", loadedAt: null, stale: false, cursor: null, history: [] });
const modeName = (context: HumanContext) => context.mode === "administration" ? "Administration" : "Personnel";
const kindName: Record<HumanResource["resource"]["kind"], string> = { session: "Session", sandbox: "Sandbox", agent: "Agent", project: "Projet" };
const brokerStateName: Record<HumanBroker["state"], string> = { unseen: "Heartbeat non reçu", current: "Bail courant", stale: "Bail périmé", expired: "Autorisation expirée", revoked: "Autorisation révoquée", clock_uncertain: "Horloge incertaine" };
function workerTimeLabel(value: string | null, status: HumanWorkerTimeStatus) {
  if (status === "absent") return "Non fourni";
  if (status === "unknown") return "Inconnu · date hors format ou non interprétable";
  return value && Number.isFinite(Date.parse(value)) ? formatDate(value, true) : value ?? "Inconnu";
}
type DashboardMode = "contexts" | "overview" | AdminDestination | "supervision" | "journal" | "sessions" | "resources" | "events" | "brokers" | "workers" | "federation";
const ADMIN_MODES = Object.keys(ADMIN_DESTINATIONS) as AdminDestination[];
const isAdminMode = (mode: DashboardMode): mode is AdminDestination => (ADMIN_MODES as readonly string[]).includes(mode);
interface ViewPreferences { supervision: SupervisionPreferences; journal: JournalPreferences; overview: OverviewPreferences }
/** Opaque to the session owner: it only hands it from a replaced dashboard to its successor. */
export type HumanResumeIntent = ResumeIntent<ViewPreferences>;
/** Shown while a resume the person confirmed waits for the control to propose a context that is not elapsed. */
const RESUME_PENDING = "Reprise confirmée. Le contrôle propose encore le contexte expiré : vos accès sont relus dans un instant, puis la vue se rouvre sans autre action.";
/** Shown when the tab was hidden before a confirmed resume reopened the view, whatever the control proposes meanwhile: nothing reopens by itself any more. */
const RESUME_INTERRUPTED = "Reprise interrompue : l’onglet a été masqué avant que la vue se rouvre, et rien ne se rouvre dans un onglet masqué. Vos accès sont relus à votre retour sur l’onglet ; si le contrôle propose alors un contexte de mêmes droits, la reprise vous sera proposée de nouveau et attendra votre confirmation.";
const IDENTITY_REVALIDATION_FAILED = "Revalidation des accès impossible. ";
const DELEGATED_ACCESS_CHANGED = `Vos accès actuels ne permettent plus de reprendre ce changement. ${DELEGATED_OUTCOME_UNKNOWN}`;
const RESOURCE_STREAM_HIDDEN = "Flux suspendu · onglet masqué";
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
  #workers = readState<HumanWorkerPage>();
  #workerAbort = new AbortController();
  #workerGeneration = 0;
  #workerForbidden = false;
  #workerLastLaunch = 0;
  #federation = readState<HumanFederationWorkerPage>();
  #federationPositions: Record<FederationLeg, PagePosition> = { workers: { cursor: null, history: [] }, brokers: { cursor: null, history: [] } };
  #federationAbort = new AbortController();
  #federationGeneration = 0;
  #federationForbidden = false;
  #federationRefusal = "";
  #federationLastLaunch = 0;
  #selection: { item: HumanResource; view: HumanView } | null = null;
  #supervision: SupervisionView | null = null;
  /** Typed filters of the unified view. They hold no server data and never outlive the selected context. */
  #supervisionPreferences: SupervisionPreferences = defaultSupervisionPreferences();
  #journal: JournalView | null = null;
  #journalPreferences: JournalPreferences = defaultJournalPreferences();
  #overview: OverviewView | null = null;
  /** Period and cadence chosen for the summary. They hold no server data and never outlive the selected context. */
  #overviewPreferences: OverviewPreferences = defaultOverviewPreferences();
  /** The prepare→launch journey mounted for the selected context; withdrawn with it. */
  #sessionLaunch: SessionLaunchView | null = null;
  /** A subject asked from the summary: opened once, by the destination that reads it, if it is in the loaded window. */
  #pendingSupervisionDetail: { kind: "worker"; id: string } | null = null;
  #pendingBroker: string | null = null;
  /** The one administration view mounted, with the destination it belongs to. */
  #admin: { mode: AdminDestination; view: MembershipsView | UsageView | ConfigurationView } | null = null;
  /** The one delegated-change editor open, withdrawn with its context. */
  #editor: MembershipEditor | GrantEditor | null = null;
  /** After a receipt: the attribution of the context the change was made in, the view to read again and the receipt to show. */
  #changeFollowUp: { key: string; destination: DashboardMode; notice: string } | null = null;
  #delegatedChange: DelegatedChangeRecovery;
  /** Restrictive coverage observed on a real page, scoped to this exact context. */
  #journalProfile: { contextId: string; authorizationVersion: string; streamMode: "snapshot_only" | null } | null = null;
  /** Set only after a context expiry or a session rotation; dropped by any other context change. */
  #resume: HumanResumeIntent | null = null;
  #resumeOffer: HumanContext | null = null;
  #resumeAttempts = 0;
  #resumeTimer: ReturnType<typeof setTimeout> | undefined;
  /** Fires when the offered context has itself expired, so that the offer says so. */
  #offerTimer: ReturnType<typeof setTimeout> | undefined;
  /**
   * The person clicked on an offer that had expired: the accesses are read for that click, then at most
   * three more times by themselves, whatever was read before it. The confirmation stands while such reads
   * follow one another in a visible tab. It ends with the first one that fails or decides otherwise, and with
   * a hidden tab: there no deferred read starts and no view reopens, even for a valid context.
   */
  #resumeConfirmed = false;
  /** A visibility change ends a confirmation immediately, including while its identity read is in flight. */
  #resumeInterrupted = false;
  #focusResume = false;
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
  #identitySequence = 0;
  #identityOffersWithdrawn = false;
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
  // Navigation cancels individual streams too, but only suspension requires a fresh identity before resuming.
  #resourceStreamNeedsAccess = false;
  #visibilityGeneration = 0;
  #visibility = () => {
    if (this.#closed) return;
    if (document.visibilityState !== "visible") {
      this.#visibilityGeneration++;
      this.#streamAbort.abort(); this.#supervision?.suspend(); this.#journal?.suspend();
      if (!this.#api.readBudgetStopped && this.#usesResourceStream()) {
        this.#resourceStreamNeedsAccess = true;
        this.#invalidation++;
        this.#resources.stale = true; this.#events.stale = true;
        this.#streamStatus = RESOURCE_STREAM_HIDDEN;
        this.#render();
      }
      if (this.#resumeConfirmed) {
        this.#resumeConfirmed = false; this.#resumeInterrupted = true;
        clearTimeout(this.#resumeTimer); this.#resumeTimer = undefined;
        this.#message = RESUME_INTERRUPTED; this.#render();
      }
      return;
    }
    // No read while the budget is stopped: the unified views still leave their suspension and name that reason.
    if (this.#api.readBudgetStopped) { this.#resumeViews(); return; }
    if (this.#mode === "brokers" && this.#brokerForbidden || this.#mode === "workers" && this.#workerForbidden || this.#mode === "federation" && this.#federationForbidden) return;
    void this.#revalidateVisibleContext();
  };
  async #revalidateVisibleContext(operatorRetry = false) {
    if (this.#closed || document.visibilityState !== "visible") return;
    if (!this.#api.readBudgetStopped && this.#resourceStreamAwaitingAccess()) this.#streamStatus = "Flux suspendu · accès à relire";
    const generation = this.#generation;
    const visibility = this.#visibilityGeneration;
    const success = await this.#loadIdentity(operatorRetry);
    if (this.#closed || generation !== this.#generation || !this.#context || document.visibilityState !== "visible") return;
    // A second suspension during the read still needs its own revalidation after the loading latch is released.
    if (visibility !== this.#visibilityGeneration) {
      if (!this.#api.readBudgetStopped) await this.#revalidateVisibleContext();
      else this.#resumeViews();
      return;
    }
    // The unified views resume even when the identity could not be read: their own read reports the real
    // state (unreachable, refused, context lost) with its retry, instead of a suspension that is over.
    this.#resumeViews();
    if (!success) return;
    this.#resourceStreamNeedsAccess = false;
    if (this.#usesResourceStream()) this.#startStream();
    this.#refreshActive(true);
  }
  #resumeViews() {
    if (document.visibilityState !== "visible") return;
    this.#supervision?.resume(); this.#journal?.resume();
  }
  #navigationResize = () => {
    const row = this.#navigation?.querySelector<HTMLElement>(".human-nav");
    if (row && row.scrollWidth > row.clientWidth) {
      const focused = row.querySelector<HTMLElement>(".human-tab:focus");
      const destination = focused ?? row.querySelector<HTMLElement>('[aria-current="page"]');
      if (destination) {
        const bounds = row.getBoundingClientRect();
        const visible = destination.getBoundingClientRect();
        const style = getComputedStyle(row);
        const canScrollLeft = row.scrollLeft > 1;
        const canScrollRight = row.scrollLeft + row.clientWidth < row.scrollWidth - 1;
        const left = visible.left - bounds.left - (canScrollLeft ? parseFloat(style.scrollPaddingLeft) || 0 : 0);
        const right = visible.right - bounds.right + (canScrollRight ? parseFloat(style.scrollPaddingRight) || 0 : 0);
        if (left < -1) row.scrollLeft += left;
        else if (right > 1) row.scrollLeft += right;
      }
    }
    this.#markNavigation();
  };
  /** Says on which side the destination row has more to scroll to; CSS fades that edge. */
  #markNavigation() {
    const row = this.#navigation?.querySelector<HTMLElement>(".human-nav"); if (!row) return;
    const start = row.scrollLeft > 1; const end = row.scrollLeft + row.clientWidth < row.scrollWidth - 1;
    const more = start && end ? "both" : start ? "start" : end ? "end" : "";
    if (row.dataset.more !== more) row.dataset.more = more;
  }
  constructor(root: HTMLElement, token: string, chrome: HumanChrome, onLogout: (message: string) => void, resume: HumanResumeIntent | null = null) {
    this.#root = root; this.#chrome = chrome; this.#onLogout = onLogout; this.#resume = resume;
    this.#api = new HumanApiClient(token, {
      onAuthentication: (error) => { if (this.#mounted && !this.#closed) this.logout(error.message); },
      onContextInvalidated: (error, identity) => {
        if (!this.#mounted || this.#closed) return;
        const readingIdentity = this.#identityLoading;
        const changePending = this.#delegatedChange.phase !== null && delegatedContextLost(error);
        const review = changePending && identity ? this.#delegatedChange.revalidate(identity, Date.now()) : undefined;
        const keepChange = changePending && this.#delegatedChange.phase !== null;
        // Captured before the purge and holding no server row: at most the view and the typed filters.
        const intent = captureIntent({ cause: error.code, principal: this.#identity?.principal ?? null, context: this.#context, view: this.#mode,
          now: Date.now(), budgetStopped: this.#api.readBudgetStopped, payload: { supervision: this.#supervisionPreferences, journal: this.#journalPreferences, overview: this.#overviewPreferences } });
        this.#purgeContext(keepChange); this.#resume = intent;
        if (changePending && identity) {
          this.#identity = identity;
          this.#identityOffersWithdrawn = false;
        } else if (keepChange || error.code === "permission_denied") this.#identityOffersWithdrawn = true;
        this.#message = review === "withdrawn" ? DELEGATED_ACCESS_CHANGED : keepChange ? DELEGATED_OUTCOME_UNKNOWN : error.message;
        this.#mode = "contexts"; this.#render();
        if (changePending && document.visibilityState === "visible") this.#focusHeading();
        if (!readingIdentity && !this.#api.readBudgetStopped && this.#delegatedChange.phase !== "sending") void this.#loadIdentity();
      },
      onReadBudgetExhausted: () => {
        this.#streamAbort.abort(); this.#exportAbort.abort(); clearTimeout(this.#refreshTimer); this.#refreshTimer = undefined;
        this.#resources.stale = true; this.#events.stale = true; this.#brokers.stale = true; this.#workers.stale = true; this.#federation.stale = true;
        this.#streamStatus = "Lecture automatique suspendue"; this.#supervision?.budgetStopped(); this.#render();
      },
    });
    this.#delegatedChange = new DelegatedChangeRecovery(this.#api);
  }
  async start() {
    this.#identity = await this.#api.identity();
    if (this.#closed) throw new HumanSuperseded();
    this.#mounted = true; this.#shell(); this.#evaluateResume(); this.#render();
    this.#poll = setInterval(() => this.#refreshActive(true), 30000);
    document.addEventListener("visibilitychange", this.#visibility);
    window.addEventListener("resize", this.#navigationResize);
  }
  close() {
    this.#closed = true; this.#purgeContext(); this.#api.close(); this.#identity = null;
    clearInterval(this.#poll); this.#page?.replaceChildren(); if (this.#mounted) this.#root.replaceChildren();
    document.removeEventListener("visibilitychange", this.#visibility);
    window.removeEventListener("resize", this.#navigationResize);
  }
  /** For the session owner, just before it replaces this dashboard after a token rotation. Nothing is read. */
  continuity(): HumanResumeIntent | null {
    if (this.#closed) return null;
    return this.#resume ?? captureIntent({ cause: "session_rotated", principal: this.#identity?.principal ?? null, context: this.#context, view: this.#mode,
      now: Date.now(), budgetStopped: this.#api.readBudgetStopped, payload: { supervision: this.#supervisionPreferences, journal: this.#journalPreferences, overview: this.#overviewPreferences } });
  }
  /**
   * After an identity read with a pending intention: offer the one equivalent context the control proposes,
   * read the identity again when it still carries the elapsed expiry, or fall back to the explicit choice.
   */
  #evaluateResume() {
    const intent = this.#resume; const identity = this.#identity;
    if (!intent || !identity || this.#context) return;
    const confirmed = this.#resumeConfirmed;
    const visible = document.visibilityState === "visible";
    if (visible) this.#resumeInterrupted = false;
    const decision = resumeCandidate(intent, identity, { now: Date.now(), visible,
      budgetStopped: this.#api.readBudgetStopped, attempts: this.#resumeAttempts, confirmed });
    // The control may still propose the elapsed context for a moment: the confirmation outlives those re-reads only.
    if (decision.kind !== "retry") this.#resumeConfirmed = false;
    clearTimeout(this.#offerTimer);
    if (decision.kind === "offer") {
      const offer = decision.context; this.#resumeOffer = offer; this.#message = "";
      // The person already confirmed, on an offer that had expired meanwhile: this identity read was made for that click.
      if (decision.reopen) { this.#acceptResume(); return; }
      // Confirmed, but the tab is hidden: the valid context stays an offer, which waits for a click at the return.
      if (confirmed || this.#resumeInterrupted) this.#message = RESUME_INTERRUPTED;
      this.#focusResume = true;
      // The offered context has its own life of at most 300 seconds: when it ends, the offer says so.
      this.#offerTimer = setTimeout(() => { if (!this.#closed && this.#resumeOffer === offer && !this.#context) this.#render(); },
        Math.max(0, Date.parse(offer.expires_at) - Date.now()) + 50);
      return;
    }
    this.#resumeOffer = null;
    if (decision.kind === "retry") {
      clearTimeout(this.#resumeTimer);
      this.#resumeTimer = setTimeout(() => {
        if (this.#closed || this.#resume !== intent || this.#context) return;
        if (deferredReadAllowed({ visible: document.visibilityState === "visible" })) { this.#readForResume(); return; }
        // Hidden tab: this read does not start. A confirmation ends here and the page says so; the return
        // to the foreground reads the accesses and decides again.
        if (!this.#resumeConfirmed) return;
        this.#resumeConfirmed = false; this.#resumeInterrupted = true; this.#message = RESUME_INTERRUPTED; this.#render();
      }, resumeRetryDelay(this.#resumeAttempts));
      this.#resumeAttempts++;
      if (confirmed) this.#message = RESUME_PENDING;
      return;
    }
    // A hidden tab arms no deferred read; its return to the foreground reads the identity and decides again.
    if (decision.reason === "hidden") {
      // The confirmation ended above: the view will not reopen by itself, and the page no longer says it will.
      if (confirmed) this.#message = RESUME_INTERRUPTED;
      return;
    }
    if (decision.reason === "still_expired") {
      // Automatic reads stop here. The intention stays for an explicit refresh of the list, which reads the
      // identity once and offers the resume as soon as the control proposes a new equivalent context.
      this.#message = "Le contexte a expiré et le contrôle propose encore l’ancien. Actualisez la liste dans un instant : la reprise sera proposée dès qu’un nouveau contexte de mêmes droits existera.";
      return;
    }
    this.#resume = null;
    const lost = intent.cause === "session_rotated" ? "Votre session a été renouvelée." : "Le contexte a expiré.";
    this.#message = decision.reason === "none" || decision.reason === "ambiguous" ? `${lost} Aucun contexte unique de mêmes droits n’est proposé : choisissez un contexte.` :
      decision.reason === "too_old" ? "La reprise n’est proposée que pendant 15 minutes après la perte du contexte. Choisissez un contexte." :
        "Le contexte précédent n’est plus sélectionné. Choisissez un contexte.";
  }
  #acceptResume() {
    if (document.visibilityState !== "visible" || this.#delegatedChange.phase === "sending") return;
    const intent = this.#resume; const offer = this.#resumeOffer; if (!intent || !offer) return;
    if (Date.parse(offer.expires_at) <= Date.now()) {
      // The offered context expired while the list stayed open. This click is the explicit action: the
      // accesses are read and the view reopens on the equivalent context the control proposes now.
      // Nothing is purged before that answer, so the intention and its filters survive a failed read.
      // The automatic reads an earlier expiry used are not counted against this click: it gets its own.
      this.#resumeAttempts = 0; this.#resumeConfirmed = true; this.#resumeInterrupted = false; this.#readForResume();
      return;
    }
    // The shared selector checks the offer before purging context data.
    try { if (!this.#selectOfferedContext(offer.id)) return; }
    catch (error) { this.#resumeOffer = null; this.#identityError = failure(error); this.#render(); return; }
    const { view, payload } = intent;
    this.#supervisionPreferences = payload.supervision; this.#journalPreferences = payload.journal; this.#overviewPreferences = payload.overview; this.#openView(view);
  }
  /** Reads the accesses for a pending resume. A read that fails or does not start ends a confirmation already given. */
  #readForResume() {
    void this.#loadIdentity().then((read) => {
      if (read === true || !this.#resumeConfirmed) return;
      this.#resumeConfirmed = false;
      // Nothing is read again by itself any more: the failure is displayed and the next resume needs a new click.
      if (this.#message === RESUME_PENDING) { this.#message = ""; this.#render(); }
    });
  }
  logout(message?: string) {
    const detail = message ?? (this.#identity?.authentication.method === "oidc_access_token" ?
      "Session OIDC locale fermée." : "Session personnelle fermée. Le credential local reste administré par le contrôle.");
    this.close(); this.#onLogout(detail);
  }
  #purgeContext(keepDelegatedChange = false) {
    if (keepDelegatedChange) this.#delegatedChange.withdrawOffer();
    else this.#delegatedChange.clear();
    this.#closeEditor();
    this.#generation++; this.#selectionGeneration++; this.#invalidation++;
    this.#scope.abort(); this.#scope = new AbortController();
    this.#selectionAbort.abort(); this.#selectionAbort = new AbortController();
    this.#streamAbort.abort(); this.#exportAbort.abort(); this.#detailAbort.abort();
    this.#resourceStreamNeedsAccess = false;
    clearTimeout(this.#refreshTimer); this.#refreshTimer = undefined;
    this.#context = null; this.#journalProfile = null; this.#selection = null; this.#resources = readState(); this.#events = readState();
    this.#closeSupervision(); this.#supervisionPreferences = defaultSupervisionPreferences();
    this.#closeJournal(); this.#journalPreferences = defaultJournalPreferences();
    this.#closeOverview(); this.#overviewPreferences = defaultOverviewPreferences(); this.#closeAdmin();
    this.#closeSessionLaunch();
    this.#pendingSupervisionDetail = null; this.#pendingBroker = null;
    this.#resume = null; this.#resumeOffer = null; this.#resumeAttempts = 0; clearTimeout(this.#resumeTimer);
    this.#resumeConfirmed = false; this.#resumeInterrupted = false; clearTimeout(this.#offerTimer);
    this.#purgeBrokers(); this.#brokerForbidden = false; this.#brokerLastLaunch = 0;
    this.#purgeWorkers(); this.#workerForbidden = false; this.#workerLastLaunch = 0;
    this.#purgeFederation(); this.#federationForbidden = false; this.#federationRefusal = ""; this.#federationLastLaunch = 0;
    if (this.#identity) this.#identity = { ...this.#identity, contexts: [] };
    this.#identityLoading = false;
    this.#exporting = false; this.#exportMessage = ""; this.#exportPages = 0; this.#exportBytes = 0;
    this.#streamStatus = "Flux arrêté"; this.#streamError = "";
    this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
    if (this.#downloadUrl) URL.revokeObjectURL(this.#downloadUrl); this.#downloadUrl = null;
  }
  // Presentation gating only, from the operation catalogue the control established for the selected context.
  // A capability list never opens a view; Rust revalidates every read.
  #canResources() { return can(this.#context, "resources_read"); }
  #canEvents() { return can(this.#context, "events_read_metadata"); }
  /** The content view also needs the per-resource content permission returned with the resource. */
  #canContent() { return can(this.#context, "events_read_content"); }
  #canExport() { return can(this.#context, "events_export"); }
  #canBrokers() { return can(this.#context, "brokers_read"); }
  #purgeBrokers() {
    this.#brokerGeneration++; this.#brokerAbort.abort(); this.#brokerAbort = new AbortController(); this.#brokers = readState();
    if (this.#mode === "brokers") { this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null; }
  }
  #canWorkers() { return can(this.#context, "workers_read"); }
  #purgeWorkers() {
    this.#workerGeneration++; this.#workerAbort.abort(); this.#workerAbort = new AbortController(); this.#workers = readState();
    if (this.#mode === "workers") { this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null; }
  }
  #canFederation() { return can(this.#context, "federation_read"); }
  #canSupervise() { return supervisionAccess(this.#context).any; }
  #closeSupervision() { this.#supervision?.close(); this.#supervision = null; }
  #openSupervision() {
    const context = this.#context; if (!context) return;
    this.#closeSupervision();
    this.#supervision = new SupervisionView({ reads: this.#api, context, contextsComplete: () => this.#identity?.contexts_complete === true,
      preferences: this.#supervisionPreferences, onReviewAccess: () => this.#openView("contexts"),
      onAccessDoubt: () => { void this.#revalidateAccess(); },
      onReadStateChange: () => this.#updateRefreshState(),
      hasRegistry: (view) => view === "workers" ? !!this.#canWorkers() : view === "brokers" ? !!this.#canBrokers() : this.#canResources(),
      openRegistry: (view) => this.#openView(view),
      openJournal: this.#canJournal() ? (resource) => { this.#journalPreferences = { ...defaultJournalPreferences(), resource }; this.#openView("journal"); } : null,
      initialDetail: this.#pendingSupervisionDetail });
    this.#pendingSupervisionDetail = null;
    this.#supervision.start();
  }
  /**
   * The unified view met a refusal whose reason it could not read. The identity read is the authority: a
   * context that is revoked or expired is purged by it; a context still offered unchanged had only its
   * registries refused, and what was withdrawn meanwhile is read again.
   */
  async #revalidateAccess() {
    const view = this.#supervision; const generation = this.#generation;
    const confirmed = await this.#loadIdentity();
    if (confirmed && !this.#closed && generation === this.#generation && this.#context && view && view === this.#supervision) void view.reread();
  }
  #canJournal() { return supervisionAccess(this.#context).journal; }
  #currentJournalProfile() {
    return this.#journalProfile?.contextId === this.#context?.id &&
      this.#journalProfile?.authorizationVersion === this.#context?.authorization_version ? this.#journalProfile : null;
  }
  #snapshotOnly() { return this.#currentJournalProfile()?.streamMode === "snapshot_only"; }
  #journalStatus() {
    return this.#snapshotOnly() ? "Journal · historique · direct indisponible pour ce profil" :
      this.#currentJournalProfile() ? "Journal · récent d’abord, puis direct" : "Journal · lecture de l’historique";
  }
  #journalDescription() {
    return this.#snapshotOnly() ?
      "Historique des métadonnées autorisées, à l’heure de lecture. Direct, contenu et export indisponibles pour ce profil. Actualiser effectue une nouvelle lecture." :
      this.#currentJournalProfile() ?
        "Tous les événements que ce contexte peut lire, les plus récents d’abord, puis en direct. L’onglet Événements reste la lecture d’une ressource choisie, avec son contenu et son export selon vos droits." :
        "Les événements les plus récents que ce contexte peut lire. Le mode de suivi et la portée disponibles seront indiqués après une lecture réussie.";
  }
  #closeJournal() { const previous = this.#journal; this.#journal = null; previous?.close(); }
  /** "view": the summary is read; "explanation": granted but not served now, the destination says why; null: no destination. */
  #overviewState() { return destinationState(this.#context, "summary_read"); }
  #closeOverview() { const previous = this.#overview; this.#overview = null; previous?.close(); }
  #openOverview() {
    const context = this.#context;
    this.#closeOverview();
    if (!context || this.#overviewState() !== "view") return;
    const generation = this.#generation;
    const view: OverviewView = new OverviewView({ context, preferences: this.#overviewPreferences,
      read: (window, signal) => this.#api.summary(window, signal),
      budgetStopped: () => this.#api.readBudgetStopped,
      // The refused summary already withdrew its values: the identity read decides whether the context still stands.
      onAccessDoubt: () => { if (!this.#closed && generation === this.#generation && this.#overview === view) void this.#loadIdentity(); },
      onReadStateChange: () => this.#updateRefreshState(),
      openSubject: this.#canWorkers() || this.#canBrokers() ? (subject) => this.#openSubject(subject) : null,
      canOpenSubject: (kind) => kind === "worker" ? this.#canWorkers() : this.#canBrokers() });
    this.#overview = view; view.start();
  }
  /** The journey is offered only when the catalogue grants and serves the preparation mutation. */
  #canSessionLaunch() { return sessionLaunchAvailable(this.#context); }
  #closeSessionLaunch() { const previous = this.#sessionLaunch; this.#sessionLaunch = null; previous?.close(); }
  #openSessionLaunch() {
    const context = this.#context;
    this.#closeSessionLaunch();
    if (!context || !this.#canSessionLaunch()) return;
    const generation = this.#generation;
    const view = new SessionLaunchView({ context, api: this.#api,
      onAccessDoubt: () => { if (!this.#closed && generation === this.#generation && this.#sessionLaunch === view) void this.#loadIdentity(); },
      onReadStateChange: () => this.#updateRefreshState() });
    this.#sessionLaunch = view; view.start();
  }
  /** From a point of attention of the summary to the exact view of its subject, when this context offers it. */
  #openSubject(subject: { kind: "worker" | "broker"; id: string }) {
    if (this.#closed || this.#mode !== "overview") return;
    if (subject.kind === "worker" && this.#canWorkers()) {
      // Only the worker filter is replaced: the refresh period and the other choices of the person stay.
      this.#supervisionPreferences = { ...this.#supervisionPreferences, fleet: { ...defaultFleetFilter(), text: subject.id }, rendered: RENDER_STEP };
      this.#pendingSupervisionDetail = { kind: "worker", id: subject.id }; this.#openView("supervision");
    } else if (subject.kind === "broker" && this.#canBrokers()) {
      // The registry read starts with the view; its answer opens this broker when it is on the page read.
      this.#openView("brokers"); if ((this.#mode as DashboardMode) === "brokers") this.#pendingBroker = subject.id;
    }
  }
  /** A destination granted but not served now: why, operation by operation, with no read and no value in its place. */
  #renderUnavailable(title: string, operations: readonly HumanOperation[], key: string) {
    const node = el("section", "panel human-overview-unavailable", el("h2", "", title),
      ...operations.map((operation) => el("p", "", accessExplanation(this.#context, operation))),
      el("p", "muted", "Aucune lecture n’est émise et aucune valeur n’est affichée à sa place. Les autres destinations de ce contexte gardent leurs propres accès."),
      action("Relire mes accès", key, () => this.#openView("contexts")));
    node.setAttribute("role", "status");
    return node;
  }
  #adminState(mode: AdminDestination) { return adminDestinationState(this.#context, mode); }
  #closeAdmin() { const previous = this.#admin; this.#admin = null; previous?.view.close(); }
  #openAdmin(mode: AdminDestination) {
    const context = this.#context;
    this.#closeAdmin();
    if (!context || this.#adminState(mode) !== "view") return;
    const generation = this.#generation;
    const host = { context, budgetStopped: () => this.#api.readBudgetStopped,
      // The refused read already withdrew its values: the identity read decides whether the context still stands.
      onAccessDoubt: () => { if (!this.#closed && generation === this.#generation && this.#admin?.view === view) void this.#loadIdentity(); },
      onReadStateChange: () => this.#updateRefreshState() };
    const view: MembershipsView | UsageView | ConfigurationView = mode === "memberships" ?
      // The delegated change is offered only when the catalogue grants and serves memberships_manage.
      new MembershipsView({ ...host, read: (cursor, signal) => this.#api.memberships(cursor, signal),
        change: delegationOffers(context).memberships && this.#delegatedChange.phase === null ? (membership) => this.#editMembership(membership) : null }) :
      mode === "usage" ? new UsageView({ ...host, owned: this.#api.readProfiles !== undefined,
        quota: (signal) => this.#api.quota(signal), usage: (cursor, signal) => this.#api.usage(cursor, signal) }) :
        new ConfigurationView({ ...host, read: (signal) => this.#api.configuration(signal) });
    this.#admin = { mode, view }; view.start();
  }
  #closeEditor() { const editor = this.#editor; this.#editor = null; editor?.close(); }
  #editMembership(membership: Membership) {
    const context = this.#context;
    if (!context || membership.own || !delegationOffers(context).memberships || this.#closed || this.#delegatedChange.phase !== null) return;
    this.#closeEditor();
    const generation = this.#generation;
    const editor: MembershipEditor = new MembershipEditor({ context, membership,
      send: (change, signal) => this.#submitDelegatedChange(change.operation_id, signal, (principal, current) =>
        this.#delegatedChange.submitMembership(principal, current, membership.membership_id, change, signal)),
      onApplied: (receipt) => {
        if (this.#editor !== editor) return;
        void this.#followChange("memberships", `Changement enregistré : appartenance de ${membership.display_name} en révision ${receipt.membership_revision} · révision d’autorisation ${receipt.authorization_revision} · position d’audit ${receipt.audit_seq}.`);
      },
      onStale: () => { if (this.#editor === editor && this.#mode === "memberships") void this.#admin?.view.refresh(); },
      onRefused: () => {
        if (this.#editor !== editor) return;
        this.#closeEditor();
        this.#message = "Le contrôle a refusé le changement d’appartenance. Vos accès sont relus.";
        this.#render();
        if (generation === this.#generation) void this.#loadIdentity();
        if (document.visibilityState === "visible" && !this.#page?.contains(document.activeElement)) this.#focusHeading();
      },
      onClosed: () => { if (this.#editor === editor) this.#editor = null; } });
    this.#editor = editor; editor.open();
  }
  async #submitDelegatedChange<R>(operationId: string, signal: AbortSignal,
    submit: (principal: Principal, context: HumanContext) => Promise<R>) {
    const context = this.#context; const principal = this.#identity?.principal;
    if (this.#closed || !context || !principal || signal.aborted) throw new HumanSuperseded();
    try {
      return await submit(principal, context);
    } catch (error) {
      if (!this.#closed && this.#delegatedChange.operationId === operationId && this.#delegatedChange.phase === "unknown")
        this.#withdrawDelegatedContext(DELEGATED_OUTCOME_UNKNOWN, true);
      throw error;
    }
  }
  #withdrawDelegatedContext(notice: string, keepDelegatedChange = false) {
    const readingIdentity = this.#identityLoading;
    this.#api.clearContext();
    this.#purgeContext(keepDelegatedChange);
    this.#identityOffersWithdrawn = true;
    this.#mode = "contexts"; this.#message = notice; this.#identityError = ""; this.#render();
    if (!readingIdentity && !this.#api.readBudgetStopped && !this.#delegatedChange.unavailable) void this.#loadIdentity();
    if (document.visibilityState === "visible") this.#focusHeading();
  }
  async #retryDelegatedChange() {
    if (this.#closed || this.#mode !== "contexts" || this.#identityLoading || this.#api.readBudgetStopped) return;
    const now = Date.now(); const offer = this.#delegatedChange.offer(now);
    if (!offer) { this.#render(); return; }
    const operationId = this.#delegatedChange.operationId;
    const generation = this.#generation; const signal = this.#scope.signal;
    try {
      this.#message = offer.kind === "grant" ? "Envoi de la même demande de changement des partages au contrôle…" :
        "Envoi de la même demande d’appartenance au contrôle…";
      const pending = this.#delegatedChange.retry(now, signal);
      this.#render();
      const result = await pending;
      if (!result || this.#closed || generation !== this.#generation) return;
      const revision = result.kind === "grant" ? `Révision des partages ${result.receipt.grant_revision}` :
        `Révision de l’appartenance ${result.receipt.membership_revision}`;
      this.#withdrawDelegatedContext(`Changement confirmé par le contrôle. ${revision} · position d’audit ${result.receipt.audit_seq}.`);
    } catch (error) {
      if (this.#closed) return;
      if (this.#delegatedChange.operationId === operationId && this.#delegatedChange.phase === "unknown")
        this.#withdrawDelegatedContext(DELEGATED_OUTCOME_UNKNOWN, true);
      else if (generation === this.#generation)
        this.#withdrawDelegatedContext(delegatedReplayRefusal(offer.kind, error));
    }
  }
  #editGrant(resource: ResourceRef) {
    const context = this.#context; const self = this.#identity?.principal;
    if (!context || !self || context.mode !== "administration" || !delegationOffers(context).grants ||
        this.#closed || this.#delegatedChange.phase !== null) return;
    this.#closeEditor();
    const generation = this.#generation;
    const editor: GrantEditor = new GrantEditor({ context, self, resource,
      read: (signal) => this.#api.grant(resource, signal),
      members: (cursor, signal) => this.#api.memberships(cursor, signal),
      send: (change, owner, signal) => this.#submitDelegatedChange(change.operation_id, signal, (principal, current) =>
        this.#delegatedChange.submitGrant(principal, current, change, owner, signal)),
      onApplied: (receipt) => {
        if (this.#editor !== editor) return;
        void this.#followChange("resources", `Partages enregistrés : ${resource.kind} ${resource.resource_id} en révision ${receipt.grant_revision} · révision d’autorisation ${receipt.authorization_revision} · position d’audit ${receipt.audit_seq}.`);
      },
      onRefused: () => {
        if (this.#editor !== editor) return;
        this.#closeEditor();
        this.#message = "Le contrôle a refusé l’opération sur les partages. Vos accès sont relus.";
        this.#render();
        if (generation === this.#generation) void this.#loadIdentity();
        if (document.visibilityState === "visible" && !this.#page?.contains(document.activeElement)) this.#focusHeading();
      },
      onClosed: () => { if (this.#editor === editor) this.#editor = null; } });
    this.#editor = editor; editor.open();
  }
  /**
   * A change was recorded under a new authorization revision: the selected context is now refused. The accesses are
   * read again, the one context of the same attribution the control proposes is selected, and the view is read again
   * with the receipt displayed. Without exactly one such context the person chooses, with the receipt kept on screen.
   */
  async #followChange(destination: DashboardMode, notice: string) {
    const previous = this.#context; if (!previous || this.#closed) return;
    this.#closeEditor();
    this.#changeFollowUp = { key: grantKey(previous), destination, notice };
    const read = await this.#loadIdentity();
    const followUp = this.#changeFollowUp; this.#changeFollowUp = null;
    if (!followUp || this.#closed) return;
    if (read !== true) { this.#message = `${notice} Vos accès n’ont pas pu être relus : actualisez la liste des contextes.`; this.#render(); return; }
    if (this.#context?.id === previous.id) { this.#message = notice; this.#openView(destination); return; }
    const candidates = (this.#identity?.contexts ?? []).filter((context) => grantKey(context) === followUp.key && Date.parse(context.expires_at) > Date.now());
    if (candidates.length !== 1) {
      this.#message = `${notice} Choisissez de nouveau un contexte : le contrôle n’en propose pas un seul de mêmes droits.`; this.#mode = "contexts"; this.#render(); return;
    }
    try { if (!this.#selectOfferedContext(candidates[0]!.id)) return; }
    catch (error) { this.#identityError = failure(error); this.#message = notice; this.#mode = "contexts"; this.#render(); return; }
    this.#message = notice; this.#openView(followUp.destination);
  }
  #openJournal() {
    const context = this.#context; if (!context) return;
    this.#closeJournal();
    const generation = this.#generation;
    const journal: JournalView = new JournalView({ reads: this.#api, context, preferences: this.#journalPreferences, onReviewAccess: () => this.#openView("contexts"),
      onReadStateChange: () => this.#updateRefreshState(),
      onProfileMode: (mode, confirmed) => {
        const current = this.#context;
        if (this.#closed || this.#journal !== journal || generation !== this.#generation ||
          !current || current.id !== context.id || current.authorization_version !== context.authorization_version) return;
        this.#journalProfile = confirmed ? { contextId: context.id, authorizationVersion: context.authorization_version, streamMode: mode } : null;
        if (this.#mode === "journal") {
          this.#streamStatus = this.#journalStatus();
          if (this.#streamLabel) this.#streamLabel.textContent = this.#streamStatus;
          const description = this.#page?.querySelector(".page-description");
          if (description) description.textContent = this.#journalDescription();
        }
      },
      openWorkers: this.#canWorkers() ? (sessionId) => {
        // Only the worker filter is replaced: the refresh period and the other choices of the person stay.
        this.#supervisionPreferences = { ...this.#supervisionPreferences, fleet: { ...defaultFleetFilter(), text: sessionId }, rendered: RENDER_STEP }; this.#openView("supervision");
      } : null });
    this.#journal = journal; journal.start();
  }
  #purgeFederation() {
    this.#federationGeneration++; this.#federationAbort.abort(); this.#federationAbort = new AbortController(); this.#federation = readState();
    this.#federationPositions = { workers: { cursor: null, history: [] }, brokers: { cursor: null, history: [] } };
    if (this.#mode === "federation") { this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null; }
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
    if (this.#delegatedChange.phase === "sending") return;
    // One view owns server data at a time: leaving the unified view aborts its reads and drops its rows.
    if (mode !== "supervision" && this.#mode === "supervision") this.#closeSupervision();
    if (mode !== "journal" && this.#mode === "journal") this.#closeJournal();
    if (mode !== this.#mode) this.#closeEditor();
    if (mode !== "overview" && this.#mode === "overview") this.#closeOverview();
    if (mode !== "sessions" && this.#mode === "sessions") this.#closeSessionLaunch();
    if (this.#admin && mode !== this.#admin.mode) this.#closeAdmin();
    if (mode !== "brokers") this.#pendingBroker = null;
    if (mode === "overview" && this.#overviewState()) {
      if (this.#mode !== mode || this.#overviewState() === "view" && !this.#overview) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode; this.#openOverview();
      } else void this.#overview?.refresh();
      this.#streamStatus = this.#overview ? "Synthèse · instantanés ponctuels" : "Synthèse indisponible pour ce contexte";
    } else if (isAdminMode(mode) && this.#adminState(mode)) {
      if (this.#mode !== mode || this.#adminState(mode) === "view" && !this.#admin) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode; this.#openAdmin(mode);
      } else void this.#admin?.view.refresh();
      this.#streamStatus = this.#admin ? "Administration · lectures ponctuelles" : "Lecture indisponible pour ce contexte";
    } else if (mode === "journal" && this.#canJournal()) {
      if (this.#mode !== mode || !this.#journal) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode; this.#openJournal();
      } else void this.#journal.reload();
      this.#streamStatus = this.#journalStatus();
    } else if (mode === "supervision" && this.#canSupervise()) {
      if (this.#mode !== mode || !this.#supervision) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode; this.#openSupervision();
      } else void this.#supervision.refresh();
      this.#streamStatus = "Supervision · lectures ponctuelles";
    } else if (mode === "sessions" && this.#canSessionLaunch()) {
      if (this.#mode !== mode || !this.#sessionLaunch) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode; this.#openSessionLaunch();
      } else void this.#sessionLaunch.refresh();
      this.#streamStatus = "Session · préparation puis lancement";
    } else if (mode === "contexts") {
      const keepChange = this.#delegatedChange.phase === "unknown";
      this.#api.clearContext(); this.#purgeContext(keepChange); this.#mode = mode;
      if (keepChange) this.#message = DELEGATED_OUTCOME_UNKNOWN;
      if (!this.#api.readBudgetStopped) void this.#loadIdentity();
    } else if (mode === "federation" && this.#canFederation()) {
      if (this.#mode !== mode) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode;
      }
      this.#streamStatus = "Fédération · snapshots indépendants"; this.#refreshActive(false);
    } else if (mode === "workers" && this.#canWorkers()) {
      if (this.#mode !== mode) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode;
      }
      this.#streamStatus = "Registre · lectures ponctuelles"; this.#refreshActive(false);
    } else if (mode === "brokers" && this.#canBrokers()) {
      if (this.#mode !== mode) {
        this.#stopViewReads(); this.#resources = readState(); this.#events = readState(); this.#selection = null;
        this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode;
      }
      this.#streamStatus = "Registre · sans flux temps réel"; this.#refreshActive(false);
    } else if (mode === "resources" && this.#canResources()) {
      this.#stopViewReads(); this.#purgeBrokers(); this.#purgeWorkers(); this.#purgeFederation(); this.#mode = mode; this.#resources = readState();
      this.#events = readState(); this.#selection = null;
      this.#startStream(); this.#refreshActive(false);
    } else if (mode === "events" && this.#selection && this.#canEvents()) {
      this.#mode = mode; this.#startStream(); this.#refreshActive(false);
    }
    this.#render(); this.#focusHeading();
  }
  #focusHeading() { this.#page?.querySelector<HTMLElement>("h1")?.focus({ preventScroll: true }); }
  #refreshActive(automatic: boolean) {
    if (this.#closed || this.#api.readBudgetStopped || automatic &&
      (document.visibilityState !== "visible" || this.#resourceStreamAwaitingAccess())) return;
    // The unified view runs its own bounded cycle; only an explicit action reads from here.
    if (this.#mode === "supervision") { if (!automatic) void this.#supervision?.refresh(); return; }
    if (this.#mode === "journal") { if (!automatic) void this.#journal?.reload(); return; }
    if (this.#mode === "overview") { if (!automatic) void this.#overview?.refresh(); return; }
    if (this.#mode === "sessions") { if (!automatic) void this.#sessionLaunch?.refresh(); return; }
    if (isAdminMode(this.#mode)) { if (!automatic) void this.#admin?.view.refresh(); return; }
    if (this.#mode === "federation" && this.#canFederation() && !this.#federationForbidden) void this.#loadFederation(undefined, automatic);
    else if (this.#mode === "workers" && this.#canWorkers() && !this.#workerForbidden) void this.#loadWorkers(undefined, undefined, automatic);
    else if (this.#mode === "brokers" && this.#canBrokers() && !this.#brokerForbidden) void this.#loadBrokers(undefined, undefined, automatic);
    else if (this.#mode === "resources" && this.#canResources()) void this.#loadResources();
    else if (this.#mode === "events" && this.#selection && this.#canEvents()) void this.#loadEvents();
  }
  #refreshReading() {
    return this.#identityLoading || this.#delegatedChange.phase === "sending" ||
      this.#mode === "supervision" && this.#supervision?.reading === true ||
      this.#mode === "journal" && this.#journal?.reading === true || this.#mode === "overview" && this.#overview?.reading === true ||
      this.#mode === "sessions" && this.#sessionLaunch?.reading === true ||
      isAdminMode(this.#mode) && this.#admin?.view.reading === true;
  }
  #updateRefreshState() {
    this.#navigation?.querySelector('[data-focus-key="human-refresh"]')?.setAttribute("aria-disabled", String(this.#refreshReading()));
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
    this.#notice.hidden = !this.#message;
    if (this.#notice.textContent !== this.#message) this.#notice.textContent = this.#message;
    const changeSending = this.#delegatedChange.phase === "sending";
    const destinations: DashboardMode[] = ["contexts", ...(this.#overviewState() ? ["overview" as const] : []), ...ADMIN_MODES.filter((mode) => this.#adminState(mode)), ...(this.#canSupervise() ? ["supervision" as const] : []), ...(this.#canJournal() ? ["journal" as const] : []), ...(this.#canSessionLaunch() ? ["sessions" as const] : []), "resources", "events", ...(this.#canBrokers() ? ["brokers" as const] : []), ...(this.#canWorkers() ? ["workers" as const] : []), ...(this.#canFederation() ? ["federation" as const] : [])];
    const nav = destinations.map((mode) => {
      const label = { contexts: "Contextes", overview: this.#context?.mode === "administration" ? "Vue 360°" : "Mon espace",
        memberships: "Utilisateurs", usage: "Quotas et usage", configuration: "Configuration", supervision: "Supervision", journal: "Journal", sessions: "Nouvelle session", resources: "Ressources", events: "Événements", brokers: "Brokers", workers: "Workers", federation: "Fédération" }[mode];
      const node = action(label, `human-nav-${mode}`, () => this.#openView(mode),
        changeSending || mode === "resources" && !this.#canResources() || mode === "events" && (!this.#selection || !this.#canEvents()));
      node.className = "button human-tab"; if (mode === this.#mode) node.setAttribute("aria-current", "page"); return node;
    });
    // The destination row may scroll sideways: keep the user's position, bring a newly opened destination into
    // view, and bring into view whichever destination takes the keyboard focus.
    const scrolled = this.#navigation.querySelector<HTMLElement>(".human-nav")?.scrollLeft ?? 0;
    const row = el("div", "human-nav", ...nav);
    row.addEventListener("focusin", (event) => { if (event.target instanceof HTMLElement) event.target.scrollIntoView({ block: "nearest", inline: "nearest" }); });
    row.addEventListener("scroll", () => this.#markNavigation(), { passive: true });
    // While the accesses or the unified view are read, « Actualiser » stays focusable: unavailable, it ignores
    // activation and keeps the focus, so that the same key asks again once that read ended.
    const refresh = action("Actualiser", "human-refresh", () => {
      if (this.#refreshReading()) return;
      if (this.#mode === "contexts") void this.#loadIdentity();
      else if (this.#resourceStreamAwaitingAccess()) this.#visibility();
      else this.#refreshActive(false);
    }, this.#api.readBudgetStopped || (this.#mode === "brokers" && (this.#brokers.loading || this.#brokerForbidden) || this.#mode === "workers" && (this.#workers.loading || this.#workerForbidden) || this.#mode === "federation" && (this.#federation.loading || this.#federationForbidden)));
    refresh.setAttribute("aria-disabled", String(this.#refreshReading()));
    this.#navigation.replaceChildren(row, refresh);
    row.scrollLeft = scrolled;
    if (this.#navigation.dataset.mode !== this.#mode) row.querySelector<HTMLElement>('[aria-current="page"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
    this.#navigation.dataset.mode = this.#mode; this.#markNavigation();
    const heading = el("h1", "", { contexts: "Choisir un contexte.", overview: this.#context?.mode === "administration" ? "Vue 360° du tenant." : "Mon espace.",
      memberships: "Utilisateurs de l’organisation.", usage: "Quotas et usage du tenant.", configuration: "Configuration effective.", supervision: this.#context?.mode === "administration" ? "Supervision du tenant." : "Supervision de mon espace.", journal: "Journal des événements autorisés.", sessions: "Créer une session IA.", resources: "Ressources autorisées.", events: "Événements autorisés.", brokers: "Brokers enregistrés du tenant.", workers: "Workers enregistrés du tenant.", federation: "Workers et brokers du tenant." }[this.#mode]);
    heading.tabIndex = -1;
    const sections = [el("div", "page-heading", el("p", "eyebrow", this.#identity?.authentication.method === "oidc_access_token" ?
      "COMPTE OIDC / CONSULTATION NOMINATIVE" : "COMPTE LOCAL / CONSULTATION NOMINATIVE"), heading,
      el("p", "page-description", this.#mode === "journal" ? this.#journalDescription() :
        this.#mode === "memberships" ? "Appartenances de l’organisation que ce contexte administre. La création de comptes et la gestion des identifiants restent à l’opérateur." :
        this.#mode === "usage" ? "Politique de quota du tenant, réservations durables et allocations terminales observées. Ni consommation mesurée, ni prix, ni facture." :
        this.#mode === "configuration" ? "Configuration effective du contrôle, axe par axe, avec sa provenance. Types et modes seulement : ni adresse, ni chemin, ni secret." :
        this.#mode === "overview" ? (this.#context?.mode === "administration" ?
        "Synthèse de tout ce que ce contexte d’administration peut lire dans le tenant, en un seul instantané du contrôle. Une section non accordée ou indisponible le dit ; le navigateur ne compte rien lui-même." :
        "Synthèse de vos ressources et de celles partagées avec vous, en un seul instantané du contrôle. Une section non accordée ou indisponible le dit ; le navigateur ne compte rien lui-même.") :
        this.#mode === "supervision" ? (this.#context?.mode === "administration" ?
        "Brokers, workers et ressources que ce contexte d’administration peut lire, réunis sur une page. Chaque valeur vient d’une lecture autorisée par le contrôle ; ce qui n’a pas été lu reste inconnu." :
        "Les ressources que vous possédez ou qui vous sont partagées, et ce que ce contexte personnel ne permet pas de lire. Chaque valeur vient d’une lecture autorisée par le contrôle.") : this.#mode === "federation" ? "Vue d’administration offerte par l’opération federation_read de ce contexte. Workers et brokers enregistrés restent visibles dans deux pages indépendantes du même tenant." : this.#mode === "workers" ? "Workers de tout le tenant · opération workers_read de ce contexte. États du contrôle et observations runtime restent distincts." : this.#mode === "brokers" ? "Infrastructure de tout le tenant · opération brokers_read de ce contexte. Les permissions et l’évaluation du registre proviennent du contrôle." :
        this.#mode === "sessions" ? "Préparation retenue puis lancement de l’intention exacte, offerts par les opérations session_launch_prepare et session_launch de ce contexte. Le contrôle résout la cible et autorise chaque étape ; un lancement accepté n’est pas un worker en cours d’exécution." :
        "Les permissions sont vérifiées par le serveur. Une ressource visible ne donne aucun droit implicite sur son contenu ou son export.")),
      this.#renderDelegatedRecovery() ?? "",
      ...(this.#api.readBudgetStopped ? [el("div", "control-error human-budget-stop", el("p", "", "Lecture automatique suspendue : budget durable de lecture atteint. Contactez l’opérateur ; changer de vue ou de contexte ne réinitialise pas ce budget."),
        action("Relire après intervention de l’opérateur", "human-operator-retry", () => {
          if (this.#delegatedChange.phase === "sending") return;
          this.#api.allowReadAfterOperatorAction();
          void this.#refreshAfterOperatorAction();
        }, changeSending || this.#identityLoading || this.#brokers.loading || this.#workers.loading || this.#federation.loading || this.#resources.loading || this.#events.loading))] : []),
      this.#context ? el("div", "info-strip human-context-summary", icon("shield"), el("div", "",
        el("strong", "", `${modeName(this.#context)} · ${this.#context.tenant_id}`),
        el("p", "", `Organisation : ${this.#context.organization_scope}`),
        el("p", "", `Contexte valable jusqu’au ${formatDate(this.#context.expires_at, true)}. Source : identité authentifiée du contrôle.`))) : "",
      this.#mode === "supervision" || this.#mode === "journal" || this.#mode === "sessions" ? "" : this.#mode === "overview" ? (this.#overview ? "" : this.#renderUnavailable("Synthèse indisponible pour ce contexte", ["summary_read"], "human-overview-access")) :
        isAdminMode(this.#mode) ? (this.#admin ? "" : this.#renderUnavailable({ memberships: "Utilisateurs indisponibles pour ce contexte", usage: "Quotas et usage indisponibles pour ce contexte",
          configuration: "Configuration indisponible pour ce contexte" }[this.#mode], ADMIN_DESTINATIONS[this.#mode], `human-${this.#mode}-access`)) : this.#mode === "contexts" ? this.#renderContexts() : this.#mode === "resources" ? this.#renderResources() : this.#mode === "brokers" ? this.#renderBrokers() : this.#mode === "workers" ? this.#renderWorkers() : this.#mode === "federation" ? this.#renderFederation() : this.#renderEvents()];
    const live = this.#mode === "supervision" ? this.#supervision?.element : this.#mode === "journal" ? this.#journal?.element :
      this.#mode === "overview" ? this.#overview?.element : this.#mode === "sessions" ? this.#sessionLaunch?.element : isAdminMode(this.#mode) ? this.#admin?.view.element : undefined;
    if (live && live.parentElement === this.#page) {
      // The unified view keeps its own element across dashboard renders, so typed filters keep focus and caret.
      while (this.#page.firstChild && this.#page.firstChild !== live) this.#page.firstChild.remove();
      live.before(...sections);
    } else this.#page.replaceChildren(...sections, ...(live ? [live] : []));
    if (this.#focusResume) {
      const resume = this.#page.querySelector<HTMLElement>('[data-focus-key="human-resume"]:not(:disabled)');
      if (resume) { this.#focusResume = false; resume.focus({ preventScroll: true }); return; }
    }
    if (active?.isConnected && live?.contains(active)) return;
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
    // The stop notice of the dashboard follows the budget of the client: render again once the read settled.
    if (this.#mode === "supervision" && this.#supervision) { await this.#supervision.refresh(); this.#render(); return; }
    if (this.#mode === "journal" && this.#journal) { await this.#journal.reload(); this.#render(); return; }
    // Without a served summary there is nothing to read again here: only the accesses, which may now serve it.
    if (this.#mode === "overview") { if (this.#overview) await this.#overview.refresh(); else await this.#loadIdentity(true); this.#render(); return; }
    if (this.#mode === "sessions") { await this.#sessionLaunch?.refresh(); this.#render(); return; }
    if (isAdminMode(this.#mode)) { if (this.#admin) await this.#admin.view.refresh(); else await this.#loadIdentity(true); this.#render(); return; }
    if (this.#mode === "federation" && this.#canFederation() && !this.#federationForbidden) {
      await this.#loadFederation(undefined, false, true); return;
    }
    if (this.#mode === "workers" && this.#canWorkers() && !this.#workerForbidden) {
      await this.#loadWorkers(undefined, undefined, false, true); return;
    }
    if (this.#mode === "brokers" && this.#canBrokers() && !this.#brokerForbidden) {
      await this.#loadBrokers(undefined, undefined, false, true); return;
    }
    if (this.#resourceStreamAwaitingAccess()) { await this.#revalidateVisibleContext(true); return; }
    const generation = this.#generation; const mode = this.#mode;
    const success = mode === "resources" && this.#canResources() ? await this.#loadResources(undefined, undefined, true) :
      mode === "events" && this.#selection && this.#canEvents() ? await this.#loadEvents(undefined, undefined, true) : null;
    if (success && !this.#closed && generation === this.#generation && mode === this.#mode && !this.#api.readBudgetStopped && this.#canResources()) {
      this.#startStream(); return;
    }
    if (success !== null || this.#closed) return;
    // A refused destination must regain a current offered context before another page read.
    this.#api.clearContext(); this.#purgeContext(this.#delegatedChange.phase === "unknown");
    this.#mode = "contexts"; this.#render();
    await this.#loadIdentity(true);
  }
  async #loadIdentity(operatorRetry = false): Promise<boolean | undefined> {
    if (this.#closed || this.#identityLoading || this.#api.readBudgetStopped && !operatorRetry ||
        this.#mode === "contexts" && this.#delegatedChange.phase === "sending") return;
    this.#delegatedChange.withdrawOffer();
    this.#identityLoading = true; this.#render(); const generation = this.#generation; const read = ++this.#identitySequence;
    let abandoned = false;
    try {
      const identity = await this.#api.identity();
      if (this.#closed || generation !== this.#generation && this.#context) return;
      if (generation !== this.#generation && this.#identityOffersWithdrawn) throw new HumanSuperseded();
      if (this.#identityError && this.#message === IDENTITY_REVALIDATION_FAILED + this.#identityError) this.#message = "";
      this.#identity = identity; this.#identityError = ""; this.#identityOffersWithdrawn = false;
      const review = this.#delegatedChange.revalidate(identity, Date.now());
      if (review === "withdrawn") this.#message = DELEGATED_ACCESS_CHANGED;
      else if (review === "identity_changed") this.#message = "";
      this.#evaluateResume();
      const offer = this.#delegatedChange.offer(Date.now());
      if (offer) {
        clearTimeout(this.#offerTimer);
        this.#offerTimer = setTimeout(() => this.#render(), Math.max(0, Date.parse(offer.expiresAt) - Date.now()) + 25);
      }
      return true;
    } catch (error) {
      // Losing the context aborts the reads in flight, this one included: an abort or a superseded read after a purge is not a failure to report.
      abandoned = error instanceof HumanSuperseded || generation !== this.#generation;
      if (!this.#closed && !abandoned) {
        if (error instanceof HumanApiError && error.status === 503) this.#delegatedChange.noteUnavailable();
        if (error instanceof HumanApiError && error.status === 403) {
          this.#api.clearContext(); this.#purgeContext(); this.#mode = "contexts";
          this.#identityError = failure(error); this.#message = "Revalidation des accès refusée. Les données du contexte ont été retirées ; relisez vos accès.";
        } else {
          this.#identityError = failure(error); this.#resources.stale = true; this.#events.stale = true; this.#brokers.stale = true; this.#workers.stale = true; this.#federation.stale = true;
          this.#message = this.#context ? IDENTITY_REVALIDATION_FAILED + this.#identityError : this.#message;
        }
      }
      if (!abandoned) return false;
    }
    // Only the latest identity read owns the loading state: an abandoned one must not release it for its successor.
    // A read answered after the person opened another destination releases it too, or « Actualiser » and every later revalidation would stay blocked.
    finally { if (!this.#closed && read === this.#identitySequence) { this.#identityLoading = false; this.#render(); } }
    // Abandoned: the context was lost through another read while this one was in flight. When no later
    // identity read took over and no context is selected, the list of contexts would stay empty: read once more.
    return !this.#closed && read === this.#identitySequence && !this.#context && !this.#api.readBudgetStopped &&
      !this.#delegatedChange.unavailable ? this.#loadIdentity() : false;
  }
  #renderDelegatedRecovery() {
    const kind = this.#delegatedChange.kind;
    if (this.#delegatedChange.phase !== "unknown" || !kind) return null;
    const choosing = this.#mode === "contexts";
    const offer = choosing && !this.#identityLoading && !this.#api.readBudgetStopped ? this.#delegatedChange.offer(Date.now()) : null;
    const focusKey = `human-${kind}-recovery`;
    const title = kind === "grant" ? "Reprise du changement de partages" : "Reprise du changement d’appartenance";
    const status = el("section", "panel", el("h2", "", title),
      el("p", "", "Tant que cette reprise est en attente, les modifications des appartenances et des partages sont indisponibles."));
    if (!choosing) {
      status.append(action("Voir les contextes pour la reprise", `${focusKey}-contexts`, () => this.#openView("contexts")));
    } else if (!offer) {
      status.append(el("p", "", this.#delegatedChange.unavailable ?
        "Le contrôle ou son audit était indisponible. Un rafraîchissement des accès peut constater la remise en service. Si la panne persiste, contactez l’opérateur." :
        "Relisez vos accès pour vérifier si un contexte d’administration équivalent permet une reprise."));
      if (!this.#api.readBudgetStopped && !this.#identityError)
        status.append(action("Relire les accès", `${focusKey}-access`, () => { void this.#loadIdentity(); }, this.#identityLoading));
    } else {
      const revision = `Révision attendue ${offer.expectedRevision}`;
      status.append(el("p", "muted", "Votre confirmation renvoie la demande d’origine."));
      if (offer.kind === "grant") {
        status.append(el("p", "", `${revision} · Partages demandés ${offer.shareCount}`));
      } else {
        status.append(el("p", "", `${revision} · `, offer.active ? "Appartenance active" : "Appartenance désactivée"),
          el("p", "", `Capacités demandées — ${offer.capabilities.map((capability) => CAPABILITY_LABELS[capability]).join(", ") || "aucune"}`));
      }
      status.append(action(`Réessayer la même opération · tenant ${offer.tenant} · organisation ${offer.organization}`,
        focusKey, () => { void this.#retryDelegatedChange(); }));
    }
    status.append(el("p", "muted", "L’abandon retire la reprise de cette session. Tout effet déjà enregistré par le contrôle est conservé."),
      action("Abandonner la reprise", `${focusKey}-abandon`, () => {
        if (this.#delegatedChange.abandon()) this.#withdrawDelegatedContext("Résultat de la demande précédente toujours inconnu");
      }));
    return status;
  }
  #selectOfferedContext(id: string) {
    const selected = this.#delegatedChange.selectContext(this.#identity?.principal, () => this.#api.selectContext(id));
    if (!selected) return null;
    const { context, keepChange } = selected;
    this.#purgeContext(keepChange); this.#context = context;
    this.#message = keepChange ? DELEGATED_OUTCOME_UNKNOWN : "";
    return context;
  }
  #renderContexts() {
    const contexts = this.#identity?.contexts ?? [];
    const changeSending = this.#delegatedChange.phase === "sending";
    const contextChoiceDisabled = this.#identityLoading || changeSending;
    const complete = this.#identityOffersWithdrawn ? undefined : this.#identity?.contexts_complete;
    const partial = complete === false;
    const coverage = partial ? el("div", "control-status",
      badge("Liste des contextes partielle", "amber"),
      el("p", "", "Certaines sources ne peuvent pas être consultées. D’autres contextes peuvent manquer. Réessayez pour actualiser la liste."),
      action("Actualiser les contextes", "human-contexts-retry", () => { void this.#loadIdentity(); }, contextChoiceDisabled)) : null;
    coverage?.setAttribute("role", "status");
    const offer = this.#resume && this.#resumeOffer ? this.#resumeOffer : undefined;
    // The offer names what really happened, and stays usable after the offered context has itself expired.
    const lost = this.#resume?.cause === "session_rotated" ? "Votre session a été renouvelée et les données affichées ont été retirées." :
      "Le contexte précédent a expiré et ses données ont été retirées.";
    // An identity read temporarily prevents activation without moving focus.
    const resumed = this.#resume?.view === "journal" ? "le journal" : this.#resume?.view === "overview" ? offer?.mode === "administration" ? "la vue 360°" : "mon espace" : "la supervision";
    const confirm = offer ? action(`Reprendre ${resumed} · ${modeName(offer).toLowerCase()} · ${offer.tenant_id}`, "human-resume",
      () => { if (!this.#identityLoading) this.#acceptResume(); }, changeSending) : null;
    confirm?.setAttribute("aria-disabled", String(contextChoiceDisabled));
    const resume = offer ? el("div", "control-status human-resume",
      el("p", "", Date.parse(offer.expires_at) <= Date.now() ?
        `${lost} Le contexte proposé pour la reprise (${modeName(offer)} · ${offer.tenant_id}) a expiré à son tour. « Reprendre » relit d’abord vos accès, puis rouvre la vue si le contrôle propose encore un contexte de mêmes droits ; vos filtres seront rétablis.` :
        `${lost} Le contrôle ne renouvelle pas un contexte : il en propose un nouveau, de mêmes droits (${modeName(offer)} · ${offer.tenant_id}). Aucune donnée n’est relue avant votre confirmation ; vos filtres seront rétablis.`),
      confirm) : null;
    resume?.setAttribute("role", "status");
    return el("section", "human-contexts", resume,
      el("p", "muted", `Identité authentifiée : ${this.#identity?.display_name ?? "Inconnue"}. Les contextes personnel et administration sont proposés par le serveur selon vos appartenances.`),
      this.#identityLoading ? el("p", "control-status", "Lecture des accès…") : "",
      this.#identityError ? this.#error(this.#identityError, () => { void this.#loadIdentity(); }) : "",
      coverage,
      !this.#context && !contexts.length && complete === true && !this.#identityLoading && !this.#identityError ? empty("Aucun contexte autorisé", "Votre compte est authentifié mais aucun contexte actif n’est proposé. Faites vérifier vos appartenances par l’opérateur.") : "",
      ...contexts.map((context, index) => el("article", "panel human-context-card",
        el("h2", "", `${modeName(context)} · ${context.tenant_id}`), el("p", "muted", `Organisation : ${context.organization_scope}`),
        el("p", "muted", `Valable jusqu’au ${formatDate(context.expires_at, true)}`),
        el("p", "mono", `Capacités serveur : ${context.capabilities.join(", ") || "aucune"}`),
        action(`Ouvrir ${modeName(context).toLowerCase()} · ${context.tenant_id}`, `context-${index}`, () => {
          try {
            const selected = this.#selectOfferedContext(context.id);
            if (!selected) return;
            const landing = landingDestination(selected);
            if (landing) this.#openView(landing);
            else { this.#mode = "contexts"; this.#message = "Ce contexte n’offre aucune opération de consultation disponible dans ce build. Faites vérifier vos accès."; this.#render(); }
          } catch (error) { this.#identityError = failure(error); this.#render(); }
        }, contextChoiceDisabled || Date.parse(context.expires_at) <= Date.now()),
        supervisionAccess(context).any ? action(`Superviser ${modeName(context).toLowerCase()} · ${context.tenant_id}`, `context-supervise-${index}`, () => {
          try {
            if (this.#selectOfferedContext(context.id)) this.#openView("supervision");
          } catch (error) { this.#identityError = failure(error); this.#render(); }
        }, contextChoiceDisabled || Date.parse(context.expires_at) <= Date.now()) : null)),
      el("p", "field-help", "La vue personnelle des workers reste à raccorder à une chaîne authentifiée d’initiation et de délégation. Une référence de session déclarée ne prouve pas la propriété humaine."),
      el("p", "field-help", "La déconnexion efface ce jeton de l’onglet. Sa révocation et ses permissions restent contrôlées côté serveur."));
  }
  #error(message: string, retry: () => void) {
    const node = el("div", "control-error", icon("alert"), el("p", "", message), this.#api.readBudgetStopped ? null : action("Réessayer", "human-retry", retry));
    node.setAttribute("role", "alert"); return node;
  }
  #status<T extends { read_receipt: { received_at: string } }>(state: ReadState<T>, formatTime: (value: string, detailed: boolean) => string = formatDate) {
    const stale = state.stale || !!state.loadedAt && Date.now() - Date.parse(state.loadedAt) > 30000;
    const node = el("div", "control-status", badge(state.loading ? "Lecture en cours…" : state.error ? state.value ? "Données périmées · dernière lecture conservée" : "Données indisponibles" : stale ? "Données à actualiser" : state.value ? "Lecture réussie" : "Lecture requise", state.error || stale ? "amber" : "neutral"),
      el("span", "muted", state.loadedAt ? `Lu le ${formatTime(state.loadedAt, true)}` : "Aucune lecture réussie"),
      state.value ? el("span", "muted", `Reçu du contrôle : ${formatTime(state.value.read_receipt.received_at, true)}`) : null);
    node.setAttribute("role", "status"); return node;
  }
  #pagination<T extends { items: unknown[]; has_more: boolean; next_cursor: string | null }>(state: ReadState<T>, load: (cursor: string | null, history: (string | null)[]) => void, key = "human", label?: string) {
    const blocked = state.loading || this.#api.readBudgetStopped;
    const node = el("div", "table-footer control-pagination", el("span", "control-page-summary",
      state.value ? `${state.value.items.length} résultat(s) sur cette page · page ${state.history.length + 1}${state.value.has_more || state.history.length ? " · lecture partielle" : ""}` : "Sélection autorisée inconnue"),
      el("div", "control-page-actions",
        state.history.length || state.error ? action("Première page", `${key}-first`, () => load(null, []), blocked) : null,
        state.history.length ? action("Page précédente", `${key}-previous`, () => load(state.history.at(-1) ?? null, state.history.slice(0, -1)), blocked) : null,
        state.value?.next_cursor ? action("Page suivante", `${key}-next`, () => load(state.value?.next_cursor ?? null, [...state.history, state.cursor]), blocked || state.history.length >= 100) :
          state.value ? el("span", "muted", "Fin de cette sélection à cette lecture") : null));
    if (label) { node.setAttribute("role", "group"); node.setAttribute("aria-label", label); }
    return node;
  }
  async #loadFederation(position?: { leg: FederationLeg; cursor: string | null; history: (string | null)[] }, automatic = false, operatorRetry = false) {
    if (!this.#canFederation() || this.#mode !== "federation" || this.#closed || this.#federation.loading || this.#federationForbidden ||
      this.#api.readBudgetStopped && !operatorRetry || automatic && (document.visibilityState !== "visible" || Date.now() - this.#federationLastLaunch < 30000)) return;
    const state = this.#federation; const generation = this.#generation; const request = this.#federationGeneration;
    const positions = { ...this.#federationPositions, ...(position ? { [position.leg]: { cursor: position.cursor, history: [...position.history] } } : {}) };
    this.#federationLastLaunch = Date.now(); state.loading = true; this.#render();
    try {
      const page = await this.#api.federationWorkers({ workerCursor: positions.workers.cursor, brokerCursor: positions.brokers.cursor }, this.#federationAbort.signal);
      if (this.#closed || generation !== this.#generation || request !== this.#federationGeneration || this.#mode !== "federation" || !this.#canFederation()) return;
      this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
      // Publish both checked legs together. Neither a failed read nor a late callback moves one position alone.
      this.#federationPositions = positions;
      state.value = page; state.loadedAt = new Date().toISOString(); state.error = ""; state.stale = false;
    } catch (error) {
      if (this.#closed || generation !== this.#generation || request !== this.#federationGeneration || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) {
        this.#purgeFederation(); this.#purgeWorkers(); this.#purgeBrokers();
        this.#federationForbidden = true; this.#federationRefusal = failure(error); this.#federation.error = this.#federationRefusal; this.#federation.stale = true;
      } else { state.error = failure(error); state.stale = true; }
    } finally {
      state.loading = false;
      if (!this.#closed && generation === this.#generation && this.#mode === "federation") this.#render();
    }
  }
  #renderFederation() {
    const state = this.#federation;
    const workers: ReadState<HumanWorkerPage> = { ...state, ...this.#federationPositions.workers, value: state.value?.workers ?? null };
    const brokers: ReadState<HumanBrokerPage> = { ...state, ...this.#federationPositions.brokers, value: state.value?.brokers ?? null };
    const description = el("div", "info-strip human-federation-coverage", icon("shield"), el("div", "",
      badge("Couverture partielle", "amber"), el("p", "", "Source tenant_federation_registry · independent_snapshots · inventaire runtime inconnu."),
      el("p", "", "Les deux registres sont évalués séparément. Chaque pagination relit les deux pages avec leurs positions et reçus propres. Aucun total du tenant ni snapshot global n’est déduit."),
      el("p", "", "Les brokers enregistrés restent affichés sans worker associé sur la page courante. Un heartbeat non reçu, un bail périmé ou révoqué ne prouve ni absence d’activité, ni arrêt d’un worker, ni libération de quota.")));
    return el("div", "human-federation", description,
      this.#federationForbidden ? el("div", "control-error", el("p", "", this.#federationRefusal), action("Relire mes accès", "human-federation-access", () => this.#openView("contexts"))) :
        state.error ? this.#error(state.error, () => { void this.#loadFederation(); }) : null,
      this.#federationForbidden ? empty("Fédération interdite", "Les deux pages, détails et positions de lecture ont été retirés. Relisez vos accès puis sélectionnez un contexte actuellement autorisé.") :
        el("div", "human-federation-registries", this.#renderWorkers(workers, true), this.#renderBrokers(brokers, true)),
      el("p", "human-broker-footnote muted", "Chaque reçu durable décrit une lecture enregistrée ; il ne prouve ni livraison ni exécution. Une erreur refuse toute la nouvelle composition. Après une erreur de connexion, la dernière lecture autorisée reste datée et périmée jusqu’à une reprise réussie."));
  }
  async #loadBrokers(cursor = this.#brokers.cursor, history = this.#brokers.history, automatic = false, operatorRetry = false) {
    if (!this.#canBrokers() || this.#mode !== "brokers" || this.#closed || this.#brokers.loading || this.#brokerForbidden ||
      this.#api.readBudgetStopped && !operatorRetry || automatic && (document.visibilityState !== "visible" || Date.now() - this.#brokerLastLaunch < 30000)) return;
    const state = this.#brokers; const generation = this.#generation; const request = this.#brokerGeneration;
    this.#brokerLastLaunch = Date.now(); state.loading = true; this.#render();
    try {
      const page = await this.#api.brokers(cursor, this.#brokerAbort.signal);
      if (this.#closed || generation !== this.#generation || request !== this.#brokerGeneration || this.#mode !== "brokers" || !this.#canBrokers()) return;
      const open = this.#dialog; const row = page.items.find((item) => item.broker_id === open?.dataset.brokerId);
      // A heartbeat refresh keeps the dated detail and its focus. Remove it when its broker leaves the page
      // or its grant/incarnation/disclosed authority metadata changes; context withdrawal purges it earlier.
      if (open?.dataset.brokerId && (!row || open.dataset.brokerDisclosure !== JSON.stringify([row.incarnation_id, row.grant_revision, row.authorities]))) {
        open.close(); open.remove(); this.#dialog = null;
      }
      state.value = page; state.cursor = cursor; state.history = [...history]; state.loadedAt = new Date().toISOString(); state.error = ""; state.stale = false;
    } catch (error) {
      if (this.#closed || generation !== this.#generation || request !== this.#brokerGeneration || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) {
        this.#purgeBrokers(); this.#brokerForbidden = true; this.#brokers.error = failure(error); this.#brokers.stale = true; this.#workers.stale = true;
      } else { state.error = failure(error); state.stale = true; }
    } finally {
      state.loading = false;
      if (!this.#closed && generation === this.#generation && this.#mode === "brokers") this.#render();
    }
    // A broker asked from the summary opens once, after this read, when it is on the page the control returned.
    const pending = this.#pendingBroker;
    if (pending && !this.#closed && generation === this.#generation && request === this.#brokerGeneration && this.#mode === "brokers") {
      this.#pendingBroker = null;
      const row = this.#brokers.value?.items.find((item) => item.broker_id === pending);
      if (row) this.#showBroker(row);
      else if (this.#brokers.value) { this.#message = `Le broker ${pending} ne figure pas sur cette page du registre : parcourez les pages suivantes.`; this.#render(); }
    }
  }
  async #loadWorkers(cursor = this.#workers.cursor, history = this.#workers.history, automatic = false, operatorRetry = false) {
    if (!this.#canWorkers() || this.#mode !== "workers" || this.#closed || this.#workers.loading || this.#workerForbidden ||
      this.#api.readBudgetStopped && !operatorRetry || automatic && (document.visibilityState !== "visible" || Date.now() - this.#workerLastLaunch < 30000)) return;
    const state = this.#workers; const generation = this.#generation; const request = this.#workerGeneration;
    this.#workerLastLaunch = Date.now(); state.loading = true; this.#render();
    try {
      const page = await this.#api.workers(cursor, this.#workerAbort.signal);
      if (this.#closed || generation !== this.#generation || request !== this.#workerGeneration || this.#mode !== "workers" || !this.#canWorkers()) return;
      this.#dialog?.close(); this.#dialog?.remove(); this.#dialog = null;
      state.value = page; state.cursor = cursor; state.history = [...history]; state.loadedAt = new Date().toISOString(); state.error = ""; state.stale = false;
    } catch (error) {
      if (this.#closed || generation !== this.#generation || request !== this.#workerGeneration || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) {
        this.#purgeWorkers(); this.#workerForbidden = true; this.#workers.error = failure(error); this.#workers.stale = true;
      } else { state.error = failure(error); state.stale = true; }
    } finally {
      state.loading = false;
      if (!this.#closed && generation === this.#generation && this.#mode === "workers") this.#render();
    }
  }
  #workerAssociation(value: HumanWorkerAssociation, label: string) {
    const broker = value.broker;
    const status = { registry_unavailable: "Registre indisponible", authority_unknown: "Autorité inconnue", unregistered: "Autorité non enregistrée", registered: "Association historique" }[value.status];
    return el("div", "", el("strong", "", label), el("p", "muted", status), broker ? el("p", "mono control-id", broker.broker_id) : null,
      broker ? el("p", "muted", `${brokerStateName[broker.state]} · incarnation courante ${broker.current_incarnation_id}`) : null,
      broker ? el("p", "muted", `Runtime ${value.runtime_kind} · correspondance ${value.runtime_matches ? "oui" : "non"} · dans le grant courant ${value.in_current_grant ? "oui" : "non"}`) : null);
  }
  #renderWorkers(state = this.#workers, federation = false) {
    const table = el("table", "human-broker-table");
    table.append(el("caption", "", "Métadonnées du registre durable · cette page n’est ni un total du tenant ni un inventaire runtime complet."),
      el("thead", "", el("tr", "", ...["Worker / session déclarée", "État du contrôle", "Dernière observation runtime", "Associations historiques", "Réservation", "Détail"].map((title) => el("th", "", title)))));
    const rows = state.value?.items.map((item) => el("tr", "",
      el("td", "", el("p", "mono control-id", item.worker_id), el("p", "muted", `Session déclarée : ${item.session_id}`), el("p", "muted", `Révision ${item.revision} · ${item.runtime_kind}`)),
      el("td", "", badge(item.state), item.allocation_inconsistent ? badge("Allocation incohérente", "amber") : null),
      el("td", "", item.last_observation ? el("div", "", badge(`Observé : ${item.last_observation.status}`),
        el("p", "muted", `Relevé : ${workerTimeLabel(item.last_observation.observed_at, item.last_observation.observed_at_status)}`),
        el("p", "muted", `Début fourni : ${workerTimeLabel(item.last_observation.started_at, item.last_observation.started_at_status)}`),
        el("p", "muted", `Fin fournie : ${workerTimeLabel(item.last_observation.finished_at, item.last_observation.finished_at_status)}`),
        el("p", "mono", `Code de sortie : ${item.last_observation.exit_code ?? "Non fourni"}`)) : el("p", "muted", "Observation inconnue")),
      el("td", "", this.#workerAssociation(item.launch_broker, "Autorité du lancement"), this.#workerAssociation(item.observation_broker, "Autorité de l’inspection")),
      el("td", "", badge(item.quota_held ? "Quota réservé" : "Quota non réservé"), el("p", "muted", `${item.cpu_millis} milliCPU · ${item.memory_bytes} octets alloués`), el("p", "muted", "Consommation réelle inconnue · non facturable")),
      el("td", "", action("Détails", `human-worker-${item.worker_id}`, () => this.#showWorker(item))))) ?? [];
    table.append(el("tbody", "", ...rows));
    const scroll = el("div", "human-broker-scroll", table); scroll.tabIndex = 0; scroll.setAttribute("role", "region"); scroll.setAttribute("aria-label", "Tableau des workers autorisés, défilement horizontal");
    return el("section", "panel human-broker-panel", el("div", "panel-heading", el("div", "", el("h2", "", "Registre des workers"),
      el("p", "", "Source : contrôle du tenant authentifié. Une référence de session déclarée n’attribue aucun propriétaire humain."))),
      this.#status(state), state.value ? el("p", "human-broker-evaluation muted", `Évaluation serveur : ${formatDate(state.value.evaluated_at, true)} · source tenant_worker_broker_registry · snapshot par page, pages successives vivantes.`) : null,
      federation ? null : this.#workerForbidden ? el("div", "control-error", el("p", "", state.error), action("Relire mes accès", "human-worker-access", () => this.#openView("contexts"))) :
        state.error ? this.#error(state.error, () => { void this.#loadWorkers(); }) : null,
      !state.value ? empty(state.loading ? "Chargement des workers…" : "Workers inconnus", "La lecture doit réussir avant de conclure sur l’activité.") :
        !state.value.items.length ? empty("Aucun worker sur cette page", "Le registre n’a retourné aucune ligne pour cette lecture autorisée. L’inventaire runtime reste inconnu.") : scroll,
      this.#pagination(state, (cursor, history) => { if (federation) void this.#loadFederation({ leg: "workers", cursor, history }); else void this.#loadWorkers(cursor, history); }, federation ? "human-federation-workers" : "human", federation ? "Pages des workers" : undefined),
      el("p", "human-broker-footnote muted", "Une date connue est une valeur interprétable fournie, sans preuve de chronologie cohérente ou de fin réelle. Aucune durée n’est calculée depuis ces dates. Une date inconnue reste distincte d’une valeur non fournie."),
      el("p", "human-broker-footnote muted", "Une association historique d’autorité peut être postérieure au fait worker ; elle ne prouve pas le broker physique ayant exécuté ou inspecté la session. Le bail du broker ne prouve aucun état du worker."),
      el("p", "human-broker-footnote muted", "Actualisation au plus toutes les 30 secondes lorsque cette vue est visible. Aucun flux worker implicite ni délai garanti de notification. La vue personnelle reste un écart obligatoire tant que sa chaîne d’attribution authentifiée manque."));
  }
  #showWorker(item: HumanWorker) {
    const federation = this.#mode === "federation";
    const page = federation ? this.#federation.value?.workers : this.#workers.value;
    if (federation ? !this.#canFederation() || this.#federationForbidden : !this.#canWorkers() || this.#mode !== "workers" || this.#workerForbidden) return;
    if (!page?.items.some((row) => row.worker_id === item.worker_id)) return;
    this.#dialog?.close(); this.#dialog?.remove();
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const heading = el("h2", "", "Métadonnées du worker"); heading.id = "human-worker-detail-title";
    const dialog = el("dialog", "detail-dialog"); dialog.setAttribute("aria-labelledby", heading.id);
    const close = button("", "icon-button", () => dialog.close(), "x"); close.setAttribute("aria-label", "Fermer les détails");
    const data = el("pre", "code-block"); data.textContent = JSON.stringify(item, null, 2);
    dialog.append(el("div", "detail-header", heading, close), el("div", "detail-body",
      el("p", "muted", `Projection sans commande ni credential · lecture du ${formatDate(page.evaluated_at, true)}. Reçu durable ${page.read_receipt.audit_event_id} ; il ne prouve ni livraison ni effet runtime.`),
      el("p", "muted", `Début historique : ${workerTimeLabel(item.observed_started_at, item.observed_started_at_status)}. Une date connue indique une valeur interprétable, pas une chronologie validée.`),
      item.last_observation ? el("p", "mono", `Code de sortie exact : ${item.last_observation.exit_code ?? "Non fourni"}`) : null,
      data));
    document.body.append(dialog); this.#dialog = dialog; dialog.showModal();
    dialog.addEventListener("close", () => {
      if (this.#dialog === dialog) this.#dialog = null; dialog.remove();
      if (this.#closed || document.querySelector("dialog[open]")) return;
      const key = trigger?.dataset.focusKey;
      const replacement = key ? [...this.#page?.querySelectorAll<HTMLElement>("[data-focus-key]") ?? []].find((node) => node.dataset.focusKey === key) : null;
      (trigger?.isConnected ? trigger : replacement ?? this.#page?.querySelector<HTMLElement>("h1"))?.focus({ preventScroll: true });
    }, { once: true });
  }
  #brokerAuthorities(item: HumanBroker) {
    return el("ul", "human-broker-authorities", ...item.authorities.map((authority) => el("li", "",
      el("span", "mono", brokerAuthorityLabel(authority)),
      el("span", "muted", ` · ${authority.runtime_kind}`))));
  }
  #renderBrokers(state = this.#brokers, federation = false) {
    const table = el("table", "human-broker-table");
    const caption = el("caption", "", "Registre central du tenant · métadonnées enregistrées");
    const profiles = this.#api.readProfiles;
    const owned = state.value?.profile === "human_broker_registry_owned_v1" || !federation && profiles?.broker_registry === "human_broker_registry_owned_v1";
    const headers = ["Broker / incarnation", "État du bail", "Autorités déclarées", "Dernière réception observée", "Expiration", ...(owned ? ["Checkpoint déclaré"] : []), ...(federation ? ["Associations de la page workers"] : []), "Détails"].map((label) => {
      const cell = el("th", "", label); cell.scope = "col"; return cell;
    });
    table.append(caption, el("thead", "", el("tr", "", ...headers)), el("tbody", "", ...(state.value?.items ?? []).map((item, index) => {
      const heartbeat = item.last_heartbeat;
      const associated = federation ? this.#federation.value?.workers.items.filter((worker) =>
        worker.launch_broker.broker?.broker_id === item.broker_id || worker.observation_broker.broker?.broker_id === item.broker_id) ?? [] : [];
      return el("tr", "", el("td", "", el("p", "mono control-id", item.broker_id), el("p", "muted", "Incarnation courante"), el("p", "mono control-id", item.incarnation_id)),
        el("td", "", badge(brokerStateName[item.state], item.state === "current" ? "neutral" : "amber"),
          item.state === "stale" ? el("p", "muted", "Broker déconnecté possible · état runtime inconnu") : null,
          item.owned?.clock_uncertain && item.state !== "clock_uncertain" ? badge("Horloge du checkpoint incertaine", "amber") : null),
        el("td", "", this.#brokerAuthorities(item)),
        el("td", "", heartbeat ? el("div", "", el("p", "", formatBrokerDate(heartbeat.received_at, true)), el("p", "muted", "Heartbeat déclaré · réception observée"),
          heartbeat.incarnation_id !== item.incarnation_id ? badge("Heartbeat d’une ancienne incarnation", "amber") : null) : el("p", "muted", "Aucun heartbeat reçu dans le registre")),
        el("td", "", el("p", "muted", `Autorisation : ${formatBrokerDate(item.not_after, true)}`),
          heartbeat ? el("p", "muted", `Bail déclaré : ${formatBrokerDate(heartbeat.lease_expires_at, true)}`) : null),
        owned ? el("td", "", el("p", "", checkpointLabel(item)), item.owned?.checkpoint.status === "received" ?
          el("p", "muted", `Réception observée : ${formatBrokerDate(item.owned.checkpoint.received_at, true)} · couverture déclarée partielle`) :
          el("p", "muted", "L’activité reste inconnue")) : null,
        federation ? el("td", "", associated.length ? el("div", "human-federation-links", el("p", "muted", "Références historiques, sans preuve d’exécution par ce broker"),
          ...associated.map((worker) => action(`Ouvrir le worker ${worker.worker_id}`, `federation-broker-${item.broker_id}-worker-${worker.worker_id}`, () => this.#showWorker(worker)))) :
          el("p", "muted", "Aucune association dans la page workers · autres pages inconnues")) : null,
        el("td", "", action("Détails du broker", `broker-detail-${index}`, () => this.#showBroker(item))));
    })));
    const scroll = el("div", "human-broker-scroll", table); scroll.tabIndex = 0; scroll.setAttribute("role", "region"); scroll.setAttribute("aria-label", "Tableau des brokers enregistrés, défilement horizontal");
    return el("section", "panel human-broker-panel", el("div", "panel-heading", el("div", "", el("h2", "", "Registre des brokers"),
      el("p", "", "Source : registre central du tenant. Inventaire runtime inconnu ; aucun worker ni état de fonctionnement n’est déduit du bail."),
      el("p", "", "Lecture live paginée : les lignes peuvent changer entre deux pages. Le flux des ressources ne couvre pas ce registre."))),
      owned ? el("div", "info-strip", badge("Couverture partielle", "amber"), el("p", "", `Profil human_broker_registry_owned_v1 annoncé par le contrôle. Heartbeats et checkpoints ont des fraîcheurs indépendantes. ${profiles?.worker_registry === "unavailable" ? "Registre des workers indisponible. " : ""}La fédération est indisponible. L’activité et l’inventaire runtime restent inconnus dans ce registre.`),
        this.#brokerForbidden ? null : action("Relire mes accès", "human-owned-profile-access", () => this.#openView("contexts"))) : null,
      this.#status(state, formatBrokerDate), state.value ? el("p", "human-broker-evaluation muted", `Évaluation serveur : ${formatBrokerDate(state.value.evaluated_at, true)} · source tenant_broker_registry · cohérence live_keyset${owned ? " · count_scope page : aucun total du tenant" : ""}.`) : null,
      federation ? null : this.#brokerForbidden ? el("div", "control-error", el("p", "", state.error), action("Relire mes accès", "human-broker-access", () => this.#openView("contexts"))) :
        state.error ? this.#error(state.error, () => { void this.#loadBrokers(); }) : null,
      !state.value ? empty(state.loading ? "Chargement du registre…" : "Registre indisponible", this.#brokerForbidden ? "L’accès a été refusé. Les données, détails et positions de lecture ont été retirés." :
        "Une lecture réussie est nécessaire pour connaître les brokers enregistrés. L’inventaire runtime reste inconnu.") :
        !state.value.items.length ? empty(state.history.length ? "Fin de cette traversée live" : "Aucun broker enregistré à cette lecture", "Cette page décrit le registre à l’heure d’évaluation. L’inventaire runtime reste inconnu.") : scroll,
      !federation && this.#brokerForbidden ? null : this.#pagination(state, (cursor, history) => { if (federation) void this.#loadFederation({ leg: "brokers", cursor, history }); else void this.#loadBrokers(cursor, history); }, federation ? "human-federation-brokers" : "human", federation ? "Pages des brokers" : undefined),
      el("p", "human-broker-footnote muted", "Actualisation automatique au plus toutes les 30 secondes lorsque cette vue est visible. Ce rythme ne garantit aucun délai de réponse, notification ou retrait des droits."));
  }
  #showBroker(item: HumanBroker) {
    const federation = this.#mode === "federation";
    const page = federation ? this.#federation.value?.brokers : this.#brokers.value;
    if (federation ? !this.#canFederation() || this.#federationForbidden : this.#mode !== "brokers" || !this.#canBrokers() || this.#brokerForbidden) return;
    if (!page?.items.some((row) => row.broker_id === item.broker_id)) return;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.#dialog?.close(); this.#dialog?.remove();
    const heading = el("h2", "", "Détails du broker"); heading.id = "human-broker-detail-title";
    const dialog = el("dialog", "detail-dialog human-broker-dialog"); dialog.setAttribute("aria-labelledby", heading.id);
    dialog.dataset.brokerId = item.broker_id;
    dialog.dataset.brokerDisclosure = JSON.stringify([item.incarnation_id, item.grant_revision, item.authorities]);
    const close = button("", "icon-button", () => dialog.close(), "x"); close.setAttribute("aria-label", "Fermer les détails");
    const detail = (name: string, value: string | HTMLElement) => el("div", "", el("dt", "", name), el("dd", "", value));
    const h = item.last_heartbeat;
    dialog.append(el("div", "detail-header", heading, close), el("div", "detail-body",
      el("p", "muted", `Métadonnées de la page évaluée le ${formatBrokerDate(page.evaluated_at, true)}. Inventaire runtime inconnu.`),
      el("dl", "human-broker-details", detail("Broker", item.broker_id), detail("Incarnation courante", item.incarnation_id),
        detail("État du bail", brokerStateName[item.state]), detail("Révision de l’autorisation", item.grant_revision), detail("Révision du heartbeat", item.heartbeat_revision),
        detail("Autorisation accordée", formatBrokerDate(item.granted_at, true)), detail("Expiration de l’autorisation", formatBrokerDate(item.not_after, true)),
        detail("Révocation", item.revoked_at ? formatBrokerDate(item.revoked_at, true) : "Aucune révocation enregistrée"), detail("Autorités déclarées", this.#brokerAuthorities(item)),
        detail("Heartbeat déclaré", h ? `Séquence ${h.sequence} · incarnation ${h.incarnation_id}${h.incarnation_id !== item.incarnation_id ? " · ancienne incarnation" : ""}` : "Aucun heartbeat reçu"),
        detail("Réception observée", h ? formatBrokerDate(h.received_at, true) : "Inconnue"), detail("Expiration du bail déclaré", h ? formatBrokerDate(h.lease_expires_at, true) : "Inconnue"),
        ...(item.owned ? [detail("Profil", "human_broker_registry_owned_v1"), detail("Durée du bail autorisée", `${item.owned.lease_seconds} s`),
          detail("Horloge", item.owned.clock_uncertain ? "Incertaine pour le heartbeat ou le checkpoint" : "Aucune incertitude signalée par le contrôle"),
          ...checkpointFacts(item).map(([label, value]) => detail(label, label === "Réception du checkpoint" || label === "Expiration du checkpoint" ? formatBrokerDate(value, true) : value))] : []),
        detail("Demande de lecture", page.read_receipt.request_id), detail("Événement du reçu", page.read_receipt.audit_event_id),
        detail("Reçu durable du contrôle", formatBrokerDate(page.read_receipt.received_at, true))),
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
    if (!this.#canResources() || this.#mode !== "resources" || this.#closed || this.#resources.loading ||
      this.#resourceStreamAwaitingAccess() || this.#api.readBudgetStopped && !operatorRetry) return;
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
      this.#snapshotOnly() ? el("p", "muted", "Contenu et export indisponibles pour ce profil : seules les métadonnées sont disponibles. Les droits accordés à vos ressources restent distincts de cette limite.") : null,
      this.#status(state), state.error ? this.#error(state.error, () => { void this.#loadResources(); }) : "",
      this.#streamError ? this.#error(this.#streamError, () => this.#startStream()) : "",
      !state.value ? empty(state.loading ? "Chargement des ressources…" : "Ressources inconnues", "La lecture doit réussir avant de conclure sur ce contexte.") :
        !state.value.items.length ? empty("Aucune ressource sur cette page", "Aucune ressource explicitement autorisée dans cette sélection. Une ancienne trace seule n’accorde aucun accès.") :
          el("div", "human-resource-list", ...state.value.items.map((item, index) => el("article", "human-resource-row",
            el("div", "", el("h3", "", kindName[item.resource.kind]), el("p", "mono control-id", item.resource.resource_id),
              el("p", "muted", item.owner.kind === "human_local" ? `Propriétaire local : ${item.owner.account_id}` :
                `Propriétaire OIDC : ${item.owner.subject} · ${item.owner.issuer}`), el("p", "mono", `Révision ${item.revision}`)),
            el("div", "human-resource-actions",
              action("Métadonnées", `resource-metadata-${index}`, () => this.#select(item, "metadata"), !this.#canEvents() || !item.permissions.metadata),
              action("Contenu", `resource-content-${index}`, () => this.#select(item, "content"), this.#snapshotOnly() || !this.#canContent() || !item.permissions.metadata || !item.permissions.content),
              badge(item.permissions.export ? this.#snapshotOnly() ? "Droit d’export accordé · indisponible pour ce profil" : "Export autorisé selon la projection" : "Export non autorisé"),
              // Delegated sharing: offered in the administration context when the catalogue grants and serves grants_manage.
              this.#context?.mode === "administration" && delegationOffers(this.#context).grants && this.#delegatedChange.phase === null ?
                action("Gérer les partages", `resource-grants-${index}`, () => this.#editGrant(item.resource)) : null)))),
      this.#pagination(state, (next, previous) => { void this.#loadResources(next, previous); }));
  }
  #select(item: HumanResource, view: HumanView) {
    if (!this.#canResources() || !this.#canEvents() || this.#mode !== "resources" || !item.permissions.metadata ||
      view === "content" && (!this.#canContent() || !item.permissions.content || this.#snapshotOnly())) return;
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
    if (!selection || this.#mode !== "events" || !this.#canEvents() || this.#closed || this.#events.loading ||
      this.#resourceStreamAwaitingAccess() || this.#api.readBudgetStopped && !operatorRetry) return;
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
        this.#denySelection(error.message); this.#refreshActive(true); return;
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
      state.error && selection.view === "content" ? el("div", "", el("p", "muted", "Le contenu n’a pas été lu. Vous pouvez demander les métadonnées depuis Ressources."),
        action("Revenir aux ressources", "human-content-resources", () => this.#openView("resources"))) : null,
      this.#snapshotOnly() ? el("p", "muted", "Métadonnées uniquement pour ce profil. Le contenu et l’export sont indisponibles.") : null,
      state.value?.archived_through_seq ? el("p", "notice notice-info retention-notice",
        `Historique archivé hors ligne jusqu’à l’événement n° ${state.value.archived_through_seq} : les événements retirés de la base vivante n’apparaissent ni dans ce journal ni dans ses exports. Leur vérification exige les archives de l’opérateur.`) : null,
      !state.value ? empty(state.loading ? "Chargement des événements…" : "Événements inconnus", "Une lecture absente ou refusée ne constitue pas une absence d’activité.") :
        !state.value.items.length ? empty("Aucun événement sur cette page", "Aucune trace reçue pour cette sélection autorisée, à cette lecture.") :
          el("div", "human-event-list", ...state.value.items.map((item, index) => el("article", "human-event-row",
            el("div", "", el("h3", "", item.metadata.kind), el("p", "muted", formatDate(item.metadata.timestamp, true)),
              el("p", "muted", `Source : ${item.metadata.source} · Reçu le ${formatDate(item.metadata.received_at, true)}`),
              el("div", "detail-badges", badge(`Provenance : ${item.metadata.provenance}`), badge(`Couverture : ${item.metadata.coverage}`),
                item.metadata.decision ? badge(`Décision : ${item.metadata.decision}`) : badge("Décision non fournie"))),
            action("Détails", `human-event-${index}`, () => { void this.#showEvent(item); }, !this.#canShowEvent(selection, item))))),
      this.#pagination(state, (next, previous) => { void this.#loadEvents(next, previous); }), this.#renderExport());
  }
  #usesResourceStream() {
    return can(this.#context, "events_stream") && !this.#snapshotOnly() && (this.#mode === "resources" || this.#mode === "events");
  }
  #resourceStreamAwaitingAccess() { return this.#resourceStreamNeedsAccess && this.#usesResourceStream(); }
  #startStream() {
    this.#streamAbort.abort(); this.#streamError = "";
    if (this.#snapshotOnly()) { this.#streamStatus = "Direct indisponible pour ce profil"; return; }
    if (!this.#usesResourceStream() || this.#closed || this.#api.readBudgetStopped) return;
    const hidden = document.visibilityState !== "visible";
    if (hidden || this.#resourceStreamAwaitingAccess()) {
      this.#resourceStreamNeedsAccess = true;
      const status = hidden ? RESOURCE_STREAM_HIDDEN : "Flux suspendu · accès à relire";
      if (this.#streamStatus !== status) { this.#streamStatus = status; this.#render(); }
      return;
    }
    this.#streamAbort = new AbortController();
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
        this.#denySelection(error.message); this.#refreshActive(true); return;
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
  #canShowEvent(selection: { item: HumanResource; view: HumanView }, item: HumanEvent) {
    const snapshot = this.#snapshotOnly();
    const requiresCanonicalId = snapshot || this.#identity?.read_profiles?.broker_registry === "human_broker_registry_owned_v1";
    return this.#mode === "events" && this.#canEvents() && selection.item.permissions.metadata &&
      !this.#api.readBudgetStopped && !this.#resourceStreamAwaitingAccess() &&
      (!snapshot || selection.view === "metadata") && (!requiresCanonicalId || hasCanonicalEventId(item));
  }
  async #showEvent(item: HumanEvent) {
    const selection = this.#selection;
    if (!selection || !this.#canShowEvent(selection, item)) return;
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
      if (error instanceof HumanApiError && ["permission_denied", "not_found"].includes(error.code)) { this.#denySelection(error.message); this.#refreshActive(true); return; }
      body.replaceChildren(this.#error(failure(error), () => { void this.#showEvent(item); }));
    }
  }
  #renderExport() {
    // Two server facts: the context offers the export operation, and this resource grants its own export.
    const offered = this.#canExport();
    const permitted = !!this.#selection?.item.permissions.export;
    const covered = !this.#snapshotOnly();
    const status = el("p", "human-export-status", this.#exporting ? `${this.#exportPages} page(s) reçue(s), ${this.#exportBytes} octets · export encore partiel, aucun téléchargement.` : this.#exportMessage);
    status.setAttribute("role", "status");
    return el("section", "human-export", el("h3", "", "Exporter cette sélection"),
      el("p", "muted", "JSON · ressource et projection affichées ci-dessus. Limites de cet onglet : 25 pages, 4 Mio, 60 secondes. Le fichier est proposé seulement à la fin de la sélection autorisée, avec ses reçus de lecture."),
      action(`Exporter ${this.#selection?.view === "content" ? "le contenu" : "les métadonnées"}`, "human-export", () => { void this.#export(); },
        !covered || !offered || !permitted || this.#exporting || this.#resourceStreamAwaitingAccess()),
      this.#exporting ? action("Annuler l’export", "human-export-cancel", () => this.#exportAbort.abort()) : null,
      !covered ? el("p", "muted", "Export indisponible pour ce profil, même lorsque le droit d’export est accordé.") : null,
      !offered ? el("p", "muted", "Ce contexte n’offre pas l’opération events_export : aucun export n’est émis.") : null,
      !permitted ? el("p", "muted", "Le contrôle n’accorde pas le droit d’export pour cette ressource.") : null, status);
  }
  async #export() {
    if (this.#snapshotOnly() || this.#resourceStreamAwaitingAccess()) return;
    const selection = this.#selection; if (!selection || this.#mode !== "events" || !this.#canEvents() || !this.#canExport() || !selection.item.permissions.export || this.#exporting || this.#api.readBudgetStopped) return;
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
      if (error instanceof HumanApiError && ["permission_denied", "not_found"].includes(error.code)) { this.#denySelection(error.message); this.#refreshActive(true); return; }
      this.#exportMessage = signal.aborted ? "Export annulé. Aucun fichier partiel n’a été téléchargé." :
        error instanceof DOMException && error.name === "TimeoutError" ? "Export arrêté après 60 secondes. Aucun fichier partiel n’a été téléchargé." : failure(error);
    } finally { if (!this.#closed && generation === this.#generation && selected === this.#selectionGeneration) { this.#exporting = false; this.#render(); } }
  }
}
