import { button, el } from "./dom.ts";
import { HumanApiError, HumanSuperseded, type HumanApiClient } from "./human-api.ts";
import { HumanContractError } from "./human-model.ts";
import { formatDate } from "./model.ts";
import { messageDestination, type SessionConversationViewV1 } from "./session-message-model.ts";
import { SessionMessageView, type SessionMessageHost } from "./session-message-view.ts";

interface SessionConversationHost extends SessionMessageHost {
  api: SessionMessageHost["api"] & Pick<HumanApiClient, "sessionConversation">;
}
const statusLabel = { prepared: "Préparé · en attente d’admission", active: "Actif", closed: "Fermé" };

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
  #read: AbortController | null = null;
  #error = "";
  #closed = false;

  constructor(host: SessionConversationHost) {
    this.#host = host;
    this.#message = new SessionMessageView({ ...host, onReadStateChange: () => {
      if (this.#selectButton) this.#selectButton.disabled = this.#message.reading;
      this.#host.onReadStateChange();
    } });
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
      this.#sessionId = sessionId;
      this.#withdraw();
    }
    return this.refresh();
  }

  async refresh() {
    const sessionId = this.#sessionId;
    if (!sessionId || this.#closed) return;
    this.#read?.abort();
    const read = new AbortController();
    this.#read = read; this.#facts = null; this.#error = "";
    this.#render();
    try {
      const facts = await this.#host.api.sessionConversation(sessionId, read.signal);
      if (this.#read !== read) return;
      this.#facts = facts;
      if (this.#selected && (!messageDestination(facts, Date.now()) || !sameChannel(this.#selected, facts))) this.#withdraw();
    } catch (error) {
      if (this.#read !== read || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && (error.status === 403 || error.status === 404)) this.#withdraw();
      this.#error = error instanceof HumanApiError || error instanceof HumanContractError ?
        error.message : "Lecture du canal impossible. Réessayez.";
      if (error instanceof HumanApiError && error.status === 403) this.#host.onAccessDoubt();
    } finally {
      if (this.#read === read) { this.#read = null; this.#render(); }
    }
  }

  suspend() {
    this.#read?.abort(); this.#read = null;
    this.#facts = null;
    this.#render();
  }

  close() {
    this.#closed = true;
    this.#read?.abort(); this.#read = null;
    this.#sessionId = null; this.#facts = null; this.#selected = null; this.#error = "";
    this.#selectButton = null;
    this.#message.close();
    this.element.replaceChildren();
  }

  #withdraw() {
    this.#selected = null;
    this.#message.setDestination(null);
  }

  #select(facts: SessionConversationViewV1) {
    const destination = messageDestination(facts, Date.now());
    if (this.#closed || this.#read || this.#message.reading || facts !== this.#facts || !destination) return;
    this.#selected = facts;
    this.#message.setDestination(destination);
    this.#render();
    this.#message.element.querySelector<HTMLElement>('[data-focus-key="sm-content"]')?.focus();
  }

  #render() {
    const facts = this.#facts;
    const destination = facts ? messageDestination(facts, Date.now()) : null;
    const canSend = this.#selected !== null && destination !== null;
    this.#message.setChannelCurrent(canSend);
    if (canSend) {
      if (this.#message.element.parentElement !== this.element) this.element.append(this.#message.element);
    } else this.#message.element.remove();
    this.#refreshButton.hidden = !this.#sessionId;
    this.#refreshButton.setAttribute("aria-disabled", String(this.#read !== null));

    this.#selectButton = null;
    if (facts) {
      if (destination && !this.#selected) {
        const select = button("Utiliser ce canal", "button button-secondary", () => this.#select(facts));
        select.dataset.focusKey = "sc-select";
        select.disabled = this.#message.reading;
        this.#selectButton = select;
      }
      this.#body.replaceChildren(el("p", "", statusLabel[facts.status]),
        el("p", "mono control-id", facts.session_id),
        el("p", "", "Époque d’entrée ", facts.input_epoch, " · Échéance ", formatDate(facts.expires_at, true)),
        facts.provider && facts.protocol ? el("p", "", facts.provider, " · ", facts.protocol) : "",
        facts.status === "active" && !destination ? el("p", "control-warning", "L’échéance du canal est dépassée.") : "",
        this.#selectButton ?? "");
    } else {
      this.#body.replaceChildren(this.#error ? el("p", "control-error", this.#error) :
        el("p", "muted", this.#read ? "Lecture du canal…" : this.#sessionId ? "Canal à relire." :
          "Choisissez une session dont vous êtes propriétaire dans Ressources."));
    }
    this.#host.onReadStateChange();
  }
}
