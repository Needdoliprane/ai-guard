import "./style.css";
import { ApiClient, ApiError, IdentityError, eventUrl } from "./api.ts";
import type { StreamStatus } from "./api.ts";
import { el, button, icon } from "./dom.ts";
import { ControlDashboard } from "./control-view.ts";
import { HumanDashboard } from "./human-view.ts";
import { HumanApiError } from "./human-api.ts";
import { HumanContractError, uuid } from "./human-model.ts";
import { OidcSession, OidcSessionError } from "./oidc-session.ts";
import { FRONTEND_IDENTITY } from "./frontend-version.ts";
import { buildIdentityFooter } from "./build-identity-view.ts";
import { relatedAgents, relatedSandboxes, workerRelation } from "./control.ts";
import type { WorkerRecord } from "./control.ts";
import type { ActivityChannelCounts, BrokerCapture } from "./model.ts";
import {
  activityChannel,
  activityDetails,
  asText,
  brokerBatchResultDetails,
  brokerFailureDetails,
  brokerWorkspaceDetails,
  buildAgentForest,
  decisionEvaluationDetails,
  eventSummary,
  formatBytes,
  formatDate,
  formatMetric,
  isAuditEvent,
  matchesFilters,
  mergeEventPage,
  periodFilters,
  parseEntityCollection,
  projectEvents,
} from "./model.ts";
import type {
  Agent,
  AgentNode,
  AuditEvent,
  EventFilters,
  EventPage,
  EventPeriod,
  Integrity,
  Overview,
  Sandbox,
  View,
  ViewerIdentity,
} from "./types.ts";

const root = document.querySelector<HTMLDivElement>("#app")!;
const navItems: {
  id: View;
  label: string;
  icon: string;
  description: string;
}[] = [
  {
    id: "overview",
    label: "Vue d’ensemble",
    icon: "grid",
    description: "État des services, identités reçues et journal du client authentifié.",
  },
  {
    id: "agents",
    label: "Agents",
    icon: "agents",
    description: "Identités, tâches et liens explicitement reçus.",
  },
  {
    id: "actions",
    label: "Actions",
    icon: "terminal",
    description: "Commandes, décisions et résultats consignés.",
  },
  {
    id: "security",
    label: "Sécurité",
    icon: "shield",
    description: "Contrôles de politique et intégrité du journal.",
  },
  {
    id: "files",
    label: "Fichiers",
    icon: "folder",
    description: "Événements de fichiers et d’export reçus par le collecteur.",
  },
  {
    id: "sandboxes",
    label: "Sandboxes",
    icon: "box",
    description: "Conteneurs et paramètres d’isolation connus.",
  },
  {
    id: "workers",
    label: "Workers",
    icon: "box",
    description: "Registre durable et dernières observations runtime de votre client.",
  },
  {
    id: "quota",
    label: "Quotas",
    icon: "cpu",
    description: "Ressources réservées et limites appliquées à votre client.",
  },
  {
    id: "usage",
    label: "Allocations",
    icon: "clock",
    description: "Temps d’allocation observé, avec unités exactes et sans facturation.",
  },
];
let client: ApiClient | null = null;
let identity: ViewerIdentity | null = null;
let control: ControlDashboard | null = null;
let humanSession: HumanDashboard | null = null;
let oidcSession: OidcSession | null = null;
let authenticationView = 0;
let overview: Overview | null = null;
let agents: Agent[] = [];
let sandboxes: Sandbox[] = [];
let events: AuditEvent[] = [];
let integrity: Integrity | null = null;
let nextCursor: number | null = null;
let view: View = "overview";
let filters: EventFilters = {};
let selectedPeriod: EventPeriod = "all";
let treeMode = false;
let loadingEvents = false;
let eventsError = "";
let agentsAvailable: boolean | null = null;
let sandboxesAvailable: boolean | null = null;
let agentsLoadedAt: string | null = null;
let sandboxesLoadedAt: string | null = null;
let agentsError = "";
let sandboxesError = "";
let summaryLoading = false;
let summaryRequest = 0;
let eventRequest = 0;
let refreshedAt: string | null = null;
let streamStatus: StreamStatus = "closed";
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
let healthTimer: ReturnType<typeof setInterval> | undefined;
let health: "ok" | "unknown" = "unknown";
let page: HTMLElement;
let navigation: HTMLElement;
let viewPosition: HTMLElement;
let currentViewLabel: HTMLElement;
let previousView: HTMLButtonElement;
let nextView: HTMLButtonElement;
let journalExport: HTMLButtonElement;
let streamPill: HTMLElement;
let notice: HTMLElement;
type NoticeOwner = { client: ApiClient; source: "summary" | "journal" };
let noticeOwner: NoticeOwner | null = null;
let eventSlot: HTMLElement | null = null;
let statSlot: HTMLElement | null = null;
let sideSlot: HTMLElement | null = null;
let collectionSlot: HTMLElement | null = null;
let dialog: HTMLDialogElement | null = null;
let detailEntityLinks: { slot: HTMLElement; render: () => HTMLElement }[] = [];
let dark = matchMedia("(prefers-color-scheme: dark)").matches;
document.documentElement.dataset.theme = dark ? "dark" : "light";

function badge(label: string, tone = "neutral"): HTMLElement {
  return el("span", `badge badge-${tone}`, label);
}
function provenance(value: unknown): HTMLElement {
  const values: Record<string, [string, string]> = {
    observed: ["Observé", "teal"],
    declared: ["Déclaré", "blue"],
    inferred: ["Inféré", "amber"],
  };
  const mapped = values[asText(value, "")];
  const node = badge(mapped?.[0] ?? "Inconnu", mapped?.[1]);
  node.title = `Provenance : ${asText(value)}`;
  return node;
}
function coverage(value: unknown): HTMLElement {
  const labels: Record<string, string> = {
    native: "Native",
    observed: "Observée",
    generic: "Générique",
  };
  const node = badge(labels[asText(value, "")] ?? "Inconnue");
  node.title = `Couverture : ${asText(value)}`;
  return node;
}
function decision(value: unknown): HTMLElement {
  const map: Record<string, [string, string]> = {
    allow: ["Autorisé", "teal"],
    deny: ["Refusé", "red"],
    review: ["À confirmer", "amber"],
  };
  const item = map[asText(value, "")];
  return badge(item?.[0] ?? "Non renseignée", item?.[1]);
}
function status(value: unknown): HTMLElement {
  const map: Record<string, [string, string]> = {
    running: ["En cours", "teal"],
    active: ["Actif", "teal"],
    started: ["Démarré", "teal"],
    ready: ["Prêt", "teal"],
    created: ["Créé", "blue"],
    completed: ["Terminé", "neutral"],
    stopped: ["Arrêté", "neutral"],
    failed: ["En échec", "red"],
    error: ["Erreur", "red"],
    pending: ["En attente", "amber"],
    idle: ["Inactif", "neutral"],
  };
  const item = map[asText(value, "")];
  return badge(item?.[0] ?? asText(value), item?.[1]);
}
function themeButton(): HTMLButtonElement {
  const node = button(
    "",
    "icon-button",
    () => {
      dark = !dark;
      document.documentElement.dataset.theme = dark ? "dark" : "light";
      node.replaceChildren(icon(dark ? "sun" : "moon"));
      node.setAttribute(
        "aria-label",
        dark ? "Activer le thème clair" : "Activer le thème sombre",
      );
    },
    dark ? "sun" : "moon",
  );
  node.setAttribute(
    "aria-label",
    dark ? "Activer le thème clair" : "Activer le thème sombre",
  );
  return node;
}
function brand(): HTMLElement {
  const mark = el("div", "brand",
    el("strong", "brand-signature", "OVPT"),
    el("span", "brand-product", el("strong", "brand-wordmark", "ai-guard"),
      el("span", "brand-caption", "LECTURE SEULE")));
  mark.setAttribute("role", "img");
  mark.setAttribute("aria-label", "OVPT · ai-guard · Lecture seule");
  return el("div", "brand-identity", mark, el("span", "brand-version", `Version ${FRONTEND_IDENTITY.version}`));
}
function footerBrand(): HTMLElement {
  const mark = el("span", "footer-brand", el("strong", "footer-signature", "OVPT"),
    el("span", "footer-product", "ai-guard"));
  mark.setAttribute("role", "img");
  mark.setAttribute("aria-label", "OVPT · ai-guard");
  return mark;
}

const controlDeadlineLabels = new Map<unknown, string>([
  ["original_execution_deadline", "Durée d’exécution autorisée atteinte"],
  ["observation_window", "Fenêtre d’observation écoulée"],
]);
const controlStopLabels = new Map<unknown, string>([
  ["no_owned_worker", "Aucun worker sous ce contrôle"],
  ["already_stopped", "Worker déjà hors exécution"],
  ["engine_acknowledged", "Demande acceptée par le moteur"],
  ["unconfirmed", "Résultat de la demande inconnu"],
]);

