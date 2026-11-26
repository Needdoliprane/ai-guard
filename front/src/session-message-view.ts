// "Envoyer une consigne": the SendSessionMessage journey of the selected personal context, driven by the owned human
// routes. DOM only: the wire, the bounds and the life of the request come from session-message-model.ts and
// session-message-flow.ts. Rust revalidates the distinct SendSessionMessage permission, the session's owner ACL, the
// retained binding and the masking policy before the mandatory audit. Every value shown comes from a control receipt;
// the destination comes from a current conversation read. queued, transmitted, acknowledged and unknown are
// shown distinctly, and none is presented as the instruction being executed.
import { button, el } from "./dom.ts";
import { formatDate } from "./model.ts";
import { HumanSuperseded, type HumanApiClient } from "./human-api.ts";
import { HumanContractError, type HumanContext } from "./human-model.ts";
import { CONTENT_PROBLEM_TEXT, MESSAGE_NOT_EXECUTED, MESSAGE_STATUS_BADGE, MESSAGE_STATUS_TEXT, MESSAGE_UNKNOWN_WARNING,
  destinationExpired, messageContentProblem, sessionMessageOffers, type ContentProblem, type SessionMessageDestination,
  type SessionMessageReceipt } from "./session-message-model.ts";
import { MESSAGE_PHASE_TEXT, SessionMessageJourney, messageContextLost, messageDestinationRefusal,
  type MessageDestinationRefusal } from "./session-message-flow.ts";

export interface SessionMessageHost {
  context: HumanContext;
  api: Pick<HumanApiClient, "sessionMessageSend" | "sessionMessageReceipt">;
  /** A refusal whose reason could not be read: the host reads the identity again and withdraws what became forbidden. */
  onAccessDoubt: () => void;
  onReadStateChange: () => void;
  onDestinationRefused?: (destination: SessionMessageDestination, code: MessageDestinationRefusal) => void;
  now?: () => number;
}
const mono = (text: string) => el("span", "mono", text);
const shortHash = (hash: string) => `${hash.slice(0, 12)}…${hash.slice(-8)}`;
function row(label: string, value: HTMLElement | string) {
  return el("div", "sl-row", el("span", "sl-row-label", label), typeof value === "string" ? el("span", "sl-row-value", value) : value);
}
function badge(label: string, tone = "neutral") { return el("span", `badge badge-${tone}`, label); }

export class SessionMessageView {
  readonly element = el("div", "sl sm");
  #host: SessionMessageHost;
  #journey = new SessionMessageJourney();
  #destination: SessionMessageDestination | null;
  /** A destination change that arrived while a send was in flight; applied once it settles. */
  #pendingDestination: SessionMessageDestination | null | undefined = undefined;
  #closed = false;
  #channelCurrent = true;
  #readingReceipt = false;
  #abort = new AbortController();
  /** The exact send intent last sent, kept so a retry replays the identical body (same operation, same content). */
  #intent: { content: string; key: string } | null = null;
  #problem: ContentProblem | null = null;
  #announce = el("p", "sr-only"); #said = "";
  #contentInput = el("textarea", "text-input sm-textarea");
  #form = el("section", "panel sl-form");
  #status = el("section", "sl-status");
  #result = el("section", "sl-result");

