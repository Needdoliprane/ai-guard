// "Nouvelle session": the prepare→launch journey of the selected personal context, driven by the owned human routes.
// DOM only: the wire, the bounds and the life of the request come from session-launch-model.ts and session-launch-flow.ts;
// Rust resolves the target, authorizes every step and commits the mandatory audit before any effect. Every value shown
// comes from a control receipt; an unknown broker-side fact stays unknown, and an accepted launch is never a running worker.
import { button, el } from "./dom.ts";
import { formatDate } from "./model.ts";
import { HumanSuperseded, type HumanApiClient } from "./human-api.ts";
import type { HumanContext } from "./human-model.ts";
import { SESSION_PREPARE_TTL_MS, TARGET_PROBLEM_TEXT, preparationExpired, sessionLaunchOffers, targetReferenceProblem,
  type SessionConversationBinding, type SessionLaunchReceipt, type SessionPrepareReceipt } from "./session-launch-model.ts";
import { LAUNCH_OUTCOME_UNKNOWN, SESSION_PHASE_TEXT, SessionLaunchJourney, launchContextLost } from "./session-launch-flow.ts";

export interface SessionLaunchHost {
  context: HumanContext;
  api: Pick<HumanApiClient, "sessionPrepare" | "sessionLaunch">;
  /** A refusal whose reason could not be read: the host reads the identity again and withdraws what became forbidden. */
  onAccessDoubt: () => void;
  onReadStateChange: () => void;
  now?: () => number;
}
const mono = (text: string) => el("span", "mono", text);
const shortHash = (hash: string) => `${hash.slice(0, 12)}…${hash.slice(-8)}`;
function row(label: string, value: HTMLElement | string) {
  return el("div", "sl-row", el("span", "sl-row-label", label), typeof value === "string" ? el("span", "sl-row-value", value) : value);
}
function badge(label: string, tone = "neutral") { return el("span", `badge badge-${tone}`, label); }

export class SessionLaunchView {
  readonly element = el("div", "sl");
  #host: SessionLaunchHost;
  #journey = new SessionLaunchJourney();
  #closed = false;
  #abort = new AbortController();
  #expiryTimer: ReturnType<typeof setTimeout> | undefined;
  /** The exact prepare intent last sent, kept so a retry replays the identical body (same operation, same deadline). */
  #intent: { target: string; revision: number; expiresAt: string; key: string } | null = null;
  #problem: "target" | "revision" | null = null;
  #announce = el("p", "sr-only"); #said = "";
  #targetInput = el("input", "text-input sl-input");
  #revisionInput = el("input", "text-input sl-input");
  #form = el("section", "panel sl-form");
  #status = el("section", "sl-status");
  #result = el("section", "sl-result");