function labelValue(
  label: string,
  value: unknown,
  fieldSource?: unknown,
): HTMLElement {
  return el(
    "div",
    "key-value",
    el("dt", "", label, fieldSource ? provenance(fieldSource) : null),
    el("dd", "", asText(value)),
  );
}
function empty(
  title: string,
  description: string,
  glyph = "pulse",
): HTMLElement {
  return el(
    "div",
    "empty-state",
    el("span", "empty-icon", icon(glyph)),
    el("h3", "", title),
    el("p", "", description),
  );
}
function setNotice(message = "", kind: "error" | "info" = "error", owner: NoticeOwner | null = null) {
  if (!notice) return;
  noticeOwner = message ? owner : null;
  notice.hidden = !message;
  notice.className = `notice notice-${kind}`;
  notice.replaceChildren(
    icon(kind === "error" ? "alert" : "info"),
    el("span", "", message),
  );
}
function errorMessage(error: unknown): string {
  return error instanceof ApiError || error instanceof IdentityError || error instanceof HumanApiError || error instanceof HumanContractError || error instanceof OidcSessionError ||
    error instanceof Error && error.message === "Contrat de liste d’identités incomplet ou incompatible."
    ? error.message
    : "Connexion au collecteur impossible. Vérifiez le service local puis réessayez.";
}
function onError(error: unknown, owner: NoticeOwner | null = null) {
  if (
    error instanceof ApiError &&
    (error.status === 401 || error.status === 403)
  ) {
    disconnect("Votre session a expiré ou le jeton a été refusé.");
    return;
  }
  setNotice(errorMessage(error), "error", owner);
}
function login(message = "", preferredAccess: "viewer" | "human" = "viewer") {
  const currentAuthenticationView = ++authenticationView;
  document.title = "ai-guard · Supervision locale";
  const access = el("select", "text-input login-access"); access.id = "access-mode";
  const technical = el("option", "", "Lecture technique (viewer)"); technical.value = "viewer";
  const personal = el("option", "", "Compte personnel local"); personal.value = "human";
  access.append(technical, personal); access.value = preferredAccess;
  const token = el("input", "text-input");
  token.id = "viewer-token";
  token.type = "password";
  token.required = true;
  token.autocomplete = "off";
  token.spellcheck = false;
  token.setAttribute("aria-describedby", "token-help");
  const error = el("div", "login-error", message);
  error.setAttribute("role", "alert");
  error.hidden = !message;
  const submit = el(
    "button",
    "button button-primary login-submit",
    "Ouvrir la supervision",
    icon("arrow"),
  );
  submit.type = "submit";
  const form = el(
    "form",
    "login-form",
    el("label", "field-label", "Jeton de lecture"),
    token,
    el(
      "p",
      "field-help",
      "Utilisez le jeton viewer configuré pour votre collecteur.",
    ),
    error,
    submit,
  );
  form.querySelector("label")!.htmlFor = token.id;
  form.querySelector("p")!.id = "token-help";
  const describeAccess = () => {
    const personal = access.value === "human";
    token.value = ""; token.id = personal ? "human-token" : "viewer-token";
    const label = form.querySelector("label")!;
    label.htmlFor = token.id; label.textContent = personal ? "Credential personnel local" : "Jeton de lecture";
    form.querySelector("p")!.textContent = personal ?
      "Utilisez le credential nominatif préparé par l’opérateur. Le contrôle établit votre identité et vos accès." :
      "Utilisez le jeton viewer configuré pour votre collecteur.";
    submit.replaceChildren(personal ? "Ouvrir mon espace" : "Ouvrir la supervision", icon("arrow"));
  };
  access.addEventListener("change", () => { describeAccess(); error.hidden = true; });
  describeAccess();
  const sso = el("div", "login-note"); sso.setAttribute("aria-live", "polite");
  const inspectSso = async () => {
    sso.replaceChildren(el("p", "muted", "Vérification de la connexion SSO…"));
    const candidate = new OidcSession();
    try {
      const status = await candidate.status();
      if (currentAuthenticationView !== authenticationView) { candidate.close(); return; }
      if (!status.enabled) { sso.replaceChildren(el("p", "muted", "SSO non activé sur ce contrôle.")); candidate.close(); return; }
      const connect = button(`Se connecter avec ${status.provider_label}`, "button button-secondary", async () => {
        connect.disabled = true; connect.textContent = "Connexion au fournisseur…";
        try { window.location.assign(await candidate.start()); }
        catch (cause) { if (currentAuthenticationView === authenticationView) { error.textContent = errorMessage(cause); error.hidden = false; connect.disabled = false; connect.textContent = "Réessayer la connexion SSO"; } }
      });
      sso.replaceChildren(connect, el("p", "field-help", `Source : ${status.issuer}. Les permissions restent établies par le contrôle.`));
    } catch {
      if (currentAuthenticationView !== authenticationView) return;
      candidate.close(); sso.replaceChildren(el("p", "muted", "État SSO indisponible."), button("Réessayer", "button button-secondary", () => { void inspectSso(); }));
    }
  };
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const value = token.value.trim();
    if (!value) return;
    submit.disabled = true;
    access.disabled = true;
    submit.textContent = "Connexion…";
    error.hidden = true;
    if (access.value === "human") {
      token.value = "";
      const candidate = new HumanDashboard(root, value, { brand, themeButton }, (message) => {
        if (humanSession !== candidate) return;
        humanSession = null; login(message, "human");
      });
      humanSession = candidate;
      try { await candidate.start(); }
      catch (cause) {
        if (humanSession !== candidate) return;
        candidate.close(); humanSession = null; login(errorMessage(cause), "human");
      }
      return;
    }
    const candidate = new ApiClient(value, fetch, () => {
      if (client === candidate)
        disconnect("Votre session a expiré ou le jeton a été refusé.");
    });
    token.value = "";
    try {
      const authenticated = await candidate.identity();
      client = candidate;
      identity = authenticated;
      control = new ControlDashboard(candidate, () =>
        disconnect("Votre session a expiré ou le jeton a été refusé."), openDialog, workerEntities);
      view = "overview";
      shell();
      await loadInitial();
    } catch (cause) {
      candidate.close();
      error.textContent = errorMessage(cause);
      error.hidden = false;
      submit.disabled = false;
      access.disabled = false;
      submit.replaceChildren("Ouvrir la supervision", icon("arrow"));
      token.focus();
    }
  });
  root.replaceChildren(
    el(
      "div",
      "login-page",
      el("header", "login-header", brand(), themeButton()),
      el(
        "main",
        "login-content",
        el(
          "section",
          "login-story",
          el("p", "eyebrow", "01 / ACCÈS AU CONTRÔLE"),
          el("h1", "", "Comprendre chaque action.", el("span", "title-secondary", " Garder la trace.")),
        ),
        el(
          "section",
          "login-card",
          el("p", "eyebrow", "SESSION DE CONSULTATION"),
          el("h2", "sr-only", "Accéder à la supervision"),
          el("label", "sr-only", "Type d’accès"),
          access,
          form,
          sso,
          el(
            "p",
            "muted",
            "Authentifiez-vous auprès du collecteur pour consulter les données de votre client.",
          ),
          el(
            "div",
            "login-note",
            icon("shield"),
            el(
              "p",
              "",
              "Le jeton reste en mémoire le temps de cette session. Une actualisation de la page vous déconnecte.",
            ),
          ),
        ),
        el("p", "login-lead",
          "Consultez les agents, les décisions et les exécutions de votre client, à partir des données reçues par le collecteur."),
        el("dl", "login-facts",
          labelValue("Identité", "Le jeton authentifié détermine votre client."),
          labelValue("Périmètre", "Lecture seule · Les opérations se pilotent depuis le CLI de confiance."),
          labelValue("Preuves", "Les données observées, déclarées et inconnues restent distinctes.")),
      ),
      el(
        "footer",
        "login-footer",
        footerBrand(),
        el("span", "footer-motto", "Omnia Vincit Progressus Technologicus"),
        el("span", "", "Source : collecteur · Accès authentifié"),
      ),
    ),
  );
  root.querySelector<HTMLLabelElement>('label.sr-only')!.htmlFor = access.id;
  void inspectSso();
}

function shell() {
  const links = navItems.map((item, index) => {
    const link = el("a", "nav-link",
      el("span", "nav-number", String(index + 1).padStart(2, "0")),
      el("span", "nav-label", item.label));
    link.href = `#${item.id}`;
    link.dataset.view = item.id;
    link.setAttribute("aria-label", item.label);
    link.title = `${String(index + 1).padStart(2, "0")} · ${item.label}`;
    link.addEventListener("click", (event) => {
      event.preventDefault();
      navigate(item.id);
    });
    return link;
  });
  streamPill = el("span", "stream-pill");
  streamPill.setAttribute("role", "status");
  streamPill.setAttribute("aria-live", "polite");
  const logout = button("", "button button-secondary header-logout", () => disconnect(), "logout");
  logout.append(el("span", "", "Se déconnecter"));
  logout.setAttribute("aria-label", "Se déconnecter");
  const header = el("header", "topbar", el("div", "header-inner",
    brand(),
    el("div", "header-scope",
      el("strong", "authenticated-scope", `Client : ${identity?.tenant_id ?? "Inconnu"}`),
      streamPill),
    el("div", "topbar-actions", themeButton(), logout)));
  viewPosition = el("span", "view-position");
  currentViewLabel = el("strong", "current-view-label");
  previousView = button("", "view-step view-previous", () => stepView(-1));
  nextView = button("", "view-step view-next", () => stepView(1));
  const refresh = button("Actualiser", "button dock-refresh", () => {
    if (view === "workers" || view === "quota" || view === "usage") void control?.refresh();
    else { void refreshSummary(); void loadEvents(); }
  }, "refresh");
  journalExport = button("Exporter JSONL", "button dock-export", () => { void exportJournal(journalExport); }, "down");
  navigation = el("nav", "control-dock", el("div", "dock-inner",
    el("div", "dock-top",
      el("div", "dock-current", viewPosition, currentViewLabel),
      el("div", "view-stepper", previousView, nextView),
      el("div", "dock-actions", refresh, el("div", "export-place", journalExport))),
    el("div", "nav-list", ...links)));
  navigation.id = "navigation";
  navigation.setAttribute("aria-label", "Vues de supervision");
  notice = el("div", "notice");
  notice.setAttribute("role", "alert");
  notice.hidden = true;
  page = el("main", "page");
  page.id = "main-content";
  const skip = el("a", "skip-link", "Aller au contenu");
  skip.href = "#main-content";
  root.replaceChildren(skip, el("div", "app-shell", header, navigation,
    el("div", "main-shell", notice, page)));
  updateStream("connecting");
  renderView();
}
function stepView(direction: number) {
  const index = navItems.findIndex((item) => item.id === view);
  const next = navItems[index + direction];
  if (next) navigate(next.id);
}
function updateNavigation() {
  const index = navItems.findIndex((item) => item.id === view);
  viewPosition.textContent = `Vue ${String(index + 1).padStart(2, "0")} / 09`;
  currentViewLabel.textContent = navItems[index]!.label;
  const update = (node: HTMLButtonElement, offset: number, label: string, arrow: string) => {
    const item = navItems[index + offset];
    node.disabled = !item;
    node.setAttribute("aria-label", item ? `${label} : ${item.label}` : `${label} : aucune vue`);
    node.replaceChildren(el("span", "step-label", el("span", "step-direction", label), el("span", "step-arrow", arrow)),
      el("span", "step-destination", item?.label ?? (offset < 0 ? "Début des vues" : "Fin des vues")));
  };
  update(previousView, -1, "Précédent", "←");
  update(nextView, 1, "Suivant", "→");
  journalExport.hidden = view === "workers" || view === "quota" || view === "usage";
}
function navigate(next: View) {
  view = next;
  history.replaceState(null, "", `#${view}`);
  renderView();
  window.scrollTo({ top: 0, behavior: "instant" });
  page.querySelector("h1")?.focus({ preventScroll: true });
}