  constructor(host: SessionMessageHost, destination: SessionMessageDestination | null = null) {
    this.#host = host; this.#destination = destination;
    this.#announce.setAttribute("role", "status"); this.#announce.setAttribute("aria-live", "polite");
    this.#contentInput.rows = 4; this.#contentInput.autocomplete = "off"; this.#contentInput.spellcheck = false;
    this.#contentInput.placeholder = "Une courte consigne pour la session…"; this.#contentInput.dataset.focusKey = "sm-content";
    this.#contentInput.setAttribute("aria-label", "Courte consigne à envoyer à la session");
    this.#contentInput.addEventListener("input", () => { this.#problem = null; this.#paintForm(); });
    this.element.append(this.#form, this.#announce, this.#status, this.#result);
    this.#render();
  }
  get reading() { return this.#journey.sending || this.#readingReceipt; }
  get outcomeUnknown() { return this.#journey.phase === "unknown" && this.#journey.operationId !== null; }
  get hasReceipt() { return this.#journey.receipt !== null; }
  start() { this.#render(); }
  setChannelCurrent(current: boolean) {
    if (this.#channelCurrent === current) return;
    this.#channelCurrent = current;
    if (current) this.#announce.before(this.#form);
    else this.#form.remove();
    this.#paintForm();
    this.#paintStatus();
  }
  /**
   * Set the destination from a current conversation read. A changed session or epoch withdraws the held receipt (the old
   * binding can no longer be targeted); while a send is in flight the change is deferred until it settles.
   */
  setDestination(destination: SessionMessageDestination | null) {
    if (this.#journey.sending) { this.#pendingDestination = destination; return; }
    this.#applyDestination(destination);
  }
  #applyDestination(destination: SessionMessageDestination | null) {
    const changed = this.#destination?.sessionId !== destination?.sessionId || this.#destination?.inputEpoch !== destination?.inputEpoch ||
      this.#destination?.sourceId !== destination?.sourceId;
    this.#destination = destination;
    if (changed) {
      this.#resetJourney();
      this.#contentInput.value = "";
    }
    this.#render();
  }
  close() {
    this.#closed = true; this.#abort.abort();
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
    const offers = sessionMessageOffers(this.#host.context);
    const destination = this.#destination;
    const expired = destination ? destinationExpired(destination, this.#now()) : false;
    const phase = this.#journey.phase;
    const retry = phase === "unknown" && this.#intent;
    const sendLabel = retry ? "Réessayer le même envoi" : "Envoyer la consigne";
    const send = button(sendLabel, "button button-primary", () => { void this.#send(); });
    send.dataset.focusKey = retry ? "sm-send-retry" : "sm-send";
    send.disabled = !this.#channelCurrent || this.reading || !offers.send || !destination || expired || phase === "sent";
    const reset = button("Nouvelle consigne", "button button-secondary", () => this.#reset());
    reset.dataset.focusKey = "sm-reset"; reset.disabled = this.reading || (phase === "editing" && !this.#intent);
    const fields: (HTMLElement | null)[] = [
      el("h2", "", "Envoyer une consigne"),
      el("p", "muted", "Une courte consigne vers une session existante. Le contrôle revérifie le droit distinct SendSessionMessage, l’ACL de la session, la liaison retenue (STOP, échéance, époque) et le masquage avant l’audit durable. Le corps est une donnée métier du tenant ; il n’est jamais dupliqué dans l’audit, les URL, argv ou les journaux."),
      this.#destinationCard(),
      el("div", "sl-field", el("label", "field-label", "Consigne (texte court)"), this.#contentInput,
        el("p", "field-help", "8192 octets maximum. Seuls le saut de ligne et la tabulation sont admis comme caractères de contrôle. Le masquage des secrets est appliqué par le contrôle avant enregistrement.")),
      this.#problem ? el("p", "control-error", CONTENT_PROBLEM_TEXT[this.#problem]) : null,
      !offers.send ? el("p", "control-error", "Ce contexte n’offre pas l’envoi de consigne : aucune requête n’est émise. La lecture, l’administration et l’approbation de lancement ne l’impliquent pas. Faites vérifier vos droits.") : null,
      !destination && offers.send ? el("p", "muted", "Aucune session retenue. Choisissez un canal actif pour envoyer une consigne.") : null,
      expired ? el("p", "control-error", "L’échéance d’origine de la session est dépassée : elle garde priorité sur toute nouvelle consigne. Relisez la session.") : null,
      el("div", "sl-actions", send, reset)];
    this.#form.replaceChildren(...fields.filter((node): node is HTMLElement => node !== null));
  }
  #destinationCard() {
    const destination = this.#destination;
    if (!destination) return el("div", "sl-conversation", el("h4", "", "Destination"), el("p", "muted", "Aucun canal sélectionné."));
    const unknown = (value: string | null) => value === null ? el("span", "muted", "Inconnu · en attente de l’admission du broker") : mono(value);
    return el("div", "sl-conversation",
      el("h4", "", "Destination retenue"),
      el("p", "muted", "L’envoi cible l’époque d’entrée affichée. Le contrôle vérifie cette époque avant d’enregistrer la consigne."),
      el("div", "sl-rows",
        row("Session", mono(destination.sessionId)),
        row("Époque d’entrée", String(destination.inputEpoch)),
        row("Tenant", destination.tenantId),
        row("Source", mono(destination.sourceId)),
        row("Fournisseur", unknown(destination.provider)),
        row("Protocole", unknown(destination.protocol)),
        row("Expire le", formatDate(destination.expiresAt, true))));
  }
  #paintStatus() {
    const phase = this.#journey.phase;
    if (phase === "editing" || phase === "sent") { this.#status.replaceChildren(); return; }
    if (phase === "sending") { this.#status.replaceChildren(el("p", "sl-phase", MESSAGE_PHASE_TEXT[phase])); this.#say(MESSAGE_PHASE_TEXT[phase]); return; }
    const error = this.#journey.error;
    const retry = error?.retryAction === "contact_operator" ? " Action requise : contactez l’opérateur." :
      error?.retryAction === "review_permissions" ? " Faites vérifier vos permissions sur ce périmètre." :
        error?.retryAction === "reread" ? " Relisez la session avant de recommencer." : "";
    const readOnlyRecovery = this.outcomeUnknown && !this.#channelCurrent;
    const category = readOnlyRecovery ? "Le résultat de l’envoi épinglé reste inconnu." : MESSAGE_PHASE_TEXT[phase];
    const children: (HTMLElement | string)[] = [el("p", "control-error", category)];
    if (error) children.push(el("p", "muted", `${error.message}${retry}`));
    if (readOnlyRecovery) {
      const lookup = button("Relire le reçu", "button button-secondary", () => { void this.#refreshStatus(); });
      lookup.dataset.focusKey = "sm-unknown-refresh"; lookup.disabled = this.reading;
      const abandon = button("Abandonner la reprise", "button button-secondary", () => {
        if (!this.reading) { this.#contentInput.value = ""; this.#reset(); }
      });
      abandon.dataset.focusKey = "sm-unknown-abandon"; abandon.disabled = this.reading;
      children.push(el("div", "sl-actions", lookup, abandon),
        el("p", "muted", "L’abandon est local. L’opération peut déjà avoir été enregistrée."));
    }
    if (phase === "conflict" || phase === "not_found" || phase === "invalid" || phase === "refused" || phase === "capacity") {
      const again = button("Nouvelle consigne", "button button-secondary", () => this.#reset());
      again.dataset.focusKey = "sm-refusal-reset"; children.push(el("div", "sl-actions", again));
    }
    this.#status.replaceChildren(...children);
    this.#say(category);
  }
  #paintResult() {
    const receipt = this.#journey.receipt;
    if (this.#journey.phase === "sent" && receipt) { this.#result.replaceChildren(this.#receiptCard(receipt)); return; }
    this.#result.replaceChildren();
  }
  #receiptCard(receipt: SessionMessageReceipt) {
    const status = receipt.status;
    const refresh = button("Relire le statut", "button button-secondary", () => { void this.#refreshStatus(); });
    refresh.dataset.focusKey = "sm-status-refresh"; refresh.disabled = this.reading;
    const card = el("div", "panel sl-card",
      el("div", "sl-card-head", el("h3", "", "Consigne enregistrée"), badge(status, MESSAGE_STATUS_BADGE[status]),
        receipt.replayed ? badge("Rejeu exact", "blue") : badge("Durable", "teal")),
      el("p", "muted", MESSAGE_STATUS_TEXT[status]),
      el("p", "control-warning sl-warning", MESSAGE_NOT_EXECUTED),
      status === "unknown" ? el("p", "control-warning sl-warning", MESSAGE_UNKNOWN_WARNING) : null,
      el("div", "sl-rows",
        row("Session", mono(receipt.session_id)),
        row("Identifiant d’opération", mono(receipt.operation_id)),
        row("Identifiant de message", mono(receipt.message_id)),
        row("Époque d’entrée", String(receipt.input_epoch)),
        row("Empreinte de contenu", el("span", "mono", shortHash(receipt.content_hash))),
        row("Empreinte d’intention", el("span", "mono", shortHash(receipt.input_hash))),
        row("Liaison épinglée", el("span", "mono", shortHash(receipt.binding_fingerprint))),
        row("Audit durable", `séquence ${receipt.audit_seq} · ${shortHash(receipt.audit_hash)}`),
        receipt.ack ? row("RPC émis", String(receipt.ack.rpc_id)) : null,
        receipt.ack ? row("Turn corrélé", mono(receipt.ack.turn_id)) : null),
      this.#journey.error ? el("p", "muted", `Dernière relecture du statut refusée : ${this.#journey.error.message} Le statut affiché reste le dernier état connu.`) : null,
      el("div", "sl-actions", refresh));
    return card;
  }
  #reset() {
    if (this.reading) return;
    this.#resetJourney();
    this.#render();
  }
  #resetJourney() {
    this.#abort.abort(); this.#abort = new AbortController(); this.#readingReceipt = false;
    this.#journey = new SessionMessageJourney(); this.#intent = null; this.#problem = null;
  }
  async #send() {
    const offers = sessionMessageOffers(this.#host.context);
    const destination = this.#destination;
    if (this.#closed || !this.#channelCurrent || this.reading || !offers.send || !destination || destinationExpired(destination, this.#now())) return;
    // A retry after an unknown outcome replays the identical intent; a fresh send reads the current instruction.
    if (this.#journey.phase !== "unknown" || !this.#intent) {
      const content = this.#contentInput.value;
      this.#problem = messageContentProblem(content);
      if (this.#problem) { this.#paintForm(); this.#say(CONTENT_PROBLEM_TEXT[this.#problem]); return; }
      this.#intent = { content, key: JSON.stringify([destination.sessionId, destination.inputEpoch, content]) };
    }
    const intent = this.#intent;
    const outcome = this.#journey.send(intent.key, (operationId) => this.#host.api.sessionMessageSend(
      { version: 1, operation_id: operationId, session_id: destination.sessionId,
        expected_input_epoch: destination.inputEpoch, content: intent.content }, this.#abort.signal));
    this.#render();
    try {
      await outcome;
    } catch (error) {
      if (error instanceof HumanSuperseded || this.#closed) return;
    }
    if (this.#closed) return;
    // A context lost mid-flight is recorded by the journey, not a product refusal: withdraw and read the accesses again.
    if (this.#journey.error && messageContextLost(this.#journey.error)) { this.#host.onAccessDoubt(); return; }
    const refusal = messageDestinationRefusal(this.#journey.error);
    this.#applyPendingDestination();
    if (refusal) this.#host.onDestinationRefused?.(destination, refusal);
    if (!this.#closed) this.#render();
  }
  async #refreshStatus() {
    const destination = this.#destination;
    if (this.#closed || this.reading || !destination || (!this.hasReceipt && !this.outcomeUnknown)) return;
    const journey = this.#journey;
    this.#readingReceipt = true;
    this.#render();
    try {
      await journey.refresh(async (operationId) => {
        const receipt = await this.#host.api.sessionMessageReceipt(operationId, this.#abort.signal);
        if (receipt.session_id !== destination.sessionId || receipt.input_epoch !== destination.inputEpoch)
          throw new HumanContractError();
        return receipt;
      });
      if (!this.#closed && this.#journey === journey && journey.error && messageContextLost(journey.error)) this.#host.onAccessDoubt();
    } catch (error) {
      if (!(error instanceof HumanSuperseded)) throw error;
    } finally {
      if (!this.#closed && this.#journey === journey) { this.#readingReceipt = false; this.#render(); }
    }
  }
  #applyPendingDestination() {
    if (this.#pendingDestination !== undefined) {
      const destination = this.#pendingDestination; this.#pendingDestination = undefined;
      this.#applyDestination(destination);
    }
  }
}