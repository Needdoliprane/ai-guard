// Administration views of the 360° dashboard: memberships, quota and usage, configuration.
// Each view reads the offered Rust routes with the selected context, shows their provenance and read time, keeps the last
// successful page dated and marked stale after a failure, and withdraws everything on a refusal or on close.
import { el } from "./dom.ts";
import { formatDate } from "./model.ts";
import { HumanApiError, HumanSuperseded } from "./human-api.ts";
import { accessExplanation, can } from "./access-model.ts";
import type { HumanContext, Principal } from "./human-model.ts";
import type { HumanConfiguration, HumanQuota, Membership, MembershipPage, UsagePage } from "./admin-model.ts";
import { gibibytes, grouped, perSecond } from "./summary-model.ts";
import "./overview.css";

export interface AdminHost {
  context: HumanContext;
  budgetStopped: () => boolean;
  onAccessDoubt: () => void;
  onReadStateChange: () => void;
}
type Phase = "loading" | "ready" | "error" | "refused";

/** Shared read lifecycle of one administration view: one read in flight, dated result, stale after failure. */
abstract class AdminView<T> {
  readonly element: HTMLElement;
  protected host: AdminHost;
  protected value: T | null = null;
  #phase: Phase = "loading"; #error = ""; #controller: AbortController | null = null; #closed = false;
  #status = el("div", "ov-status"); #body = el("div", "ov-body"); #announce = el("p", "sr-only");
  #refresh = el("button", "button button-secondary", "Actualiser");
  constructor(host: AdminHost, className: string) {
    this.host = host; this.element = el("div", `ov ${className}`);
    this.#announce.setAttribute("role", "status"); this.#announce.setAttribute("aria-live", "polite");
    this.#refresh.type = "button"; this.#refresh.dataset.focusKey = `${className}-refresh`;
    this.#refresh.addEventListener("click", () => { if (!this.reading) void this.read(); });
    this.element.append(el("div", "ov-toolbar", this.#refresh), this.#announce, this.#status, this.#body);
  }
  get reading() { return this.#controller !== null; }
  start() { void this.read(); }
  /** For the host, on an explicit action of the person; a read in flight answers instead. */
  refresh() { return this.read(); }
  close() { this.#closed = true; this.#controller?.abort(); this.#controller = null; this.value = null; this.element.replaceChildren(); }
  protected abstract fetch(signal: AbortSignal): Promise<T>;
  protected abstract render(value: T): HTMLElement[];
  protected abstract evaluatedAt(value: T): string;
  protected abstract receiptId(value: T): string;
  protected async read() {
    if (this.#closed || this.#controller) return;
    const controller = new AbortController(); this.#controller = controller;
    if (!this.value) this.#phase = "loading";
    this.paint(); this.host.onReadStateChange();
    try {
      const value = await this.fetch(controller.signal);
      if (this.#closed || controller.signal.aborted) return;
      this.value = value; this.#phase = "ready"; this.#error = "";
      this.#announce.textContent = `Lecture réussie à ${formatDate(this.evaluatedAt(value), true)}.`;
    } catch (error) {
      if (this.#closed || controller.signal.aborted || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) {
        this.value = null; this.#phase = "refused"; this.#error = error.message;
        this.#announce.textContent = "Lecture refusée par le contrôle : vos accès sont relus.";
        this.host.onAccessDoubt();
      } else {
        this.#phase = "error"; this.#error = error instanceof Error ? error.message : "Lecture impossible.";
        this.#announce.textContent = this.value ? "Nouvelle lecture en échec : la dernière lecture reste affichée, périmée." : "Lecture indisponible.";
      }
    } finally {
      if (this.#controller === controller) this.#controller = null;
      if (!this.#closed) { this.paint(); this.host.onReadStateChange(); }
    }
  }
  protected paint() {
    if (this.#closed) return;
    this.#refresh.setAttribute("aria-disabled", String(this.reading));
    const value = this.value; const stale = this.#phase === "error" && value !== null;
    this.element.dataset.state = stale ? "stale" : this.#phase;
    this.#status.replaceChildren(
      this.#phase === "loading" && !value ? el("p", "muted", "Lecture en cours…") : "",
      value ? el("p", "ov-provenance", el("span", "", "Lecture du contrôle · évaluée le "), el("time", "mono", formatDate(this.evaluatedAt(value), true)),
        el("span", "", " · reçu de lecture "), el("span", "mono", this.receiptId(value).slice(0, 8))) : "",
      stale ? el("div", "control-warning", el("p", "", "Données périmées · dernière lecture conservée. "), el("p", "", this.#error)) : "",
      this.#phase === "error" && !value ? el("div", "control-error", el("p", "", `Lecture indisponible. ${this.#error}`)) : "",
      this.#phase === "refused" ? el("div", "control-error", el("p", "", "Lecture refusée pour ce contexte. Vos accès sont relus ; aucune donnée antérieure n’est conservée.")) : "",
      this.host.budgetStopped() ? el("p", "muted", "Lecture suspendue : budget durable de lecture atteint.") : "");
    this.#body.replaceChildren(...(value ? this.render(value) : []));
  }
}
function th(label: string) { const node = el("th", "", label); node.scope = "col"; return node; }
function principalLabel(principal: Principal) {
  return principal.kind === "human_local" ? `Compte local ${principal.account_id.slice(0, 8)}` : `OIDC ${principal.subject}`;
}
const capabilityLabel: Record<string, string> = {
  read_metadata: "Métadonnées", read_content: "Contenus", export: "Export", read_tenant_metadata: "Métadonnées de l’organisation",
  manage_memberships: "Gestion des appartenances", manage_grants: "Gestion des partages", read_broker_registry: "Registre des brokers",
  read_worker_registry: "Registre des workers", approve_launch: "Approbation de lancement", read_memberships: "Lecture des appartenances",
  read_configuration: "Configuration", launch_session: "Lancement de session", send_session_message: "Consigne à une session",
  approve_egress: "Approbation des sorties réseau",
};

export interface MembershipsHost extends AdminHost {
  read: (cursor: string | null, signal: AbortSignal) => Promise<MembershipPage>;
  /** A delegated change is offered only when the context's catalogue grants and serves it. */
  change: ((membership: Membership) => void) | null;
}
export class MembershipsView extends AdminView<MembershipPage> {
  #cursor: string | null = null; #history: (string | null)[] = [];
  #membershipHost: MembershipsHost;
  constructor(host: MembershipsHost) { super(host, "ov-memberships"); this.#membershipHost = host; }
  protected fetch(signal: AbortSignal) { return this.#membershipHost.read(this.#cursor, signal); }
  protected evaluatedAt(value: MembershipPage) { return value.evaluated_at; }
  protected receiptId(value: MembershipPage) { return value.read_receipt.request_id; }
  protected render(page: MembershipPage) {
    const table = el("table", "ov-groups");
    table.append(el("caption", "", "Appartenances de l’organisation"),
      el("thead", "", el("tr", "", th("Compte"), th("Principal"), th("Capacités"), th("État"), th("Révision"), th("Action"))));
    const body = el("tbody");
    for (const item of page.items) {
      const tr = el("tr"); tr.dataset.membershipId = item.membership_id;
      const action = item.own ? el("span", "muted", "Votre appartenance : changement réservé au contrôle") :
        this.#membershipHost.change ? (() => {
          const button = el("button", "button button-quiet", "Modifier"); button.type = "button";
          button.dataset.focusKey = `ov-member-${item.membership_id}`;
          button.addEventListener("click", () => this.#membershipHost.change?.(item)); return button;
        })() : el("span", "muted", "Lecture seule");
      tr.append(el("td", "", el("strong", "", item.display_name), item.account_active ? "" : el("span", "badge badge-amber", "Compte inactif")),
        el("td", "mono", principalLabel(item.principal)),
        el("td", "", ...(item.capabilities.length ? item.capabilities.map((capability) => el("span", "ov-inline", capabilityLabel[capability] ?? capability)) : [el("span", "muted", "Aucune")])),
        el("td", "", item.active ? el("span", "badge badge-teal", "Active") : el("span", "badge badge-amber", "Inactive")),
        el("td", "mono", item.revision), el("td", "", action));
      body.append(tr);
    }
    table.append(body);
    const pager = el("div", "ov-toolbar");
    if (this.#history.length) {
      const previous = el("button", "button button-quiet", "Page précédente"); previous.type = "button";
      previous.addEventListener("click", () => { this.#cursor = this.#history.pop() ?? null; void this.read(); }); pager.append(previous);
    }
    if (page.has_more && page.next_cursor) {
      const next = el("button", "button button-quiet", "Page suivante"); next.type = "button";
      next.addEventListener("click", () => { this.#history.push(this.#cursor); this.#cursor = page.next_cursor; void this.read(); }); pager.append(next);
    }
    return [page.items.length ? el("div", "ov-scroll", table) : el("p", "", "Aucune appartenance dans cette page de l’organisation."),
      el("p", "muted", "Création de compte et d’identifiant : opérateur hors ligne. Les droits réservés au contrôle ne sont jamais accordés depuis cette vue."), pager];
  }
}

export interface UsageHost extends AdminHost {
  quota: (signal: AbortSignal) => Promise<HumanQuota>;
  usage: (cursor: string | null, signal: AbortSignal) => Promise<UsagePage>;
}
type QuotaUsageRead = { quota: HumanQuota; usage: UsagePage | null };
export class UsageView extends AdminView<QuotaUsageRead> {
  #usageHost: UsageHost; #cursor: string | null = null;
  constructor(host: UsageHost) { super(host, "ov-usage"); this.#usageHost = host; }
  protected async fetch(signal: AbortSignal) {
    const [quota, usage] = await Promise.all([
      this.#usageHost.quota(signal),
      can(this.host.context, "usage_read") ? this.#usageHost.usage(this.#cursor, signal) : null,
    ]);
    return { quota, usage };
  }
  protected evaluatedAt({ quota, usage }: QuotaUsageRead) { return (usage ?? quota).evaluated_at; }
  protected receiptId({ quota, usage }: QuotaUsageRead) { return (usage ?? quota).read_receipt.request_id; }
  protected render({ quota, usage }: QuotaUsageRead) {
    const q = quota.quota;
    const reservations = el("section", "panel ov-panel", el("h2", "", "Quota du tenant"),
      el("div", "ov-stats",
        el("span", "ov-stat", el("strong", "ov-num", grouped(q.held_workers)), el("span", "ov-stat-label", `workers réservés sur ${grouped(q.max_active_workers)}`)),
        el("span", "ov-stat", el("strong", "ov-num", grouped(q.held_cpu_millis)), el("span", "ov-stat-label", `milliCPU réservés sur ${grouped(q.max_cpu_millis)}`)),
        el("span", "ov-stat", el("strong", "ov-num", gibibytes(q.held_memory_bytes)), el("span", "ov-stat-label", `mémoire réservée sur ${gibibytes(q.max_memory_bytes)}`))),
      el("p", "muted", `Réservations durables, pas une consommation mesurée. Durée maximale d’un worker : ${grouped(quota.max_timeout_seconds)} s.`));
    if (!usage) return [reservations, el("section", "panel ov-panel", el("h2", "", "Usage observé"),
      el("p", "muted", accessExplanation(this.host.context, "usage_read")))];
    const table = el("table", "ov-groups");
    table.append(el("caption", "", "Allocations terminales observées · méthode 1 · billable=false"),
      el("thead", "", el("tr", "", th("Worker"), th("Fin observée"), th("Durée allouée"), th("milliCPU·s"), th("Mémoire (Gio·s)"), th("Autorité · runtime"))));
    const body = el("tbody");
    for (const item of usage.items) {
      const tr = el("tr"); tr.dataset.attemptId = item.attempt_id;
      tr.append(el("td", "mono", item.worker_id), el("td", "", el("time", "mono", formatDate(item.finished_at, true))),
        el("td", "mono", `${perSecond(item.duration_ns)} s`), el("td", "mono", perSecond(item.cpu_millicore_nanoseconds)),
        el("td", "mono", gibibytes(perSecond(item.memory_byte_nanoseconds).replace(/\D/g, "") || "0")),
        el("td", "", el("span", "mono", item.source.authority_id), el("span", "muted", ` · ${item.source.runtime_kind}`)));
      body.append(tr);
    }
    table.append(body);
    const next = usage.has_more && usage.next_cursor ? (() => {
      const button = el("button", "button button-quiet", "Allocations suivantes"); button.type = "button";
      button.addEventListener("click", () => { this.#cursor = usage.next_cursor; void this.read(); }); return button;
    })() : "";
    return [reservations, el("section", "panel ov-panel", el("h2", "", "Usage observé"),
      usage.items.length ? el("div", "ov-scroll", table) : el("p", "", "Aucune allocation terminale dans cette page. Un worker actif n’a pas encore de durée."),
      el("p", "muted", "Ni prix, ni facture : temps d’allocation CPU/mémoire relevé à la fin observée."), next)];
  }
}

export interface ConfigurationHost extends AdminHost { read: (signal: AbortSignal) => Promise<HumanConfiguration> }
const axisLabel: Record<string, string> = { build: "Version", topology: "Topologie", identity: "Identité", storage: "Stockage", decision: "Décision",
  runtime: "Runtimes", egress: "Sorties réseau", providers: "Clients IA" };
const provenanceLabel = { configured: "Configuré", default: "Valeur par défaut", derived: "Établi par le contrôle", declared: "Déclaré par un broker", unknown: "Inconnu au gateway" } as const;
export class ConfigurationView extends AdminView<HumanConfiguration> {
  #configurationHost: ConfigurationHost;
  constructor(host: ConfigurationHost) { super(host, "ov-configuration"); this.#configurationHost = host; }
  protected fetch(signal: AbortSignal) { return this.#configurationHost.read(signal); }
  protected evaluatedAt(value: HumanConfiguration) { return value.evaluated_at; }
  protected receiptId(value: HumanConfiguration) { return value.read_receipt.request_id; }
  protected render(value: HumanConfiguration) {
    const table = el("table", "ov-ledger");
    table.append(el("caption", "", "Configuration effective"), el("thead", "", el("tr", "", th("Axe"), th("Réglage"), th("Provenance"))));
    const body = el("tbody");
    for (const entry of value.entries) {
      const head = el("th", "", axisLabel[entry.axis] ?? entry.axis); head.scope = "row";
      const tr = el("tr", "", head, el("td", "", el("span", "muted", `${entry.key} · `), el("strong", "mono", entry.value)),
        el("td", entry.provenance === "unknown" ? "muted" : "", provenanceLabel[entry.provenance]));
      tr.dataset.configurationKey = `${entry.axis}/${entry.key}`; body.append(tr);
    }
    table.append(body);
    return [el("div", "ov-scroll", table),
      el("p", "muted", "Types et modes seulement : aucune adresse, aucun chemin, aucun identifiant secret ni contenu de fichier n’est publié.")];
  }
}