function disconnect(message = "") {
  control?.close();
  control = null;
  client?.close();
  client = null;
  identity = null;
  clearTimeout(refreshTimer);
  refreshTimer = undefined;
  clearInterval(healthTimer);
  eventRequest++;
  overview = null;
  refreshedAt = null;
  agents = [];
  sandboxes = [];
  events = [];
  integrity = null;
  filters = {};
  selectedPeriod = "all";
  nextCursor = null;
  health = "unknown";
  loadingEvents = false;
  eventsError = "";
  agentsAvailable = null;
  sandboxesAvailable = null;
  agentsLoadedAt = null;
  sandboxesLoadedAt = null;
  agentsError = "";
  sandboxesError = "";
  summaryLoading = false;
  summaryRequest++;
  noticeOwner = null;
  dialog?.close();
  dialog?.remove();
  dialog = null;
  history.replaceState(null, "", location.pathname);
  login(message);
}
function updateStream(state: StreamStatus) {
  streamStatus = state;
  control?.stream(state === "live");
  updateDetailEntityLinks();
  if (!streamPill) return;
  const labels = {
    connecting: "Connexion au flux",
    live: "Flux connecté",
    retrying: "Reconnexion…",
    closed: "Flux arrêté",
  };
  streamPill.dataset.stream = state;
  streamPill.replaceChildren(el("span", "status-dot"), labels[state]);
}
async function readHealth() {
  try {
    const response = await fetch("/healthz", {
      credentials: "omit",
      cache: "no-store",
      redirect: "error",
      signal: AbortSignal.timeout(5000),
    });
    health = response.ok ? "ok" : "unknown";
  } catch {
    health = "unknown";
  }
  renderSupport();
}
async function loadInitial() {
  const current = client;
  if (!current) return;
  await Promise.all([loadEvents(), refreshSummary()]);
  if (current !== client) return;
  void readHealth();
  healthTimer = setInterval(() => {
    void readHealth();
    void refreshSummary();
    void control?.refresh();
  }, 30000);
  const cursor = Math.max(0, ...events.map((event) => event.seq));
  void current.listen(
    cursor,
    (event) => {
      if (client !== current) return;
      if (/^(?:control\.)?(worker|quota|allocation)\./.test(event.kind)) control?.invalidate();
      if (
        matchesFilters(event, filters) &&
        !events.some((existing) => existing.seq === event.seq)
      ) {
        events = [event, ...events].sort((a, b) => b.seq - a.seq);
        if (events.length > 1000) {
          events = events.slice(0, 1000);
          nextCursor = events.at(-1)?.seq ?? null;
        }
        renderEvents();
      }
      if (refreshTimer === undefined)
        refreshTimer = setTimeout(() => {
          refreshTimer = undefined;
          void refreshSummary();
        }, 1000);
    },
    (state) => {
      if (client === current) updateStream(state);
    },
    () => {
      if (client === current)
        disconnect("Votre session a expiré ou le jeton a été refusé.");
    },
  );
}
async function refreshSummary() {
  const current = client;
  if (!current) return;
  const request = ++summaryRequest;
  summaryLoading = true;
  control?.relationshipsChanged();
  updateDetailEntityLinks();
  const result = await Promise.allSettled([
    current.get<Overview>("/api/overview"),
    current.get<unknown>("/api/agents").then(parseEntityCollection<Agent>),
    current.get<unknown>("/api/sandboxes").then(parseEntityCollection<Sandbox>),
    current.get<Integrity>("/api/integrity"),
  ]);
  if (current !== client || request !== summaryRequest) return;
  summaryLoading = false;
  const [o, a, s, i] = result;
  if (o.status === "fulfilled") {
    overview = o.value;
    refreshedAt = new Date().toISOString();
  } else {
    overview = null;
    refreshedAt = null;
  }
  agentsAvailable = a.status === "fulfilled";
  sandboxesAvailable = s.status === "fulfilled";
  agentsError = a.status === "rejected" ? errorMessage(a.reason) : "";
  sandboxesError = s.status === "rejected" ? errorMessage(s.reason) : "";
  if (a.status === "fulfilled") {
    agents = a.value;
    agentsLoadedAt = new Date().toISOString();
  }
  if (s.status === "fulfilled") {
    sandboxes = s.value;
    sandboxesLoadedAt = new Date().toISOString();
  }
  integrity = i.status === "fulfilled" ? i.value : null;
  const failed = result.find((entry) => entry.status === "rejected");
  if (failed?.status === "rejected") onError(failed.reason, { client: current, source: "summary" });
  else if (noticeOwner?.client === current && noticeOwner.source === "summary") setNotice();
  if (current !== client) return;
  control?.relationshipsChanged();
  updateDetailEntityLinks();
  renderStats();
  renderSupport();
  updateFilterChoices();
  if (view === "agents") renderAgents();
  if (view === "sandboxes") renderSandboxes();
}
async function loadEvents(append = false) {
  const current = client;
  if (!current) return;
  const request = ++eventRequest;
  const startedSeq = Math.max(0, ...events.map((event) => event.seq));
  loadingEvents = true;
  eventsError = "";
  renderEvents();
  try {
    const result = await current.get<EventPage>(
      eventUrl(filters, append ? nextCursor : null),
    );
    if (current !== client || request !== eventRequest) return;
    const received = Array.isArray(result.items)
      ? result.items.filter(isAuditEvent)
      : [];
    events = mergeEventPage(
      events.filter((event) => matchesFilters(event, filters)),
      received,
      append ? undefined : startedSeq,
    );
    nextCursor =
      typeof result.next_cursor === "number" ? result.next_cursor : null;
    if (noticeOwner?.client === current && noticeOwner.source === "journal") setNotice();
  } catch (error) {
    if (current === client && request === eventRequest) {
      eventsError = errorMessage(error);
      onError(error, { client: current, source: "journal" });
    }
  } finally {
    if (current === client && request === eventRequest) {
      loadingEvents = false;
      renderEvents();
    }
  }
}

