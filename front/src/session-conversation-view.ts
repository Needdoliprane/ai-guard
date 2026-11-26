import { button, el } from "./dom.ts";
import { HumanApiError, HumanSuperseded, type HumanApiClient } from "./human-api.ts";
import { HumanContractError } from "./human-model.ts";
import { formatDate } from "./model.ts";
import { messageDestination, type SessionConversationViewV1, type SessionMessageDestination } from "./session-message-model.ts";
import type { MessageDestinationRefusal } from "./session-message-flow.ts";
import { SessionMessageView, type SessionMessageHost } from "./session-message-view.ts";

interface SessionConversationHost extends SessionMessageHost {
  api: SessionMessageHost["api"] & Pick<HumanApiClient, "sessionConversation">;
}
const statusLabel = {
  prepared: "Préparé · en attente d’admission", active: "Actif", closed: "Fermé",
  binding_changed: "Canal modifié · relisez le canal", binding_expired: "Expiré",
};

function sameChannel(previous: SessionConversationViewV1, current: SessionConversationViewV1) {
  return previous.session_id === current.session_id && previous.input_epoch === current.input_epoch &&
    previous.source_id === current.source_id && previous.tenant_id === current.tenant_id &&
    previous.provider === current.provider && previous.protocol === current.protocol &&
    previous.expires_at === current.expires_at;
}

export class SessionConversationView {
  readonly element = el("div", "sl");
  #host: SessionConversationHost;
  #message: SessionMessageView;
  #panel = el("section", "panel sl-form");
  #body = el("div", "sl-rows");
  #refreshButton: HTMLButtonElement;
  #selectButton: HTMLButtonElement | null = null;
  #sessionId: string | null = null;
  #facts: SessionConversationViewV1 | null = null;
  #selected: SessionConversationViewV1 | null = null;
  #refused: { destination: SessionMessageDestination; code: MessageDestinationRefusal } | null = null;
  #read: AbortController | null = null;
  #expiryTimer: ReturnType<typeof setTimeout> | undefined;
  #error = "";
  #closed = false;
  #suspended = false;

