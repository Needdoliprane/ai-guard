// Vue 360° / Mon espace: one summary of the selected context, read from the Rust control.
// Every value comes from one transaction of the control; the view adds no count, never shows an unread or
// refused section as zero, and keeps the last summary dated and marked stale when a later read fails.
import { el } from "./dom.ts";
import "./overview.css";
import { formatDate } from "./model.ts";
import type { HumanContext } from "./human-model.ts";
import { HumanApiError, HumanSuperseded } from "./human-api.ts";
import {
  BROKER_STATES, DECISION_OUTCOMES, RESOURCE_KINDS, SUMMARY_WINDOWS, WORKER_STATES, countOf, gibibytes, grouped, perSecond,
  type AttentionKind, type Counts, type HumanSummary, type SummarySection, type SummaryWindow, type SummaryUnavailable,
} from "./summary-model.ts";

export const OVERVIEW_CADENCES = [120000, 30000, 0] as const;
export interface OverviewPreferences { window: SummaryWindow; cadence: number }
export const defaultOverviewPreferences = (): OverviewPreferences => ({ window: "24h", cadence: OVERVIEW_CADENCES[0] });

export interface OverviewHost {
  context: HumanContext;
  preferences: OverviewPreferences;
  read: (window: SummaryWindow, signal: AbortSignal) => Promise<HumanSummary>;
  /** Cumulative read budget stop: no automatic read, only an explicit retry after the operator acted. */
  budgetStopped: () => boolean;
  /** A refusal of this read: the host reads the identity again and withdraws what became forbidden. */
  onAccessDoubt: () => void;
  onReadStateChange: () => void;
  /** Opens the exact view of a subject when this context offers it; null when it does not. */
  openSubject: ((subject: { kind: "worker" | "broker"; id: string }) => void) | null;
  /** Integration hook: whether this context offers a view for this kind of subject. Absent: every kind opens. */
  canOpenSubject?: (kind: "worker" | "broker") => boolean;
}

type Phase = "loading" | "ready" | "error" | "refused";
const windowLabel: Record<SummaryWindow, string> = { "24h": "24 heures", "7d": "7 jours", "30d": "30 jours", all: "Tout l’historique" };
const cadenceLabel = (cadence: number) => cadence === 0 ? "Manuelle" : cadence === 30000 ? "Toutes les 30 s" : "Toutes les 2 min";
const workerStateLabel: Record<typeof WORKER_STATES[number], string> = { reserved: "Réservés", preparing: "Préparation", created: "Créés",
  start_issued: "Démarrage émis", running: "En cours selon le contrôle", stopping: "Arrêt en cours", terminal: "Terminés", in_doubt: "Incertains", cancelled: "Annulés" };
const brokerStateLabel: Record<typeof BROKER_STATES[number], string> = { unseen: "Heartbeat non reçu", current: "Bail courant", stale: "Bail périmé",
  expired: "Autorisation expirée", revoked: "Autorisation révoquée", clock_uncertain: "Horloge incertaine" };
const resourceLabel: Record<typeof RESOURCE_KINDS[number], string> = { session: "Sessions", agent: "Agents", sandbox: "Sandboxes", project: "Projets" };
const decisionLabel: Record<typeof DECISION_OUTCOMES[number], string> = { allow: "Autorisées", review: "À revoir", deny: "Refusées" };
const attentionLabel: Record<AttentionKind, string> = {
  worker_in_doubt: "Worker à l’issue incertaine", worker_allocation_inconsistent: "Allocation incohérente", worker_oom_killed: "Worker arrêté pour mémoire",
  broker_heartbeat_unseen: "Broker sans heartbeat reçu", broker_lease_stale: "Bail de broker périmé", broker_grant_expired: "Autorisation de broker expirée",
  broker_revoked: "Broker révoqué", broker_clock_uncertain: "Horloge de broker incertaine",
};
const unavailableText: Record<SummaryUnavailable, string> = {
  profile_unavailable: "Indisponible pour le profil de sources adopté : rien n’est lu ni déduit.",
  migration_required: "Indisponible : la base attend une migration explicite de l’opérateur. L’état reste inconnu.",
  capacity_exceeded: "Indisponible : le registre dépasse le budget de lecture de cette synthèse. Ouvrez la vue exacte.",
};
const coverageText = { complete_registry: "Registre complet du contrôle", authorized_selection: "Sélection autorisée de ce contexte",
  verified_journal: "Journal d’audit vérifié" } as const;