function renderView() {
  control?.unmount();
  const item = navItems.find((entry) => entry.id === view)!;
  document.title = `${item.label} · ai-guard`;
  updateNavigation();
  for (const link of document.querySelectorAll<HTMLAnchorElement>(
    ".nav-link",
  )) {
    if (link.dataset.view === view) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  const title = el("h1", "", item.label);
  title.tabIndex = -1;
  page.replaceChildren(
    el(
      "div",
      "page-heading",
      el(
        "div",
        "",
        el("div", "eyebrow", `SUPERVISION / ${String(navItems.findIndex((entry) => entry.id === view) + 1).padStart(2, "0")}`),
        title,
        el("p", "page-description", item.description),
      ),

    ),
  );
  statSlot = null;
  sideSlot = null;
  eventSlot = null;
  collectionSlot = null;
  if (view === "overview" || view === "security") {
    statSlot = el("section", "stats-section");
    statSlot.setAttribute("aria-label", "Indicateurs reçus");
    page.append(statSlot);
    renderStats();
  }
  if (view === "workers" || view === "quota" || view === "usage") {
    const slot = el("section", "control-view");
    slot.setAttribute("aria-label", item.label);
    page.append(slot);
    control?.mount(slot, view);
  } else if (view === "agents") {
    const tableToggle = button(
      "Tableau",
      `segment${!treeMode ? " selected" : ""}`,
      () => {
        treeMode = false;
        renderView();
      },
      "grid",
    );
    tableToggle.setAttribute("aria-pressed", String(!treeMode));
    const treeToggle = button(
      "Hiérarchie",
      `segment${treeMode ? " selected" : ""}`,
      () => {
        treeMode = true;
        renderView();
      },
      "branch",
    );
    treeToggle.setAttribute("aria-pressed", String(treeMode));
    page.append(
      el(
        "div",
        "section-heading",
        el(
          "div",
          "",
          el("h2", "", "Agents enregistrés"),
          el(
            "p",
            "muted",
            "Un lien parent apparaît uniquement si son identifiant est fourni.",
          ),
        ),
        el("div", "segmented", tableToggle, treeToggle),
      ),
    );
    collectionSlot = el("section", "collection");
    page.append(collectionSlot);
    renderAgents();
  } else if (view === "sandboxes") {
    page.append(
      el(
        "div",
        "info-strip",
        icon("info"),
        el(
          "span",
          "",
          "Les paramètres affichés proviennent du journal. La présence d’un conteneur n’est pas une preuve de sa disponibilité actuelle.",
        ),
      ),
    );
    collectionSlot = el("section", "sandbox-grid");
    page.append(collectionSlot);
    renderSandboxes();
  } else {
    if (view === "files")
      page.append(
        el(
          "div",
          "info-strip",
          icon("folder"),
          el(
            "span",
            "",
            "Cette vue expose les événements de fichiers reçus. Les chemins et les différences restent inconnus tant qu’aucun agent ou runner ne les fournit.",
          ),
        ),
      );
    const panel = el("section", "panel event-panel");
    const heading =
      view === "overview"
        ? "Journal d’activité"
        : view === "actions"
          ? "Historique des actions"
          : view === "security"
            ? "Événements de sécurité"
            : "Activité des fichiers";
    panel.append(
      el(
        "div",
        "panel-heading",
        el(
          "div",
          "",
          el("h2", "", heading),
          el(
            "p",
            "muted",
            "Événements reçus par le collecteur, du plus récent au plus ancien.",
          ),
        ),
        el("span", "panel-symbol", icon(item.icon)),
      ),
      filterForm(),
    );
    eventSlot = el("div", "event-slot");
    panel.append(eventSlot);
    if (view === "overview") {
      sideSlot = el("aside", "support-column");
      page.append(el("div", "overview-grid", panel, sideSlot));
    } else {
      page.append(panel);
      if (view === "security") {
        sideSlot = el("div", "security-support");
        page.append(sideSlot);
      }
    }
    renderEvents();
    renderSupport();
  }
  page.append(
    el(
      "footer",
      "page-footer",
      el("div", "footer-identity", footerBrand(), el("span", "footer-access", icon("lock"), "Consultation uniquement")),
      el(
        "span",
        "authenticated-scope",
        `Client : ${identity?.tenant_id ?? "Inconnu"} · Source : collecteur · Horaires de votre navigateur`,
      ),
    ),
  );
}
function metricCard(
  label: string,
  value: unknown,
  glyph: string,
  detail: string,
  tone = "",
  declared?: number | null,
): HTMLElement {
  return el(
    "article",
    `metric-card ${tone}`,
    el(
      "div",
      "metric-top",
      el("span", "", label),
      el("span", "metric-icon", icon(glyph)),
    ),
    el("strong", "metric-value", formatMetric(value)),
    el("span", "metric-detail", detail),
    declared === undefined
      ? null
      : el(
          "span",
          "metric-declared",
          declared === null
            ? "Déclarés : inconnu"
            : `+ ${formatMetric(declared)} déclaré${declared === 1 ? "" : "s"}`,
        ),
  );
}
function renderStats() {
  if (!statSlot) return;
  const counts = overview?.counts;
  if (view === "security") {
    statSlot.replaceChildren(
      el(
        "div",
        "stats-grid",
        metricCard(
          "Actions autorisées",
          counts?.allowed,
          "check",
          "Décisions consignées",
        ),
        metricCard(
          "Actions refusées",
          counts?.denied,
          "shield",
          "Décisions consignées",
          "metric-danger",
        ),
        metricCard(
          "À confirmer",
          counts?.pending,
          "clock",
          "Décisions de type review",
        ),
        metricCard(
          "Événements de sécurité",
          counts?.security,
          "alert",
          "Total fourni par l’API",
        ),
      ),
    );
    return;
  }
  statSlot.replaceChildren(
    el(
      "div",
      "stats-grid",
      metricCard(
        "Agents",
        counts?.agents,
        "agents",
        "Identités observées",
        "",
        counts?.declared_agents,
      ),
      metricCard(
        "Sous-agents",
        counts?.subagents,
        "branch",
        "Liens observés explicites",
        "",
        counts?.declared_subagents,
      ),
      metricCard(
        "Sandboxes",
        counts?.sandboxes,
        "box",
        "Conteneurs enregistrés",
      ),
      metricCard(
        "Commandes",
        counts?.commands,
        "terminal",
        "Commandes consignées",
      ),
    ),
    el(
      "div",
      "decision-strip",
      el(
        "span",
        "decision-strip-title",
        icon("shield"),
        "Décisions du collecteur",
      ),
      el(
        "span",
        "",
        el("span", "small-dot dot-teal"),
        formatMetric(counts?.allowed),
        " autorisées",
      ),
      el(
        "span",
        "",
        el("span", "small-dot dot-red"),
        formatMetric(counts?.denied),
        " refusées",
      ),
      el(
        "span",
        "",
        el("span", "small-dot dot-amber"),
        formatMetric(counts?.pending),
        " à confirmer",
      ),
      el(
        "span",
        "decision-strip-end",
        "Couverture ",
        coverage(overview?.coverage),
      ),
    ),
  );
}

function filterForm(): HTMLElement {
  const search = el("input", "search-input");
  search.type = "search";
  search.name = "q";
  search.value = filters.q ?? "";
  search.placeholder = "Rechercher une commande, un événement…";
  search.setAttribute("aria-label", "Rechercher dans les événements");
  const period = el("select", "select-input");
  period.setAttribute("aria-label", "Période des événements");
  for (const [value, label] of [
    ["all", "Toutes les périodes"],
    ["hour", "Dernière heure"],
    ["day", "24 heures"],
    ["week", "7 jours"],
  ] as const) {
    const option = el("option", "", label);
    option.value = value;
    option.selected = selectedPeriod === value;
    period.append(option);
  }
  const periodNote = el("p", "period-note");
  const updatePeriodNote = () => {
    periodNote.hidden = !filters.from;
    periodNote.textContent = filters.from
      ? `Depuis le ${formatDate(filters.from, true)} · date de réception`
      : "";
  };
  updatePeriodNote();
  period.addEventListener("change", () => {
    selectedPeriod = period.value as EventPeriod;
    delete filters.from;
    delete filters.to;
    Object.assign(filters, periodFilters(selectedPeriod));
    updatePeriodNote();
    void loadEvents();
  });
  const selectField = (
    name: keyof EventFilters,
    label: string,
    values: [string, string][],
  ): HTMLElement => {
    const select = el("select", "select-input");
    select.name = name;
    select.setAttribute("aria-label", label);
    if (filters[name] && !values.some(([value]) => value === filters[name]))
      values.push([filters[name]!, filters[name]!]);
    for (const [value, text] of [["", label], ...values]) {
      const option = el("option", "", text);
      option.value = value!;
      option.selected = (filters[name] ?? "") === value;
      select.append(option);
    }
    return select;
  };
  const project = el("input", "project-input");
  project.name = "project_id";
  project.placeholder = "ID du projet";
  project.value = filters.project_id ?? "";
  project.setAttribute("aria-label", "Identifiant du projet");
  const kinds = [...new Set(events.map((event) => event.kind))].sort();
  if (filters.kind && !kinds.includes(filters.kind))
    kinds.unshift(filters.kind);
  const form = el(
    "form",
    "filters",
    el("div", "search-field", icon("search"), search),
    el(
      "div",
      "filter-row",
      period,
      selectField(
        "agent_id",
        "Tous les agents",
        agents.map((agent) => [agent.id, asText(agent.name, agent.id)]),
      ),
      selectField(
        "sandbox_id",
        "Toutes les sandboxes",
        sandboxes.map((sandbox) => [sandbox.id, sandbox.id]),
      ),
      selectField(
        "kind",
        "Tous les types",
        kinds.map((kind) => [kind, kind]),
      ),
      selectField("decision", "Toutes les décisions", [
        ["allow", "Autorisées"],
        ["deny", "Refusées"],
        ["review", "À confirmer"],
      ]),
      project,
    ),
    periodNote,
  );
  const submit = el(
    "button",
    "button button-secondary filter-submit",
    "Filtrer",
  );
  submit.type = "submit";
  const reset = button("Réinitialiser les filtres", "reset-button", () => {
    filters = {};
    selectedPeriod = "all";
    form.reset();
    renderView();
    void loadEvents();
  });
  form.querySelector(".filter-row")!.append(submit, reset);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    filters = { from: filters.from, to: filters.to };
    for (const [key, value] of new FormData(form))
      if (typeof value === "string" && value.trim())
        filters[key as keyof EventFilters] = value.trim();
    void loadEvents();
  });
  return form;
}
function updateFilterChoices() {
  if (!page) return;
  const update = (name: string, label: string, values: [string, string][]) => {
    const select = page.querySelector<HTMLSelectElement>(
      `select[name="${name}"]`,
    );
    if (!select) return;
    const selected = select.value;
    if (selected && !values.some(([value]) => value === selected))
      values.push([selected, selected]);
    const next: [string, string][] = [["", label], ...values];
    const previous = [...select.options].map((option) => [
      option.value,
      option.textContent,
    ]);
    if (JSON.stringify(next) === JSON.stringify(previous)) return;
    select.replaceChildren(
      ...next.map(([value, text]) => {
        const option = el("option", "", text);
        option.value = value;
        option.selected = selected === value;
        return option;
      }),
    );
  };
  update(
    "agent_id",
    "Tous les agents",
    agents.map((agent) => [agent.id, asText(agent.name, agent.id)]),
  );
  update(
    "sandbox_id",
    "Toutes les sandboxes",
    sandboxes.map((sandbox) => [sandbox.id, sandbox.id]),
  );
  update(
    "kind",
    "Tous les types",
    [...new Set(events.map((event) => event.kind))]
      .sort()
      .map((kind) => [kind, kind]),
  );
}
function renderEvents() {
  if (!eventSlot) return;
  updateFilterChoices();
  if (eventsError) {
    eventSlot.replaceChildren(
      empty(
        "Journal indisponible",
        "La lecture a échoué. Actualisez la vue pour réessayer ; aucune absence d’événement ne peut être déduite.",
        "alert",
      ),
    );
    eventSlot.setAttribute("aria-busy", "false");
    return;
  }
  const visible = projectEvents(
    events.filter((event) => matchesFilters(event, filters)),
    view,
  );
  if (!visible.length) {
    const hasFilter = Object.values(filters).some(Boolean);
    eventSlot.replaceChildren(
      loadingEvents
        ? empty(
            "Chargement du journal…",
            "Lecture des événements du collecteur.",
          )
        : empty(
            hasFilter
              ? "Aucun événement correspondant"
              : events.length
                ? "Aucun événement de ce type"
                : "Aucun événement reçu",
            hasFilter
              ? "Modifiez votre recherche ou réinitialisez les filtres."
              : events.length
                ? "Vous pouvez charger l’historique antérieur lorsqu’il est disponible."
                : "Les premières traces apparaîtront ici dès qu’un runner ou un agent les transmettra.",
          ),
    );
  } else {
    const table = el("table", "event-table");
    table.append(el("caption", "sr-only", "Journal des événements reçus"));
    table.append(
      el(
        "thead",
        "",
        el(
          "tr",
          "",
          ...[
            "Heure",
            "Événement / commande",
            "Agent",
            "Décision",
            "Provenance",
            "",
          ].map((label) => el("th", "", label)),
        ),
      ),
    );
    const body = el("tbody");
    for (const event of visible) {
      const detailButton = button(
        "",
        "icon-button row-detail",
        () => showEvent(event),
        "chevron",
      );
      detailButton.setAttribute("aria-label", `Voir l’événement ${event.seq}`);
      const kind = el("span", "event-kind", event.kind);
      kind.title = event.kind;
      const summary = el(
        "span",
        event.command ? "event-summary mono" : "event-summary",
        eventSummary(event),
      );
      summary.title = eventSummary(event);
      const time = el("time", "event-time", formatDate(event.timestamp));
      time.dateTime = event.timestamp;
      time.title = formatDate(event.timestamp, true);
      const agentName = agents.find(
        (agent) => agent.id === event.agent_id,
      )?.name;
      body.append(
        el(
          "tr",
          "",
          el("td", "", time, el("span", "sequence", `#${event.seq}`)),
          el("td", "event-main-cell", kind, summary),
          el(
            "td",
            "",
            el(
              "span",
              "agent-cell",
              asText(agentName, asText(event.agent_id, "—")),
            ),
          ),
          el(
            "td",
            "",
            event.decision
              ? decision(event.decision.outcome)
              : el("span", "muted", "—"),
          ),
          el(
            "td",
            "",
            provenance(event.provenance),
            activityChannel(event)
              ? el("span", "muted event-channel", ` · ${activityChannel(event)}`)
              : null,
          ),
          el("td", "", detailButton),
        ),
      );
    }
    table.append(body);
    eventSlot.replaceChildren(el("div", "table-scroll", table));
  }
  const footer = el(
    "div",
    "table-footer",
    el(
      "span",
      "",
      `${formatMetric(visible.length)} événement${visible.length === 1 ? "" : "s"} affiché${visible.length === 1 ? "" : "s"} · ${formatMetric(events.length)} chargé${events.length === 1 ? "" : "s"}`,
    ),
  );
  if (nextCursor !== null) {
    const more = button(
      loadingEvents ? "Chargement…" : "Charger les précédents",
      "button button-quiet",
      () => {
        void loadEvents(true);
      },
    );
    more.disabled = loadingEvents;
    footer.append(more);
  } else footer.append(el("span", "muted", "Fin de l’historique chargé"));
  eventSlot.append(footer);
  eventSlot.setAttribute("aria-busy", String(loadingEvents));
}
function renderSupport() {
  if (!sideSlot) return;
  const collector = overview?.health?.collector;
  const stateRow = (
    label: string,
    value: string | null | undefined,
    type = "",
  ) => {
    const labels: Record<string, string> = {
      ok: "Disponible",
      unknown: "Inconnu",
      disabled: "Désactivé",
      enabled: "Activé",
      configured: "Configuré",
      unavailable: "Indisponible",
    };
    return el(
      "div",
      "health-row",
      el("span", "", label),
      badge(
        labels[value ?? "unknown"] ?? asText(value),
        value === "ok" ? "teal" : type,
      ),
    );
  };
  const state = el(
    "section",
    "panel support-card",
    el(
      "div",
      "support-title",
      icon("pulse"),
      el("h2", "", "État des services"),
    ),
    stateRow("Dashboard", health),
    stateRow("Collecteur", collector),
    stateRow("Moteur de conteneurs", overview?.health?.engine),
    stateRow("Évaluation Jev", overview?.health?.jev),
    el(
      "p",
      "support-footnote",
      `Dernière lecture : ${formatDate(refreshedAt)}`,
    ),
  );
  const resource = el(
    "section",
    "panel support-card",
    el("div", "support-title", icon("cpu"), el("h2", "", "Ressources")),
    el(
      "div",
      "resource-line",
      el("span", "", "Processeur"),
      el(
        "strong",
        "",
        overview?.resources?.cpu_percent == null
          ? "Inconnu"
          : `${formatMetric(overview.resources.cpu_percent)} %`,
      ),
    ),
    el(
      "div",
      "resource-line",
      el("span", "", "Mémoire"),
      el("strong", "", formatBytes(overview?.resources?.memory_bytes)),
    ),
    el("p", "support-footnote", "Les mesures absentes restent inconnues."),
  );
  const audit = el(
    "section",
    "panel support-card",
    el(
      "div",
      "support-title",
      icon("shield"),
      el("h2", "", "Intégrité de l’audit"),
    ),
    badge(
      integrity
        ? integrity.valid
          ? "Chaîne valide"
          : "Altération détectée"
        : "Non vérifiée",
      integrity ? (integrity.valid ? "teal" : "red") : "neutral",
    ),
    el(
      "p",
      "support-copy",
      `${formatMetric(integrity?.event_count)} événements vérifiés par le collecteur.`,
    ),
    el(
      "p",
      "support-footnote",
      "Chaînage local. Une réécriture complète par un administrateur reste hors de cette garantie.",
    ),
  );
  const legend = el(
    "section",
    "panel support-card legend-card",
    el(
      "div",
      "support-title",
      icon("info"),
      el("h2", "", "Comprendre les traces"),
    ),
    el(
      "div",
      "legend-row",
      provenance("observed"),
      el("span", "", "Mesuré ou constaté."),
    ),
    el(
      "div",
      "legend-row",
      provenance("declared"),
      el("span", "", "Fourni par un agent."),
    ),
    el(
      "div",
      "legend-row",
      provenance("inferred"),
      el("span", "", "Déduit, à confirmer."),
    ),
    el(
      "p",
      "support-footnote",
      "Couverture native, observée ou générique : aucune interception universelle n’est présumée.",
    ),
  );
  sideSlot.replaceChildren(
    state,
    ...(view === "overview" ? [resource] : []),
    audit,
    legend,
  );
}