  constructor(host: SessionLaunchHost) {
    this.#host = host;
    this.#announce.setAttribute("role", "status"); this.#announce.setAttribute("aria-live", "polite");
    this.#targetInput.type = "text"; this.#targetInput.autocomplete = "off"; this.#targetInput.spellcheck = false;
    this.#targetInput.placeholder = "00000000-0000-0000-0000-000000000000"; this.#targetInput.dataset.focusKey = "sl-target";
    this.#targetInput.setAttribute("aria-label", "Identifiant opaque de la cible de lancement");
    this.#revisionInput.type = "text"; this.#revisionInput.inputMode = "numeric"; this.#revisionInput.autocomplete = "off";
    this.#revisionInput.placeholder = "1"; this.#revisionInput.dataset.focusKey = "sl-revision";
    this.#revisionInput.setAttribute("aria-label", "Révision attendue de la cible");
    this.#targetInput.addEventListener("input", () => { this.#problem = null; this.#paintForm(); });
    this.#revisionInput.addEventListener("input", () => { this.#problem = null; this.#paintForm(); });
    this.element.append(this.#form, this.#announce, this.#status, this.#result);
    this.#render();
  }
  get reading() { return this.#journey.sending; }
  start() { this.#render(); }
  /** The journey holds no periodic read; an explicit refresh re-renders its current state and expiry. */
  refresh() { this.#render(); return Promise.resolve(); }
  close() {
    this.#closed = true; this.#abort.abort(); clearTimeout(this.#expiryTimer);
    this.#intent = null; this.#journey.reset(); this.element.replaceChildren();
  }
  #now() { return this.#host.now?.() ?? Date.now(); }
  #say(text: string | null) { if (text && this.#said !== text) { this.#said = text; this.#announce.textContent = text; } }

  #render() {
    if (this.#closed) return;
    this.#paintForm(); this.#paintStatus(); this.#paintResult();
    this.#host.onReadStateChange();
  }
  #paintForm() {
    const offers = sessionLaunchOffers(this.#host.context);
    const sending = this.#journey.sending;
    const prepared = this.#journey.phase === "prepared" || this.#journey.phase === "launched";
    const launchKey = this.#journey.phase === "unknown" && this.#intent ? "sl-prepare-retry" : "sl-prepare";
    const prepareLabel = this.#journey.phase === "unknown" && this.#intent ? "Réessayer la même préparation" : "Préparer le lancement";
    const prepare = button(prepareLabel, "button button-primary", () => { void this.#prepare(); });
    prepare.dataset.focusKey = launchKey; prepare.disabled = sending || !offers.prepare || prepared;
    const reset = button("Recommencer", "button button-secondary", () => this.#reset());
    reset.dataset.focusKey = "sl-reset"; reset.disabled = sending || (this.#journey.phase === "editing" && !this.#intent);
    const fields: (HTMLElement | null)[] = [
      el("h2", "", "Nouvelle session IA"),
      el("p", "muted", "Le contrôle résout la cible, le fournisseur, le mode d’authentification et les ressources. Le navigateur ne fournit qu’une référence opaque publiée par le contrôle : ni chemin hôte, ni argv, ni image, ni credential."),
      el("div", "sl-field", el("label", "field-label", "Cible de lancement (identifiant opaque)"), this.#targetInput,
        el("p", "field-help", "UUID publié par le contrôle pour le projet/source autorisé. Une cible inconnue ou inactive est refusée par le contrôle.")),
      el("div", "sl-field", el("label", "field-label", "Révision attendue de la cible"), this.#revisionInput,
        el("p", "field-help", "Révision publiée par le contrôle. Si elle a changé, le contrôle signale un conflit et la cible est relue.")),
      this.#problem ? el("p", "control-error", TARGET_PROBLEM_TEXT[this.#problem]) : null,
      !offers.prepare ? el("p", "control-error", "Ce contexte n’offre pas la préparation de lancement : aucune requête n’est émise. Faites vérifier vos droits.") : null,
      el("div", "sl-actions", prepare, reset)];
    this.#form.replaceChildren(...fields.filter((node): node is HTMLElement => node !== null));
  }
  #paintStatus() {
    const phase = this.#journey.phase;
    if (phase === "editing") { this.#status.replaceChildren(); return; }
    if (phase === "preparing" || phase === "launching" || phase === "prepared") {
      this.#status.replaceChildren(el("p", "sl-phase", SESSION_PHASE_TEXT[phase]));
      this.#say(SESSION_PHASE_TEXT[phase]); return;
    }
    if (phase === "launched") { this.#status.replaceChildren(); return; }
    // A distinct refusal: the category is always named, then the control's own message and its retry action.
    const error = this.#journey.error;
    const retry = error?.retryAction === "contact_operator" ? " Action requise : contactez l’opérateur." :
      error?.retryAction === "review_permissions" ? " Faites vérifier vos permissions sur ce périmètre." :
      error?.retryAction === "reread" ? " Relisez la cible avant de recommencer." : "";
    const category = SESSION_PHASE_TEXT[phase];
    const children: (HTMLElement | string)[] = [el("p", "control-error", category)];
    if (error) children.push(el("p", "muted", `${error.message}${retry}`));
    if (phase === "unknown" && category !== LAUNCH_OUTCOME_UNKNOWN) children.push(el("p", "muted", LAUNCH_OUTCOME_UNKNOWN));
    if (phase === "conflict" || phase === "not_found" || phase === "invalid" || phase === "refused") {
      const again = button("Recommencer", "button button-secondary", () => this.#reset());
      again.dataset.focusKey = "sl-refusal-reset"; children.push(el("div", "sl-actions", again));
    }
    this.#status.replaceChildren(...children);
    this.#say(category);
  }
  #paintResult() {
    const phase = this.#journey.phase;
    if (phase === "launched" && this.#journey.receipt) { this.#result.replaceChildren(this.#launchCard(this.#journey.receipt)); return; }
    const preparation = this.#journey.preparation;
    if (preparation && (phase === "prepared" || phase === "launching" || phase === "unknown")) {
      this.#result.replaceChildren(this.#prepareCard(preparation)); return;
    }
    this.#result.replaceChildren();
  }
  #prepareCard(receipt: SessionPrepareReceipt) {
    const offers = sessionLaunchOffers(this.#host.context);
    const expired = preparationExpired(receipt, this.#now());
    const sending = this.#journey.phase === "launching";
    this.#scheduleExpiry(receipt);
    const launch = button("Lancer la session", "button button-primary", () => { void this.#launch(); });
    launch.dataset.focusKey = "sl-launch"; launch.disabled = sending || expired || !offers.launch || this.#journey.phase === "unknown";
    const card = el("div", "panel sl-card",
      el("div", "sl-card-head", el("h3", "", "Préparation retenue"), receipt.replayed ? badge("Rejeu exact", "blue") : badge("Durable", "teal"),
        expired ? badge("Expirée", "amber") : null),
      el("p", "muted", "Cette préparation ne lance rien et ne consomme aucune réservation. Le lancement commit l’intention exacte après autorisation, quota, politique et audit durables."),
      el("div", "sl-rows",
        row("Cible", mono(receipt.launch_target_id)),
        row("Révision retenue", String(receipt.target_revision)),
        row("Identifiant de préparation", mono(receipt.preparation_id)),
        row("Empreinte de l’intention", el("span", "mono", shortHash(receipt.input_hash))),
        row("Expire le", formatDate(receipt.expires_at, true)),
        row("Audit durable", `séquence ${receipt.audit_seq} · ${shortHash(receipt.audit_hash)}`)),
      !offers.launch ? el("p", "control-error", "Ce contexte n’offre pas le lancement de session : la préparation est retenue mais aucun lancement n’est émis. Faites vérifier vos droits.") : null,
      expired ? el("p", "control-error", "La préparation a expiré : elle ne peut plus être lancée. Recommencez une préparation.") : null,
      el("div", "sl-actions", launch));
    return card;
  }
  #launchCard(receipt: SessionLaunchReceipt) {
    const binding = receipt.conversation;
    return el("div", "panel sl-card",
      el("div", "sl-card-head", el("h3", "", "Lancement accepté"), receipt.replayed ? badge("Rejeu exact", "blue") : badge("Durable", "teal")),
      el("p", "control-warning sl-warning", "Lancement accepté ≠ worker en cours d’exécution. Ce reçu prouve l’autorité de lancement durable et le grant nominatif ; l’admission supervisée du broker et le démarrage observé par le runtime restent des faits distincts, non observés ici."),
      el("div", "sl-rows",
        row("Session", mono(receipt.session_id)),
        row("Identifiant de lancement", mono(receipt.launch_id)),
        row("Cible", mono(receipt.launch_target_id)),
        row("Révision", String(receipt.target_revision)),
        row("Grant nominatif", `révision ${receipt.grant_revision} · audit ${receipt.grant_audit_seq}`),
        row("Audit de lancement", `séquence ${receipt.audit_seq} · ${shortHash(receipt.audit_hash)}`),
        row("Préparation consommée", mono(receipt.preparation_operation_id))),
      this.#conversationCard(binding));
  }
  #conversationCard(binding: SessionConversationBinding) {
    const unknown = (value: string | null) => value === null ? el("span", "muted", "Inconnu · en attente de l’admission du broker") : mono(value);
    return el("div", "sl-conversation",
      el("h4", "", "Canal de conversation retenu"),
      el("p", "muted", "Lié au tenant/source, à la session et à la révision de la cible. Les faits broker/runtime restent inconnus jusqu’à l’admission supervisée ; ils ne sont jamais inventés."),
      el("div", "sl-rows",
        row("Statut", badge("prepared", "neutral")),
        row("Tenant", binding.tenant_id),
        row("Source", mono(binding.source_id)),
        row("Époque d’entrée", String(binding.input_epoch)),
        row("Broker", unknown(binding.broker_id)),
        row("Incarnation", unknown(binding.broker_incarnation)),
        row("Worker", unknown(binding.worker_id)),
        row("Tentative", unknown(binding.attempt_id)),
        row("Instance runtime", unknown(binding.runtime_instance)),
        row("Fournisseur", unknown(binding.provider)),
        row("Protocole", unknown(binding.protocol)),
        row("Expire le", formatDate(binding.expires_at, true))));
  }
  #scheduleExpiry(receipt: SessionPrepareReceipt) {
    clearTimeout(this.#expiryTimer);
    const remaining = Date.parse(receipt.expires_at) - this.#now();
    if (remaining <= 0) return;
    this.#expiryTimer = setTimeout(() => { if (!this.#closed) this.#render(); }, Math.min(remaining, 2147483647) + 25);
  }
  #reset() {
    if (this.#journey.sending) return;
    this.#journey.reset(); this.#intent = null; this.#problem = null;
    this.#abort = new AbortController(); clearTimeout(this.#expiryTimer);
    this.#render();
  }
  async #prepare() {
    const offers = sessionLaunchOffers(this.#host.context);
    if (this.#closed || this.#journey.sending || !offers.prepare) return;
    // A retry after an unknown outcome replays the identical intent; a fresh prepare builds a new deadline.
    if (this.#journey.phase !== "unknown" || !this.#intent) {
      const target = this.#targetInput.value.trim();
      const revisionText = this.#revisionInput.value.trim();
      this.#problem = targetReferenceProblem(target, revisionText);
      if (this.#problem) { this.#paintForm(); this.#say(TARGET_PROBLEM_TEXT[this.#problem]); return; }
      const revision = Number(revisionText);
      const expiresAt = new Date(this.#now() + SESSION_PREPARE_TTL_MS).toISOString();
      this.#intent = { target, revision, expiresAt, key: JSON.stringify([target, revision, expiresAt]) };
    }
    const intent = this.#intent;
    this.#render();
    try {
      await this.#journey.prepare(intent.key, (operationId) => this.#host.api.sessionPrepare(
        { version: 1, operation_id: operationId, launch_target_id: intent.target, expected_target_revision: intent.revision, expires_at: intent.expiresAt },
        this.#abort.signal));
    } catch (error) {
      if (error instanceof HumanSuperseded || this.#closed) return;
      if (launchContextLost(error)) { this.#host.onAccessDoubt(); return; }
    }
    if (!this.#closed) this.#render();
  }
  async #launch() {
    const preparation = this.#journey.preparation;
    const offers = sessionLaunchOffers(this.#host.context);
    if (this.#closed || this.#journey.sending || !preparation || !offers.launch || preparationExpired(preparation, this.#now())) return;
    const key = JSON.stringify([preparation.operation_id, preparation.input_hash]);
    this.#render();
    try {
      await this.#journey.launch(key, (operationId) => this.#host.api.sessionLaunch(
        { version: 1, operation_id: operationId, launch_target_id: preparation.launch_target_id,
          expected_target_revision: preparation.target_revision, preparation_operation_id: preparation.operation_id,
          preparation_input_hash: preparation.input_hash }, this.#abort.signal));
    } catch (error) {
      if (error instanceof HumanSuperseded || this.#closed) return;
      if (launchContextLost(error)) { this.#host.onAccessDoubt(); return; }
    }
    if (!this.#closed) this.#render();
  }
}