  constructor(host: SessionConversationHost) {
    this.#host = host;
    this.#message = new SessionMessageView({ ...host,
      onReadStateChange: () => {
        if (this.#message) this.#syncMessage();
        this.#host.onReadStateChange();
      },
      onDestinationRefused: (destination, code) => this.#destinationRefused(destination, code),
    });
    this.#refreshButton = button("Relire le canal", "button button-secondary", () => {
      if (!this.#read) void this.refresh();
    });
    this.#refreshButton.dataset.focusKey = "sc-refresh";
    this.#body.setAttribute("aria-live", "polite");
    this.#panel.append(el("h2", "", "Canal de conversation"), this.#body, el("div", "sl-actions", this.#refreshButton));
    this.element.append(this.#panel);
    this.#render();
  }

  get reading() { return this.#read !== null || this.#message.reading; }

  selectSession(sessionId: string) {
    if (this.#sessionId !== sessionId) {
      this.#sessionId = sessionId; this.#refused = null;
      this.#withdraw();
    }
    return this.refresh();
  }

  async refresh() {
    const sessionId = this.#sessionId;
    if (!sessionId || this.#closed) return;
    this.#suspended = false;
    this.#read?.abort();
    clearTimeout(this.#expiryTimer);
    const read = new AbortController();
    this.#read = read; this.#facts = null; this.#error = "";
    this.#render();
    try {
      const facts = await this.#host.api.sessionConversation(sessionId, read.signal);
      if (this.#read !== read) return;
      this.#facts = facts;
      if (this.#status(facts) === "active") this.#scheduleExpiry(facts);
      if (this.#selected && (this.#status(facts) !== "active" || !sameChannel(this.#selected, facts))) this.#withdraw();
    } catch (error) {
      if (this.#read !== read || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && (error.status === 403 || error.status === 404)) this.#withdraw(true);
      this.#error = error instanceof HumanApiError || error instanceof HumanContractError ?
        error.message : "Lecture du canal impossible. Réessayez.";
      if (error instanceof HumanApiError && error.status === 403) this.#host.onAccessDoubt();
    } finally {
      if (this.#read === read) { this.#read = null; this.#render(); }
    }
  }

  suspend() {
    this.#suspended = true;
    this.#read?.abort(); this.#read = null;
    clearTimeout(this.#expiryTimer);
    this.#facts = null;
    this.#render();
  }

  close() {
    this.#closed = true;
    this.#read?.abort(); this.#read = null;
    clearTimeout(this.#expiryTimer);
    this.#sessionId = null; this.#facts = null; this.#selected = null; this.#refused = null; this.#error = "";
    this.#selectButton = null;
    this.#message.close();
    this.element.replaceChildren();
  }

  #destinationRefused(destination: SessionMessageDestination, code: MessageDestinationRefusal) {
    if (this.#closed || this.#sessionId !== destination.sessionId) return;
    const facts = this.#facts;
    // A refetch can already have confirmed a replacement before the old POST answered.
    if (facts && (facts.source_id !== destination.sourceId ||
      code === "binding_changed" && facts.input_epoch > destination.inputEpoch)) return;
    this.#refused = { destination, code };
    this.#withdraw();
    if (!this.#suspended && !this.#read) void this.refresh();
    else this.#render();
  }

  #status(facts: SessionConversationViewV1): keyof typeof statusLabel {
    if (facts.status !== "active") return facts.status;
    const refused = this.#refused;
    if (refused && refused.destination.sourceId === facts.source_id) {
      if (refused.code === "session_closed") return "closed";
      if (refused.code === "binding_expired") return "binding_expired";
      if (facts.input_epoch <= refused.destination.inputEpoch) return "binding_changed";
    }
    return Date.parse(facts.expires_at) <= Date.now() ? "binding_expired" : "active";
  }

  #scheduleExpiry(facts: SessionConversationViewV1) {
    const remaining = Date.parse(facts.expires_at) - Date.now();
    if (facts.status !== "active" || remaining <= 0) return;
    this.#expiryTimer = setTimeout(() => {
      if (this.#facts !== facts) return;
      // A deadline beyond the browser's timer range needs a fresh read at that range's end.
      if (Date.parse(facts.expires_at) > Date.now()) this.#facts = null;
      this.#render();
    }, Math.min(remaining, 2147483647));
  }

  #withdraw(accessLost = false) {
    this.#selected = null;
    if (accessLost || !this.#message.reading && !this.#message.outcomeUnknown) this.#message.setDestination(null);
  }

  #select(facts: SessionConversationViewV1) {
    const destination = messageDestination(facts, Date.now());
    if (this.#closed || this.#read || this.#cannotSelect() || facts !== this.#facts || this.#status(facts) !== "active" || !destination) return;
    this.#selected = facts;
    this.#message.setDestination(destination);
    this.#render();
    this.#message.element.querySelector<HTMLElement>('[data-focus-key="sm-content"]')?.focus();
  }

  #cannotSelect() { return this.#message.reading || this.#message.outcomeUnknown; }

  #syncMessage() {
    const facts = this.#facts;
    const canSend = this.#selected !== null && facts !== null && this.#status(facts) === "active";
    this.#message.setChannelCurrent(canSend);
    const canShowReceipt = !this.#suspended && (this.#message.outcomeUnknown || this.#message.hasReceipt);
    if (canSend || canShowReceipt) {
      if (this.#message.element.parentElement !== this.element) this.element.append(this.#message.element);
    } else this.#message.element.remove();
    if (this.#selectButton) this.#selectButton.disabled = this.#cannotSelect();
  }

  #render() {
    const facts = this.#facts;
    const status = facts ? this.#status(facts) : null;
    this.#syncMessage();
    this.#refreshButton.hidden = !this.#sessionId;
    this.#refreshButton.setAttribute("aria-disabled", String(this.#read !== null));

    this.#selectButton = null;
    if (facts && status) {
      if (status === "active" && !this.#selected) {
        const select = button("Utiliser ce canal", "button button-secondary", () => this.#select(facts));
        select.dataset.focusKey = "sc-select";
        select.disabled = this.#cannotSelect();
        this.#selectButton = select;
      }
      this.#body.replaceChildren(el("p", "", statusLabel[status]),
        el("p", "mono control-id", facts.session_id),
        el("p", "", "Époque d’entrée ", facts.input_epoch, " · Échéance ", formatDate(facts.expires_at, true)),
        facts.provider && facts.protocol ? el("p", "", facts.provider, " · ", facts.protocol) : "",
        this.#selectButton ?? "");
    } else {
      const refusal = this.#refused?.code;
      const status = refusal === "session_closed" ? statusLabel.closed : refusal ? statusLabel[refusal] : "";
      this.#body.replaceChildren(status ? el("p", "", status) : "",
        this.#error ? el("p", "control-error", this.#error) :
          el("p", "muted", this.#read ? "Lecture du canal…" : this.#sessionId ? "Canal à relire." :
            "Choisissez une session dont vous êtes propriétaire dans Ressources."));
    }
    this.#host.onReadStateChange();
  }
}