function agentName(agent: Agent): HTMLElement {
  return el(
    "div",
    "entity-name",
    el("span", "entity-avatar", icon("agents")),
    el(
      "div",
      "",
      el("strong", "", asText(agent.name, agent.id)),
      el("span", "mono muted entity-id", agent.id),
    ),
  );
}
function renderAgents() {
  if (!collectionSlot || view !== "agents") return;
  if (agentsAvailable !== true) {
    collectionSlot.replaceChildren(
      el(
        "div",
        "panel",
        empty(
          agentsAvailable === null
            ? "Chargement des agents…"
            : "Données des agents indisponibles",
          "La liste ne peut pas être établie tant que cette lecture n’a pas abouti.",
          "agents",
        ),
      ),
    );
    return;
  }
  if (!agents.length) {
    collectionSlot.replaceChildren(
      el(
        "div",
        "panel",
        empty(
          "Aucun agent enregistré",
          "Les agents apparaissent lorsqu’un événement fournit leur identifiant.",
          "agents",
        ),
      ),
    );
    return;
  }
  if (treeMode) {
    const renderNode = (node: AgentNode, depth = 0): HTMLElement => {
      const detail = button("Détails", "button button-quiet", () =>
        showAgent(node.agent),
      );
      const li = el(
        "li",
        "agent-tree-node",
        el(
          "div",
          "tree-card",
          agentName(node.agent),
          el(
            "div",
            "tree-badges",
            status(node.agent.status),
            provenance(node.agent.provenance),
            detail,
          ),
        ),
      );
      li.dataset.agentId = node.agent.id;
      if (
        node.issue ||
        node.agent.parent_unresolved ||
        node.agent.hierarchy_warning
      )
        li.append(
          el(
            "p",
            "tree-note",
            node.agent.hierarchy_warning ??
              (node.issue === "cycle"
                ? "Lien parent cyclique : agent présenté séparément."
                : `Parent non reçu : ${asText(node.agent.parent_unresolved ?? node.agent.parent_id)}`),
          ),
        );
      if (node.children.length) {
        if (depth >= 30)
          li.append(
            el(
              "p",
              "tree-note",
              "Profondeur d’affichage limitée. Consultez le tableau pour les agents suivants.",
            ),
          );
        else
          li.append(
            el(
              "ul",
              "agent-tree-children",
              ...node.children.map((child) => renderNode(child, depth + 1)),
            ),
          );
      }
      return li;
    };
    collectionSlot.replaceChildren(
      el(
        "ul",
        "agent-tree",
        ...buildAgentForest(agents).map((node) => renderNode(node)),
      ),
    );
    return;
  }
  const table = el("table", "entity-table");
  table.append(
    el("caption", "sr-only", "Agents enregistrés"),
    el(
      "thead",
      "",
      el(
        "tr",
        "",
        ...[
          "Agent",
          "État",
          "Fournisseur / modèle",
          "Couverture",
          "Provenance",
          "",
        ].map((label) => el("th", "", label)),
      ),
    ),
  );
  const body = el("tbody");
  for (const agent of agents)
    body.append(
      el(
        "tr",
        "",
        el("td", "", agentName(agent)),
        el("td", "", status(agent.status)),
        el(
          "td",
          "",
          el(
            "div",
            "stacked",
            el("span", "", asText(agent.provider)),
            el(
              "span",
              "muted field-with-provenance",
              asText(agent.model),
              agent.field_provenance?.model
                ? provenance(agent.field_provenance.model)
                : null,
            ),
          ),
        ),
        el("td", "", coverage(agent.compatibility)),
        el("td", "", provenance(agent.provenance)),
        el(
          "td",
          "",
          button("Détails", "button button-quiet", () => showAgent(agent)),
        ),
      ),
    );
  table.append(body);
  collectionSlot.replaceChildren(el("div", "panel table-scroll", table));
}
function renderSandboxes() {
  if (!collectionSlot || view !== "sandboxes") return;
  if (sandboxesAvailable !== true) {
    collectionSlot.replaceChildren(
      el(
        "div",
        "panel full-width",
        empty(
          sandboxesAvailable === null
            ? "Chargement des sandboxes…"
            : "Données des sandboxes indisponibles",
          "La liste ne peut pas être établie tant que cette lecture n’a pas abouti.",
          "box",
        ),
      ),
    );
    return;
  }
  if (!sandboxes.length) {
    collectionSlot.replaceChildren(
      el(
        "div",
        "panel full-width",
        empty(
          "Aucune sandbox enregistrée",
          "Les paramètres apparaîtront après la transmission d’un événement de création.",
          "box",
        ),
      ),
    );
    return;
  }
  collectionSlot.replaceChildren(
    ...sandboxes.map((sandbox) =>
      el(
        "article",
        "panel sandbox-card",
        el(
          "div",
          "sandbox-heading",
          el("span", "entity-avatar", icon("box")),
          status(sandbox.status),
        ),
        el("h2", "mono", sandbox.id),
        el("p", "muted sandbox-image", asText(sandbox.image)),
        el(
          "dl",
          "sandbox-meta",
          labelValue("Moteur", sandbox.engine),
          labelValue("Profil", sandbox.profile),
          labelValue(
            "Réseau",
            sandbox.network === "none" ? "Désactivé" : sandbox.network,
          ),
          labelValue("Projet", sandbox.project_id),
        ),
        el(
          "div",
          "sandbox-footer",
          el("span", "muted", "Lecture seule"),
          button(
            "Voir les détails",
            "button button-quiet",
            () => showSandbox(sandbox),
            "arrow",
          ),
        ),
      ),
    ),
  );
}

