// The life of one SendSessionMessage journey. Pure: no DOM, no network, no storage. Recovery uses the supplied send
// callbacks and time. Rust authorizes every step and commits the mandatory audit before any effect; this module only
// keeps one stable operation identifier per exact instruction so a lost response is reconciled by replaying the same
// operation, never by a second Submit. queued, transmitted, acknowledged and unknown stay distinct truthful facts;
// none means the instruction was executed. After a possible emission with no reconcilable ack, uncertainty is retained.
import { HumanContractError } from "./human-model.ts";
import { HumanApiError, HumanSuperseded } from "./human-api.ts";
import type { SessionMessageReceipt } from "./session-message-model.ts";

/**
 * editing: the person writes an instruction. sending: a request is in flight. sent: a durable receipt is held and its
 * delivery status is shown truthfully. The remaining phases are distinct refusals; unknown keeps the operation
 * identifier so a retry replays the same intent. capacity is a definitive bounded-queue refusal (nothing enqueued).
 */
export type MessagePhase = "editing" | "sending" | "sent" | "refused" | "conflict" | "invalid" | "not_found" | "capacity" | "unknown";
export class InvalidMessageRequest extends Error {}

/** Classify a step failure. A local contract break is invalid before any POST; an unreadable answer stays unknown. */
export function messageFailure(error: unknown): Exclude<MessagePhase, "editing" | "sending" | "sent"> {
  if (error instanceof InvalidMessageRequest || error instanceof HumanContractError) return "invalid";
  const status = error instanceof HumanApiError ? error.status : null;
  if (status === 409) return "conflict";
  if (status === 403 || status === 401) return "refused";
  if (status === 404) return "not_found";
  if (status === 400) return "invalid";
  if (status === 429 && error instanceof HumanApiError && error.code === "capacity_exhausted") return "capacity";
  return "unknown";
}
/** A context lost mid-flight is not a product refusal: the journey withdraws and the accesses are read again. */
export function messageContextLost(error: unknown): boolean {
  return error instanceof HumanApiError && error.status === 403 &&
    ["context_required", "context_invalid", "context_expired", "context_revoked"].includes(error.code);
}
export const MESSAGE_OUTCOME_UNKNOWN = "Le résultat du premier envoi reste inconnu : réessayer reprend la même opération, jamais un second envoi.";
export const MESSAGE_PHASE_TEXT: Readonly<Record<Exclude<MessagePhase, "editing" | "sent">, string>> = {
  sending: "Envoi de la consigne au contrôle…",
  refused: "Refusé par le contrôle : vos droits sur cette session sont à vérifier. Rien n’a été enregistré.",
  conflict: "Conflit : la consigne diffère de celle déjà retenue sous cette opération, ou le canal a changé. Relisez la session avant de recommencer.",
  invalid: "Demande invalide : elle ne respecte pas le contrat. Rien n’a été enregistré.",
  not_found: "Session introuvable dans ce périmètre. Vérifiez la destination publiée par le contrôle.",
  capacity: "File bornée pleine : trop de consignes non résolues pour cette session ou ce compte. Rien n’a été enregistré ; réessayez plus tard.",
  unknown: MESSAGE_OUTCOME_UNKNOWN,
};

interface Pending { key: string; id: string }
/**
 * One journey. `send` keeps an operation identifier per exact intent (session, epoch, content) until a definitive
 * answer. A 503, a timeout or an unreadable answer leaves the outcome unknown and keeps the identifier, so the control
 * replays the same operation or returns its retained receipt. `refresh` re-reads the held receipt's delivery status
 * without ever resending; a lost refresh keeps the last known status.
 */
export class SessionMessageJourney {
  phase: MessagePhase = "editing";
  receipt: SessionMessageReceipt | null = null;
  /** The last typed refusal, for its message and retry action; null after a success or a reset. */
  error: HumanApiError | null = null;
  #pending: Pending | null = null;
  #newId: () => string;
  constructor(newId: () => string = () => globalThis.crypto.randomUUID()) { this.#newId = newId; }

  get operationId(): string | null { return this.#pending?.id ?? null; }
  get sending(): boolean { return this.phase === "sending"; }

  /** The identifier the next send of this exact intent carries; a new intent gets a new identifier. */
  operationIdFor(key: string): string {
    if (!this.#pending || this.#pending.key !== key) this.#pending = { key, id: this.#newId() };
    return this.#pending.id;
  }
  /** Reset to editing, withdrawing any held receipt. The person writes an instruction again. */
  reset() {
    if (this.sending) return;
    this.phase = "editing"; this.receipt = null; this.error = null; this.#pending = null;
  }

  async send(key: string, send: (operationId: string) => Promise<SessionMessageReceipt>): Promise<MessagePhase> {
    if (this.sending) return this.phase;
    const id = this.operationIdFor(key);
    this.phase = "sending"; this.error = null;
    try {
      const receipt = await send(id);
      this.#pending = null; this.receipt = receipt; this.phase = "sent";
    } catch (error) {
      if (error instanceof HumanSuperseded) { this.phase = this.receipt ? "sent" : "editing"; throw error; }
      this.phase = messageFailure(error);
      this.error = error instanceof HumanApiError ? error : null;
      if (this.phase !== "unknown") this.#pending = null;
    }
    return this.phase;
  }

  /**
   * Re-read the held receipt's delivery status (metadata only). Never resends. A lost or refused refresh keeps the
   * last known receipt and records the error for display; the caller withdraws on a lost context.
   */
  async refresh(read: (operationId: string) => Promise<SessionMessageReceipt>): Promise<MessagePhase> {
    if (this.sending || !this.receipt) return this.phase;
    const id = this.receipt.operation_id;
    try {
      this.receipt = await read(id); this.error = null; this.phase = "sent";
    } catch (error) {
      if (error instanceof HumanSuperseded) throw error;
      this.error = error instanceof HumanApiError ? error : null;
    }
    return this.phase;
  }
}