function badge(label: string, tone = "neutral") { return el("span", `badge badge-${tone}`, label); }
function statText(shown: string, label: string, title?: string) {
  const node = el("span", "ov-stat", el("strong", "ov-num", shown), el("span", "ov-stat-label", label));
  if (title) node.title = title;
  return node;
}
function stat(value: string, label: string) { return statText(grouped(value), label); }

export class OverviewView {
  readonly element = el("div", "ov");
  #host: OverviewHost;
  #phase: Phase = "loading"; #summary: HumanSummary | null = null; #loadedAt: number | null = null; #error = "";
  #controller: AbortController | null = null; #timer: ReturnType<typeof setTimeout> | undefined; #closed = false;
  #announce = el("p", "sr-only"); #said = "";
  #toolbar = el("div", "ov-toolbar"); #status = el("div", "ov-status"); #body = el("div", "ov-body");
  #refresh = el("button", "button button-secondary", "Actualiser la synthèse");

  constructor(host: OverviewHost) {
    this.#host = host;
    this.#announce.setAttribute("role", "status"); this.#announce.setAttribute("aria-live", "polite");
    this.#refresh.type = "button"; this.#refresh.dataset.focusKey = "ov-refresh";
    this.#refresh.addEventListener("click", () => { if (!this.reading) void this.#read(true); });
    this.#buildToolbar();
    this.element.append(this.#toolbar, this.#announce, this.#status, this.#body);
    this.#render();
  }
  get reading() { return this.#controller !== null; }
  start() {
    document.addEventListener("visibilitychange", this.#visibility);
    void this.#read(false);
  }
  refresh() { return this.reading ? Promise.resolve() : this.#read(true); }
  /** Withdraws every value and stops reads; the host calls it when the context changes or ends. */
  close() {
    this.#closed = true; this.#controller?.abort(); this.#controller = null; clearTimeout(this.#timer);
    document.removeEventListener("visibilitychange", this.#visibility);
    this.#summary = null; this.element.replaceChildren();
  }
  #visibility = () => {
    if (this.#closed) return;
    if (document.visibilityState !== "visible") { clearTimeout(this.#timer); this.#controller?.abort(); return; }
    this.#schedule(true);
  };
  #schedule(now = false) {
    clearTimeout(this.#timer);
    const cadence = this.#host.preferences.cadence;
    if (this.#closed || this.#phase === "refused" || this.#host.budgetStopped() || document.visibilityState !== "visible") return;
    if (now) { void this.#read(false); return; }
    if (cadence > 0) this.#timer = setTimeout(() => void this.#read(false), cadence);
  }
  async #read(asked: boolean) {
    if (this.#closed || this.#controller || (!asked && this.#host.budgetStopped())) return;
    const controller = new AbortController(); this.#controller = controller;
    if (!this.#summary) this.#phase = "loading";
    this.#render(); this.#host.onReadStateChange();
    try {
      const summary = await this.#host.read(this.#host.preferences.window, controller.signal);
      if (this.#closed || controller.signal.aborted) return;
      this.#summary = summary; this.#loadedAt = Date.now(); this.#phase = "ready"; this.#error = "";
      this.#say(asked ? `Synthèse relue à ${formatDate(summary.evaluated_at, true)}.` : this.#said ? null : "Synthèse chargée.");
    } catch (error) {
      if (this.#closed || controller.signal.aborted || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) {
        // A refused read withdraws what this view showed before the identity is read again.
        this.#summary = null; this.#phase = "refused"; this.#error = error.message;
        this.#say("Synthèse refusée par le contrôle : vos accès sont relus.");
        this.#host.onAccessDoubt();
      } else {
        this.#phase = "error"; this.#error = error instanceof Error ? error.message : "Lecture impossible.";
        this.#say(this.#summary ? "Nouvelle lecture en échec : la dernière synthèse reste affichée, périmée." : "Synthèse indisponible.");
      }
    } finally {
      if (this.#controller === controller) this.#controller = null;
      if (!this.#closed) { this.#render(); this.#host.onReadStateChange(); this.#schedule(); }
    }
  }
  #say(sentence: string | null) {
    if (!sentence) return;
    // A sentence identical to the current one is said again by clearing then rewriting the region.
    if (sentence === this.#said) { this.#announce.textContent = ""; setTimeout(() => { this.#announce.textContent = sentence; }, 100); }
    else this.#announce.textContent = sentence;
    this.#said = sentence;
  }
  #buildToolbar() {
    const period = el("select", "", ...SUMMARY_WINDOWS.map((window) => {
      const option = el("option", "", windowLabel[window]); option.value = window; option.selected = window === this.#host.preferences.window; return option;
    }));
    period.id = "ov-window"; period.dataset.focusKey = "ov-window";
    period.addEventListener("change", () => { this.#host.preferences.window = period.value as SummaryWindow; void this.#read(true); });
    const cadence = el("select", "", ...OVERVIEW_CADENCES.map((value) => {
      const option = el("option", "", cadenceLabel(value)); option.value = String(value); option.selected = value === this.#host.preferences.cadence; return option;
    }));
    cadence.id = "ov-cadence"; cadence.dataset.focusKey = "ov-cadence";
    cadence.addEventListener("change", () => { this.#host.preferences.cadence = Number(cadence.value); this.#schedule(); });
    const field = (label: string, control: HTMLSelectElement) => { const node = el("label", "field", label, control); node.htmlFor = control.id; return node; };
    this.#toolbar.append(field("Période des décisions et de l’usage", period), field("Relecture automatique", cadence), this.#refresh);
  }
  #render() {
    if (this.#closed) return;
    this.#refresh.setAttribute("aria-disabled", String(this.reading));
    const summary = this.#summary;
    const stale = this.#phase === "error" && summary !== null;
    this.#status.replaceChildren(
      this.#phase === "loading" && !summary ? el("p", "muted", "Lecture de la synthèse…") : "",
      summary ? el("p", "ov-provenance",
        el("span", "", "Instantané transactionnel du contrôle · évalué le "), el("time", "mono", formatDate(summary.evaluated_at, true)),
        el("span", "", " · reçu de lecture "), el("span", "mono", summary.read_receipt.request_id.slice(0, 8)),
        el("span", "", ` · période ${windowLabel[summary.period.window]}`)) : "",
      stale ? el("div", "control-warning", el("p", "", "Données périmées · dernière synthèse conservée. "), el("p", "", this.#error)) : "",
      this.#phase === "error" && !summary ? el("div", "control-error", el("p", "", `Synthèse indisponible. ${this.#error}`)) : "",
      this.#phase === "refused" ? el("div", "control-error", el("p", "", "Synthèse refusée pour ce contexte. Vos accès sont relus ; aucune donnée antérieure n’est conservée.")) : "",
      this.#host.budgetStopped() ? el("p", "muted", "Relecture automatique suspendue : budget durable de lecture atteint.") : "",
    );
    this.element.dataset.state = stale ? "stale" : this.#phase;
    this.#body.replaceChildren(...(summary ? this.#sections(summary) : []));
  }
  #sections(summary: HumanSummary): HTMLElement[] {
    const administration = summary.mode === "administration";
    const table = el("table", "ov-ledger");
    table.append(el("caption", "", administration ? "Situation du tenant" : "Situation de mon espace"),
      el("thead", "", el("tr", "", ...["Domaine", "Situation", "Source"].map((label) => { const th = el("th", "", label); th.scope = "col"; return th; }))));
    const body = el("tbody");
    const row = <T>(key: string, label: string, section: SummarySection<T>, render: (data: T) => (HTMLElement | string)[], note?: (data: T) => string | null) => {
      const head = el("th", "", label); head.scope = "row";
      const tr = el("tr", "", head);
      tr.dataset.ledgerRow = key; tr.dataset.sourceState = section.status;
      if (section.status === "not_granted") tr.append(el("td", "muted", "Non accordé à ce contexte."), el("td", "muted", "Aucune lecture émise."));
      else if (section.status === "unavailable") tr.append(el("td", "", unavailableText[section.code]), el("td", "muted", "—"));
      else {
        const extra = note?.(section.data);
        tr.append(el("td", "ov-cell", el("div", "ov-stats", ...render(section.data)), extra ? el("p", "muted ov-note", extra) : ""),
          el("td", "muted", coverageText[section.coverage]));
      }
      body.append(tr);
    };
    const states = (counts: Counts, labels: Record<string, string>, order: readonly string[]) =>
      order.filter((key) => countOf(counts, key) !== null).map((key) => stat(countOf(counts, key)!, labels[key]!));
    row("workers", "Workers", summary.sections.workers, (data) => [
      ...(Object.keys(data.by_state).length ? states(data.by_state, workerStateLabel, WORKER_STATES) : [el("span", "muted", "Aucun worker enregistré à cette lecture.")]),
      stat(data.quota_held, "Quota retenu"),
    ], () => "État enregistré par le contrôle ; l’inventaire runtime reste inconnu.");
    row("brokers", "Brokers", summary.sections.brokers, (data) => states(data.by_state, brokerStateLabel, BROKER_STATES),
      () => "Un bail courant ne prouve pas qu’un worker fonctionne.");
    row("resources", "Ressources", summary.sections.resources, (data) => states(data.by_kind, resourceLabel, RESOURCE_KINDS));
    row("decisions", `Décisions · ${windowLabel[summary.period.window]}`, summary.sections.decisions,
      (data) => states(data.by_outcome, decisionLabel, DECISION_OUTCOMES),
      (data) => data.complete ? null : `Comptées depuis ${formatDate(data.oldest_counted_at, true)} seulement : budget de lecture du journal atteint.`);
    row("quota", "Quota du tenant", summary.sections.quota, (data) => [
      stat(data.held_workers, `workers réservés sur ${grouped(data.max_active_workers)}`),
      stat(data.held_cpu_millis, `milliCPU réservés sur ${grouped(data.max_cpu_millis)}`),
      statText(gibibytes(data.held_memory_bytes), `mémoire réservée sur ${gibibytes(data.max_memory_bytes)}`, `${grouped(data.held_memory_bytes)} octets réservés`),
    ], () => "Réservations durables, pas une consommation mesurée.");
    row("usage", `Usage observé · ${windowLabel[summary.period.window]}`, summary.sections.usage, (data) => [
      stat(data.allocations, "allocations terminales"),
      stat(perSecond(data.duration_ns), "secondes allouées"),
      stat(perSecond(data.cpu_millicore_nanoseconds), "milliCPU·s"),
    ], () => "Méthode 1 · billable=false : ni prix, ni facture.");
    table.append(body);
    const sections: HTMLElement[] = [el("div", "ov-scroll", table)];
    const workers = summary.sections.workers;
    if (workers.status === "ready" && workers.data.groups.length) sections.push(this.#groups(workers.data));
    const attention = summary.sections.attention;
    if (attention.status === "ready") sections.push(this.#attention(attention.data));
    return sections;
  }
  #groups(data: Extract<HumanSummary["sections"]["workers"], { status: "ready" }>["data"]) {
    const table = el("table", "ov-groups");
    table.append(el("caption", "", "Workers par broker, joints par identifiant exact d’autorité"),
      el("thead", "", el("tr", "", ...["Broker", "État du bail", "Dernière réception observée", "Workers", "Quota retenu"].map((label) => { const th = el("th", "", label); th.scope = "col"; return th; }))));
    const body = el("tbody");
    for (const group of data.groups) {
      const tr = el("tr");
      tr.dataset.brokerId = group.broker?.broker_id ?? "";
      const stale = group.broker && group.broker.state !== "current";
      tr.append(
        el("td", "", group.broker ? el("span", "mono", group.broker.broker_id) : el("span", "", group.association === "unregistered" ? "Autorité non enregistrée" : "Autorité inconnue"),
          el("span", "muted", ` · ${group.runtime_kind}`)),
        el("td", "", group.broker ? badge(brokerStateLabel[group.broker.state], stale ? "amber" : "teal") : badge("Sans broker enregistré", "amber")),
        el("td", "", group.broker?.last_received_at ? el("time", "mono", formatDate(group.broker.last_received_at, true)) : el("span", "muted", "Aucune réception de l’incarnation courante")),
        el("td", "", ...Object.entries(group.by_state).map(([state, value]) => el("span", "ov-inline", `${workerStateLabel[state as typeof WORKER_STATES[number]]} ${grouped(value)}`)),
          stale ? el("p", "muted ov-note", "Dernier état connu enregistré : le broker ne rapporte plus de bail courant.") : ""),
        el("td", "mono", grouped(group.quota_held)));
      body.append(tr);
    }
    table.append(body);
    return el("section", "panel ov-panel", el("h2", "", "Flotte par broker"), el("div", "ov-scroll", table),
      data.groups_complete ? "" : el("p", "muted", `Les ${data.groups.length} premiers groupes sont affichés ; ouvrez la flotte pour le registre complet.`));
  }
  #attention(data: Extract<HumanSummary["sections"]["attention"], { status: "ready" }>["data"]) {
    const kinds = Object.entries(data.by_kind);
    const list = el("ol", "ov-attention");
    for (const item of data.items) {
      const open = this.#host.canOpenSubject?.(item.subject_kind) === false ? null : this.#host.openSubject;
      const li = el("li", "", el("strong", "", attentionLabel[item.kind]), el("span", "mono ov-id", item.subject_id),
        item.at ? el("time", "muted mono", formatDate(item.at, true)) : el("span", "muted", "instant non enregistré"));
      li.dataset.incidentKind = item.kind; li.dataset.subjectKind = item.subject_kind; li.dataset.subjectId = item.subject_id;
      if (open) {
        const button = el("button", "button button-quiet", item.subject_kind === "worker" ? "Ouvrir le worker" : "Ouvrir le broker");
        button.type = "button"; button.dataset.focusKey = `ov-open-${item.subject_kind}-${item.subject_id}-${item.kind}`;
        button.addEventListener("click", () => open({ kind: item.subject_kind, id: item.subject_id }));
        li.append(button);
      }
      list.append(li);
    }
    return el("section", "panel ov-panel", el("h2", "", "Points d’attention"),
      el("p", "muted", "Dérivés des seuls états durables du contrôle ; ce n’est pas un suivi d’incidents."),
      kinds.length ? el("p", "ov-kinds", ...kinds.map(([kind, value]) => el("span", "ov-inline", `${attentionLabel[kind as AttentionKind]} : ${grouped(value)}`))) :
        el("p", "", "Aucun point d’attention dans les registres lus."),
      list, data.has_more ? el("p", "muted", `Les ${data.items.length} plus récents sont listés ; les comptes ci-dessus couvrent tout le registre.`) : "");
  }
}