function openDialog(
  title: string,
  eyebrow: string,
): { dialog: HTMLDialogElement; body: HTMLElement } {
  const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  dialog?.remove();
  detailEntityLinks = [];
  dialog = el("dialog", "detail-dialog");
  dialog.setAttribute("aria-labelledby", "detail-title");
  const heading = el("h2", "", title);
  heading.id = "detail-title";
  const close = button("", "icon-button", () => dialog?.close(), "x");
  close.setAttribute("aria-label", "Fermer les détails");
  const body = el("div", "detail-body");
  dialog.append(
    el(
      "div",
      "detail-header",
      el("div", "", el("div", "eyebrow", eyebrow), heading),
      close,
    ),
    body,
  );
  const currentDialog = dialog;
  document.body.append(currentDialog);
  currentDialog.addEventListener("close", () => {
    if (document.querySelector("dialog[open]") || !client) return;
    const fallback = document.querySelector<HTMLElement>("#main-content h1");
    (trigger?.isConnected ? trigger : fallback)?.focus({ preventScroll: true });
  }, { once: true });
  currentDialog.addEventListener("click", (event) => {
    if (event.target === currentDialog) {
      const bounds = currentDialog.getBoundingClientRect();
      if (
        event.clientX < bounds.left ||
        event.clientX > bounds.right ||
        event.clientY < bounds.top ||
        event.clientY > bounds.bottom
      )
        currentDialog.close();
    }
  });
  currentDialog.showModal();
  return { dialog: currentDialog, body };
}
function rawBlock(value: unknown): HTMLElement {
  const block = el("pre", "code-block");
  block.textContent = JSON.stringify(value, null, 2);
  return block;
}
function textBlock(value: unknown): HTMLElement {
  const block = el("pre", "code-block");
  block.textContent = asText(value);
  return block;
}
function entityLinks<T extends Agent | Sandbox>(kind: "agents" | "sandboxes", items: T[],
  render: (item: T) => HTMLElement): HTMLElement {
  const loadedAt = kind === "agents" ? agentsLoadedAt : sandboxesLoadedAt;
  const error = kind === "agents" ? agentsError : sandboxesError;
  const section = el("section", "entity-link-source");
  section.setAttribute("aria-label", kind === "agents" ? "Agents correspondants" : "Sandboxes correspondantes");
  const status = el("div", "control-status", badge(summaryLoading ? "Actualisation des liens…" :
    error ? loadedAt ? "Liens périmés · dernière lecture conservée" : "Liens indisponibles" : loadedAt ? "Lecture des liens réussie" : "Liens inconnus",
    error ? "amber" : "neutral"),
    el("span", "muted", loadedAt ? `Lu le ${formatDate(loadedAt, true)}` : "Aucune lecture réussie"));
  status.setAttribute("role", "status");
  const warning = error ? el("p", "control-warning", error) : null;
  warning?.setAttribute("role", "alert");
  section.append(el("h3", "", kind === "agents" ? "Agents correspondants" : "Sandboxes correspondantes"),
    status, el("p", "muted", `Source : /api/${kind}, projection du journal du client authentifié. Les observations absentes du journal restent inconnues ; ces liens ne constituent pas un inventaire runtime.`),
    streamStatus !== "live" ? badge("Flux déconnecté · lectures ponctuelles seulement", "amber") : "",
    warning ?? "",
    !loadedAt ? el("p", "muted", "Les correspondances restent inconnues tant que cette source n’a pas été lue.") :
      items.length ? el("ul", "resource-link-list", ...items.map((item) => el("li", "", render(item)))) :
        el("p", "muted", "Aucune correspondance dans les données reçues de cette source. L’absence de trace ne prouve pas l’absence de ressource."));
  if (error) {
    const retry = entityLink("Réessayer la source du journal", `retry-source-${kind}`, () => { void refreshSummary(); });
    retry.disabled = summaryLoading;
    section.append(retry);
  }
  return section;
}
function entityLink(label: string, key: string, open: () => void) {
  const node = button(label, "button button-quiet", open);
  node.dataset.focusKey = key;
  return node;
}
function workerEntities(worker: WorkerRecord): HTMLElement {
  const section = el("section", "worker-related-resources",
    el("p", "muted", "Les correspondances utilisent uniquement les identifiants de session ou de runtime reçus. Une session partagée ne prouve ni délégation ni exécution."),
    entityLinks("sandboxes", relatedSandboxes(worker, sandboxes), (sandbox) => el("div", "resource-link",
      entityLink(sandbox.id, `worker-sandbox-${sandbox.id}`, () => showSandbox(sandbox)),
      el("span", "muted", workerRelation({ session_id: sandbox.session_id, sandbox_id: sandbox.id }, worker).join(" · ")))),
    entityLinks("agents", relatedAgents(worker, agents), (agent) => el("div", "resource-link",
      entityLink(asText(agent.name, agent.id), `worker-agent-${agent.id}`, () => showAgent(agent)),
      ...workerRelation(agent, worker).map((reason) => el("span", "resource-link-evidence", el("span", "muted", reason),
        provenance(agent.field_provenance?.[reason === "Session identique" ? "session_id" : "sandbox_id"]))))),
    entityLink(summaryLoading ? "Actualisation des sources…" : "Actualiser les sources du journal", "refresh-worker-entities", () => { void refreshSummary(); }));
  return section;
}
function agentSandbox(agent: Agent): HTMLElement {
  return trackedEntityLinks(() => entityLinks("sandboxes", sandboxes.filter((sandbox) => sandbox.id === agent.sandbox_id), (sandbox) =>
    el("div", "resource-link", entityLink(sandbox.id, `agent-sandbox-${sandbox.id}`, () => showSandbox(sandbox)),
      el("span", "muted", "Identifiant sandbox identique"))));
}
function sandboxAgents(sandbox: Sandbox): HTMLElement {
  return trackedEntityLinks(() => entityLinks("agents", agents.filter((agent) => agent.sandbox_id === sandbox.id), (agent) =>
    el("div", "resource-link", entityLink(asText(agent.name, agent.id), `sandbox-agent-${agent.id}`, () => showAgent(agent)),
      el("span", "muted", "Identifiant sandbox identique"), provenance(agent.field_provenance?.sandbox_id))));
}
function trackedEntityLinks(render: () => HTMLElement): HTMLElement {
  const slot = el("div", "detail-entity-links", render());
  detailEntityLinks.push({ slot, render });
  return slot;
}
function updateDetailEntityLinks() {
  if (!dialog?.open) return;
  for (const { slot, render } of detailEntityLinks) {
    if (!slot.isConnected) continue;
    const focused = slot.contains(document.activeElement) ? document.activeElement as HTMLElement : null;
    const key = focused?.dataset.focusKey;
    slot.replaceChildren(render());
    if (focused) {
      const replacement = key ? [...slot.querySelectorAll<HTMLElement>("[data-focus-key]")]
        .find((node) => node.dataset.focusKey === key && node.isConnected && !node.matches(":disabled") &&
          !node.closest("[hidden], [inert]") && node.getClientRects().length > 0) : null;
      const fallback = slot.closest("dialog")?.querySelector<HTMLElement>(".detail-header button:not(:disabled)");
      (replacement ?? fallback)?.focus({ preventScroll: true });
    }
  }
}
function showAgent(agent: Agent) {
  const { body } = openDialog(asText(agent.name, agent.id), "AGENT");
  body.append(
    el(
      "div",
      "detail-badges",
      status(agent.status),
      provenance(agent.provenance),
      coverage(agent.compatibility),
    ),
    el(
      "p",
      "field-provenance-help",
      "Le badge global décrit l’identité. La provenance d’un modèle ou d’une tâche reste distincte lorsqu’elle est fournie.",
    ),
    agent.hierarchy_warning || agent.parent_unresolved
      ? el(
          "div",
          "info-strip",
          icon("info"),
          agent.hierarchy_warning ??
            `Parent non résolu : ${asText(agent.parent_unresolved)}`,
        )
      : "",
    el(
      "dl",
      "detail-grid",
      labelValue("Identifiant", agent.id, agent.field_provenance?.id),
      labelValue(
        "Fournisseur",
        agent.provider,
        agent.field_provenance?.provider,
      ),
      labelValue("Modèle", agent.model, agent.field_provenance?.model),
      labelValue(
        "Parent explicite",
        agent.parent_id,
        agent.field_provenance?.parent_id,
      ),
      labelValue(
        "Projet",
        agent.project_id,
        agent.field_provenance?.project_id,
      ),
      labelValue(
        "Sandbox",
        agent.sandbox_id,
        agent.field_provenance?.sandbox_id,
      ),
      labelValue(
        "Session",
        agent.session_id,
        agent.field_provenance?.session_id,
      ),
      labelValue(
        "Début",
        formatDate(agent.started_at, true),
        agent.field_provenance?.started_at,
      ),
      labelValue(
        "Dernière activité",
        formatDate(agent.last_activity, true),
        agent.field_provenance?.last_activity,
      ),
    ),
    el(
      "h3",
      "field-with-provenance",
      "Tâche",
      agent.field_provenance?.task
        ? provenance(agent.field_provenance.task)
        : null,
    ),
    el("p", "detail-task", asText(agent.task)),
    agentSandbox(agent),
    control?.agentWorkers(agent) ?? "",
    button(
      "Voir ses événements",
      "button button-primary",
      () => {
        dialog?.close();
        filters = { agent_id: agent.id, from: filters.from, to: filters.to };
        navigate("actions");
        void loadEvents();
      },
      "arrow",
    ),
    el("h3", "", "Données reçues"),
    rawBlock(agent),
  );
}
function showSandbox(sandbox: Sandbox) {
  const { body } = openDialog(sandbox.id, "SANDBOX");
  body.append(
    el("div", "detail-badges", status(sandbox.status)),
    el(
      "dl",
      "detail-grid",
      labelValue("Image", sandbox.image),
      labelValue("Moteur", sandbox.engine),
      labelValue("Profil", sandbox.profile),
      labelValue(
        "Réseau",
        sandbox.network === "none" ? "Désactivé" : sandbox.network,
      ),
      labelValue("Projet", sandbox.project_id),
      labelValue("Session", sandbox.session_id),
      labelValue("Début", formatDate(sandbox.started_at, true)),
      labelValue(
        "Processeur",
        sandbox.resources?.cpu_percent == null
          ? null
          : `${formatMetric(sandbox.resources.cpu_percent)} %`,
      ),
      labelValue("Mémoire", formatBytes(sandbox.resources?.memory_bytes)),
    ),
    el("h3", "", "Montages reçus"),
    rawBlock(sandbox.mounts ?? null),
    sandboxAgents(sandbox),
    control?.sandboxWorkers(sandbox) ?? "",
    button(
      "Voir ses événements",
      "button button-primary",
      () => {
        dialog?.close();
        filters = {
          sandbox_id: sandbox.id,
          from: filters.from,
          to: filters.to,
        };
        navigate("actions");
        void loadEvents();
      },
      "arrow",
    ),
    el("h3", "", "Données reçues"),
    rawBlock(sandbox),
  );
}
function brokerWorkspaceContent(event: AuditEvent) {
  const workspace = brokerWorkspaceDetails(event);
  if (!workspace) {
    return [el("p", "detail-task", "Détails de fichiers indisponibles. L’événement reçu reste consultable dans Données brutes.")];
  }
  if (workspace.kind === "export") {
    return [
      el("h3", "", "Export publié"),
      el(
        "dl", "detail-grid",
        labelValue("Fichiers publiés", workspace.files),
        labelValue("Empreinte du manifeste", workspace.manifestDigest),
        labelValue("Demande d’export", workspace.requestId),
      ),
    ];
  }
  const changeLabels = { created: "Créé", modified: "Modifié", deleted: "Supprimé" };
  return [el("section", "",
    el("h3", "", "Modifications observées"),
    el(
      "dl", "detail-grid",
      labelValue("Nombre de modifications", workspace.changedFiles),
      labelValue("Empreinte du manifeste", workspace.manifestDigest),
    ),
    workspace.truncated
      ? el("p", "detail-task", `${formatMetric(workspace.changes.length)} modifications détaillées sur ${formatMetric(workspace.changedFiles)}.`)
      : null,
    workspace.changes.length ? el(
      "div", "table-scroll",
      el(
        "table", "entity-table workspace-change-table",
        el("thead", "", el("tr", "", ...["Chemin", "Changement", "Empreinte avant", "Empreinte après"].map(
          (label) => el("th", "", label),
        ))),
        el("tbody", "", ...workspace.changes.map((change) => el(
          "tr", "",
          el("td", "mono", change.path),
          el("td", "", changeLabels[change.change]),
          el("td", "mono", change.beforeSha256 ?? "—"),
          el("td", "mono", change.afterSha256 ?? "—"),
        ))),
      ),
    ) : null,
  )];
}

function brokerCaptureContent(title: string, label: string, capture: BrokerCapture) {
  const states = { retained: "Texte conservé", not_retained: "Texte non conservé", unknown: "Inconnue" };
  return el("section", "",
    el("h3", "", title),
    el("dl", "detail-grid", labelValue(label, states[capture.state])),
    capture.text === null ? null : textBlock(capture.text === "" ? "Sortie vide" : capture.text),
    capture.truncated ? el("p", "muted", "Flux tronqué lors de la collecte.") : null,
    capture.textClipped ? el("p", "muted", "Texte conservé tronqué par le broker.") : null);
}

function brokerBatchCommandContent(event: AuditEvent) {
  const result = brokerBatchResultDetails(event);
  if (!result) return [el("p", "detail-task", "Résultat broker indisponible. Les données reçues restent consultables dans Données brutes.")];
  return [
    ...(event.command ? [el("h3", "", "Arguments reçus"), rawBlock(event.command.argv ?? null)] : []),
    el("h3", "", "Résultat de l’exécution"),
    el("dl", "detail-grid",
      event.command ? labelValue("Répertoire", event.command.cwd) : null,
      labelValue("Code de sortie observé", result.observedExitCode),
      labelValue("Début observé", formatDate(result.startedAt, true)),
      labelValue("Fin observée", formatDate(result.finishedAt, true)),
      labelValue("Arrêt mémoire (OOM)", result.oomKilled === null ? null : result.oomKilled ? "Oui" : "Non"),
      labelValue("Code de sortie du transport", result.transportExitCode),
      labelValue("Durée du transport", result.transportDurationMs === null ? null : result.transportDurationMs + " ms"),
    ),
    brokerCaptureContent("Sortie standard", "Capture de la sortie standard", result.stdout),
    brokerCaptureContent("Sortie d’erreur", "Capture de la sortie d’erreur", result.stderr),
  ];
}

function activityCountsRows(label: string, counts: ActivityChannelCounts | null) {
  if (!counts) return [labelValue(label, "Non applicable à cette session")];
  const losses = counts.dropped + counts.refused + counts.oversized + counts.invalid;
  return [
    labelValue(
      label,
      `${formatMetric(counts.records)} enregistrement${counts.records === 1 ? "" : "s"} · ${formatMetric(counts.events)} événement${counts.events === 1 ? "" : "s"}`,
    ),
    labelValue(
      `${label} · pertes`,
      losses || counts.declaredLosses
        ? `${formatMetric(losses)} observée${losses === 1 ? "" : "s"} · ${formatMetric(counts.declaredLosses)} déclarée${counts.declaredLosses === 1 ? "" : "s"} par le worker`
        : "Aucune perte comptée",
    ),
    labelValue(
      `${label} · non projetés`,
      `${formatMetric(counts.ignored)} ignoré${counts.ignored === 1 ? "" : "s"} · ${formatMetric(counts.unrecognized)} non reconnu${counts.unrecognized === 1 ? "" : "s"}`,
    ),
  ];
}
function activityContent(event: AuditEvent) {
  const details = activityDetails(event);
  if (!details) return null;
  const section = el(
    "section",
    "activity-detail",
    el("h3", "", "Activité du client IA"),
    details.declaredByClient
      ? el(
          "p",
          "detail-task",
          "Déclaration du client IA, reçue par ai-guard sans vérification indépendante. Les effets observés par ai-guard sont le cycle de vie du worker, son code de sortie, les changements de l’espace de travail et les décisions.",
        )
      : null,
    el(
      "dl",
      "detail-grid",
      labelValue("Canal", details.channel),
      labelValue("Client", details.client),
      labelValue("Version demandée pour l’image", details.versionLabel ?? "Inconnue"),
      labelValue("Mode", details.interactive === null ? null : details.interactive ? "Interactif" : "Batch"),
      labelValue("Réception par ai-guard", formatDate(details.receivedAt, true), "observed"),
      labelValue("Worker", details.workerId),
      labelValue("Tentative", details.attemptId),
    ),
    details.truncated
      ? el("div", "info-strip", icon("info"), "Contenu tronqué par le collecteur avant stockage.")
      : null,
  );
  if (details.coverage) {
    const c = details.coverage;
    section.append(
      el("h3", "", "Couverture de la collecte"),
      el(
        "div",
        "info-strip",
        icon("info"),
        c.complete
          ? "Collecte complète pour les canaux attachés. Elle ne couvre pas les actions faites hors des points de hook du client."
          : "Collecte partielle : des enregistrements manquent ou n’ont pas été livrés. Une absence d’événement ne prouve pas une absence d’activité.",
      ),
      el(
        "dl",
        "detail-grid",
        labelValue("Événements conservés", c.spooledEvents),
        labelValue("Événements livrés au contrôle", c.deliveredEvents),
        labelValue("Canal des hooks", c.hookChannel),
        labelValue("Budget de session épuisé", c.budgetExhausted ? "Oui" : "Non"),
        labelValue("Audit indisponible à la fin", c.auditUnavailable ? "Oui" : "Non"),
        labelValue("Lecture suspendue pendant la session", c.stalled ? "Oui" : "Non"),
        ...activityCountsRows("Sortie structurée", c.stdout),
        ...activityCountsRows("Hooks", c.hook),
      ),
    );
  }
  return section;
}
function decisionEvaluationContent(event: AuditEvent) {
  const evaluation = decisionEvaluationDetails(event);
  if (!evaluation) return null;
  return el("section", "",
    el("h3", "", "Évaluation complémentaire"),
    el("dl", "detail-grid",
      labelValue("Évaluateur", evaluation.evaluator),
      labelValue("État de l’évaluation", evaluation.evaluationState),
      evaluation.selectionSource ? labelValue("Sélection de l’évaluateur", evaluation.selectionSource) : null,
      evaluation.requestedModel ? labelValue("Modèle demandé", evaluation.requestedModel) : null,
      evaluation.returnedModel ? labelValue("Modèle retourné", evaluation.returnedModel) : null,
      evaluation.advice ? labelValue("Avis du modèle", evaluation.advice) : null,
    ),
    evaluation.localAdvisory
      ? el("p", "detail-task", "Le modèle local intervient à titre consultatif. L’autorisation d’exécuter relève de la politique Rust.")
      : null,
  );
}

function showEvent(event: AuditEvent) {
  const failure = brokerFailureDetails(event);
  const { body } = openDialog(event.kind, `ÉVÉNEMENT #${event.seq}`);
  const tabs = el("div", "detail-tabs");
  tabs.setAttribute("role", "tablist");
  tabs.setAttribute("aria-label", "Détails de l’événement");
  const content = el("div", "tab-content");
  content.setAttribute("role", "tabpanel");
  content.id = "event-tab-panel";
  const render = (key: string) => {
    for (const tab of tabs.querySelectorAll<HTMLButtonElement>("button")) {
      const selected = tab.dataset.tab === key;
      tab.setAttribute("aria-selected", String(selected));
      tab.tabIndex = selected ? 0 : -1;
      if (selected) content.setAttribute("aria-labelledby", tab.id);
    }
    if (key === "raw") {
      content.replaceChildren(rawBlock(event));
      return;
    }
    if (key === "command") {
      if (failure) {
        content.replaceChildren(
          el("h3", "", "Échec signalé"),
          el(
            "p",
            "detail-task",
            "L’observation porte sur l’échec du broker. L’état final de l’exécution reste à confirmer.",
          ),
        );
        return;
      }
      if (event.kind === "broker.batch.result") {
        content.replaceChildren(...brokerBatchCommandContent(event));
        return;
      }
      if (!event.command && !event.result) {
        content.replaceChildren(
          empty(
            "Aucune commande fournie",
            "Cet événement ne contient ni commande ni sortie associée.",
            "terminal",
          ),
        );
        return;
      }
      const stopReason = event.result?.transport_stop_reason;
      const observedTerminal =
        (event.kind === "command.completed" || event.kind === "command.failed") &&
        event.source === "runner" &&
        event.provenance === "observed" &&
        [event.session_id, event.data?.worker_id].every(
          (id) => uuid(id) && id !== "00000000-0000-0000-0000-000000000000",
        );
      const controlDeadline = observedTerminal ? event.result?.control_deadline : null;
      const deadlineLabel = controlDeadlineLabels.get(controlDeadline?.deadline);
      const controlStopLabel = controlStopLabels.get(controlDeadline?.stop);
      content.replaceChildren(
        el("h3", "", "Arguments reçus"),
        rawBlock(event.command?.argv ?? null),
        el(
          "dl",
          "detail-grid",
          labelValue("Répertoire", event.command?.cwd),
          labelValue("Code de sortie", event.result?.exit_code),
          event.result?.timed_out === true ? labelValue("État de la commande", "Délai dépassé") : null,
          typeof stopReason === "string" && stopReason.trim()
            ? labelValue("Motif de l’arrêt", stopReason)
            : null,
          deadlineLabel ? labelValue("Échéance du contrôle", deadlineLabel, event.provenance) : null,
          deadlineLabel && controlStopLabel
            ? labelValue("Observation de la demande d’arrêt", controlStopLabel, event.provenance)
            : null,
          labelValue(
            "Durée",
            event.result?.duration_ms == null
              ? null
              : `${formatMetric(event.result.duration_ms)} ms`,
          ),
        ),
        el("h3", "", "Sortie standard"),
        textBlock(event.result?.stdout),
        el("h3", "", "Sortie d’erreur"),
        textBlock(event.result?.stderr),
      );
      if (event.result?.truncated)
        content.append(
          el(
            "div",
            "info-strip",
            icon("info"),
            "La sortie reçue a été tronquée par le collecteur.",
          ),
        );
      return;
    }
    const details = failure
      ? [
          el("h3", "", "Échec signalé"),
          el(
            "dl",
            "detail-grid",
            labelValue("Phase", failure.phase),
            labelValue("Étape", failure.step),
            labelValue("Effet sur l’exécution", failure.runtimeEffect),
          ),
          el(
            "p",
            "detail-task",
            "L’observation porte sur l’échec du broker. La phase et l’étape situent le signalement ; l’état final de l’exécution reste à confirmer.",
          ),
        ]
      : event.kind === "broker.batch.scan" || event.kind === "broker.batch.export"
      ? brokerWorkspaceContent(event)
      : [
          el("h3", "", "Décision"),
          el(
            "dl",
            "detail-grid",
            labelValue("Moteur", event.decision?.engine),
            labelValue("Règle", event.decision?.rule),
          ),
          el("p", "detail-task", asText(event.decision?.reason)),
        ];
    const evaluation = decisionEvaluationContent(event);
    content.replaceChildren(
      el(
        "div",
        "detail-badges",
        provenance(event.provenance),
        coverage(event.coverage),
        !failure && event.decision ? decision(event.decision.outcome) : null,
      ),
      el(
        "dl",
        "detail-grid",
        labelValue("Horodatage source", formatDate(event.timestamp, true)),
        labelValue("Réception", formatDate(event.received_at, true)),
        labelValue("Source", event.source),
        labelValue("Identifiant", event.id),
        labelValue("Agent", event.agent_id),
        labelValue("Parent explicite", event.parent_agent_id),
        labelValue("Sandbox", event.sandbox_id),
        labelValue("Projet", event.project_id),
        labelValue("Session", event.session_id),
        labelValue("Corrélation", event.correlation_id),
        labelValue("Trace", event.trace_id),
      ),
      ...details,
      ...(evaluation ? [evaluation] : []),
      ...[activityContent(event)].filter((part): part is HTMLElement => part !== null),
      el("h3", "", "Chaînage d’audit"),
      el(
        "dl",
        "detail-grid single-column",
        labelValue("Empreinte", event.hash),
        labelValue("Empreinte précédente", event.prev_hash),
      ),
    );
  };
  for (const [key, label] of [
    ["summary", "Résumé"],
    ["command", "Commande"],
    ["raw", "Données brutes"],
  ] as const) {
    const tab = button(label, "detail-tab", () => render(key));
    tab.id = `event-tab-${key}`;
    tab.dataset.tab = key;
    tab.setAttribute("role", "tab");
    tab.setAttribute("aria-controls", content.id);
    tabs.append(tab);
  }
  tabs.addEventListener("keydown", (event) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const buttons = [...tabs.querySelectorAll("button")];
    const current = buttons.indexOf(
      document.activeElement as HTMLButtonElement,
    );
    const index =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? buttons.length - 1
          : (current + (event.key === "ArrowRight" ? 1 : -1) + buttons.length) %
            buttons.length;
    const next = buttons[index];
    next?.focus();
    render(next?.dataset.tab ?? "summary");
  });
  body.append(tabs, content);
  render("summary");
}
async function exportJournal(button: HTMLButtonElement) {
  const current = client;
  if (!current) return;
  button.disabled = true;
  const exportFilters = { ...filters };
  try {
    const blob = await current.export(exportFilters);
    if (current !== client) return;
    const url = URL.createObjectURL(blob);
    const link = el("a");
    link.href = url;
    link.download = `ai-guard-audit-${new Date().toISOString().slice(0, 10)}.jsonl`;
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    setNotice(
      Object.values(exportFilters).some(Boolean)
        ? "Les événements correspondant aux filtres appliqués ont été exportés au format JSONL, toutes pages incluses."
        : "Le journal complet a été exporté au format JSONL, toutes pages incluses.",
      "info",
    );
  } catch (error) {
    if (current === client) onError(error);
  } finally {
    button.disabled = false;
  }
}
function browserOidcSession(): OidcSession {
  let session: OidcSession;
  session = new OidcSession(fetch, {
    onChange: () => { if (oidcSession === session) void mountOidc(session); },
    onClose: (message) => {
      if (oidcSession !== session) return;
      oidcSession = null; humanSession?.close(); humanSession = null; login(message, "human");
    },
  });
  return session;
}
async function mountOidc(session: OidcSession): Promise<void> {
  const token = session.accessToken; if (!token || oidcSession !== session) return;
  // A token rotation replaces the dashboard. Hand over at most the open view and its typed filters;
  // the successor reads the identity and asks for confirmation before any data read.
  const resume = humanSession?.continuity() ?? null;
  ++authenticationView; humanSession?.close();
  const candidate = new HumanDashboard(root, token, { brand, themeButton }, () => {
    if (humanSession !== candidate || oidcSession !== session) return;
    candidate.close(); humanSession = null;
    const pending = session.logout(); const closedView = authenticationView;
    void pending.then((result) => {
      if (authenticationView !== closedView) return;
      const provider = result.provider_logout === "accepted" ? "Fermeture auprès du fournisseur acceptée." :
        "Fermeture auprès du fournisseur non confirmée ; sa session peut rester ouverte.";
      login(`Session locale fermée. ${provider}${result.audit === "unavailable" ? " Audit de fermeture indisponible." : ""}`, "human");
    }).catch(() => {
      if (authenticationView === closedView) login("Session locale fermée. Fournisseur et audit de fermeture non confirmés.", "human");
    });
  }, resume);
  humanSession = candidate;
  try { await candidate.start(); }
  catch (cause) {
    if (humanSession !== candidate || oidcSession !== session) return;
    candidate.close(); humanSession = null; session.close(errorMessage(cause));
  }
}
const buildFooter = buildIdentityFooter(FRONTEND_IDENTITY);
root.after(buildFooter.node);
window.addEventListener("beforeunload", () => { buildFooter.close(); client?.close(); humanSession?.close(); oidcSession?.close(); });
if (window.location.pathname === "/auth/oidc/complete") {
  window.history.replaceState(null, "", "/auth/oidc/complete");
  login("Vérification de la connexion SSO…", "human");
  const session = browserOidcSession(); oidcSession = session;
  void session.redeem().then(() => window.history.replaceState(null, "", "/")).catch((error) => {
    if (oidcSession === session) { oidcSession = null; login(errorMessage(error), "human"); }
  });
} else login();
