// The life of one prepare→launch journey. Pure: no DOM, no network, no storage. Recovery uses the supplied send
// callbacks and time. Rust authorizes every step and commits the mandatory audit before any effect; this module only
// keeps one stable operation identifier per intended step so a lost response is reconciled by replaying the same
// operation, never by a second Submit. Accepted launch, start issued and runtime-observed running stay separate facts.
import { HumanContractError } from "./human-model.ts";
import { HumanApiError, HumanSuperseded } from "./human-api.ts";
import type { SessionLaunchReceipt, SessionPrepareReceipt } from "./session-launch-model.ts";

/**
 * editing: the person names a target. preparing/launching: a request is in flight. prepared: a durable preparation is
 * held and may be launched. launched: the durable launch authority and grant are committed. The remaining phases are
 * distinct refusals; unknown keeps the operation identifier so a retry replays the same intent.
 */
export type LaunchPhase = "editing" | "preparing" | "prepared" | "launching" | "launched" | "refused" | "conflict" | "invalid" | "not_found" | "unknown";
export class InvalidLaunchRequest extends Error {}

/** Classify a step failure. A local contract break is invalid before any POST; an unreadable answer stays unknown. */
export function launchFailure(error: unknown): Exclude<LaunchPhase, "editing" | "preparing" | "prepared" | "launching" | "launched"> {
  if (error instanceof InvalidLaunchRequest || error instanceof HumanContractError) return "invalid";
  const status = error instanceof HumanApiError ? error.status : null;
  if (status === 409) return "conflict";
  if (status === 403 || status === 401) return "refused";
  if (status === 404) return "not_found";
  if (status === 400) return "invalid";
  return "unknown";
}
/** A context lost mid-flight is not a product refusal: the journey withdraws and the accesses are read again. */
export function launchContextLost(error: unknown): boolean {
  return error instanceof HumanApiError && error.status === 403 &&
    ["context_required", "context_invalid", "context_expired", "context_revoked"].includes(error.code);
}
export const LAUNCH_OUTCOME_UNKNOWN = "Le résultat du premier envoi reste inconnu : réessayer reprend la même opération.";
export const SESSION_PHASE_TEXT: Readonly<Record<Exclude<LaunchPhase, "editing" | "launched">, string>> = {
  preparing: "Préparation retenue par le contrôle…",
  prepared: "Préparation retenue. Elle ne lance rien et ne consomme aucune réservation.",
  launching: "Lancement de l’intention préparée exacte…",
  refused: "Refusé par le contrôle : vos droits sur ce périmètre sont à vérifier. Rien n’a été lancé.",
  conflict: "Conflit : l’intention diffère de celle déjà retenue, la préparation a été consommée ou la révision de la cible a changé. Relisez la cible avant de recommencer.",
  invalid: "Demande invalide : elle ne respecte pas le contrat. Rien n’a été lancé.",
  not_found: "Cible ou préparation introuvable dans ce périmètre. Vérifiez la référence publiée par le contrôle.",
  unknown: LAUNCH_OUTCOME_UNKNOWN,
};

interface Pending { key: string; id: string }
/**
 * One journey. `prepare` keeps an operation identifier per exact intent (target, revision, deadline) until a definitive
 * answer; `launch` keeps one per consumed preparation. A 503, a timeout or an unreadable answer leaves the outcome
 * unknown and keeps the identifier, so the control replays the same operation or returns its retained receipt.
 */
export class SessionLaunchJourney {
  phase: LaunchPhase = "editing";
  preparation: SessionPrepareReceipt | null = null;
  receipt: SessionLaunchReceipt | null = null;
  /** The last typed refusal, for its message and retry action; null after a success or a reset. */
  error: HumanApiError | null = null;
  #preparePending: Pending | null = null;
  #launchPending: Pending | null = null;
  #newId: () => string;
  constructor(newId: () => string = () => globalThis.crypto.randomUUID()) { this.#newId = newId; }

  get prepareOperationId(): string | null { return this.#preparePending?.id ?? null; }
  get launchOperationId(): string | null { return this.#launchPending?.id ?? null; }
  get sending(): boolean { return this.phase === "preparing" || this.phase === "launching"; }

  /** The identifier the next prepare of this exact intent carries; a new intent gets a new identifier. */
  operationIdFor(key: string): string {
    if (!this.#preparePending || this.#preparePending.key !== key) this.#preparePending = { key, id: this.#newId() };
    return this.#preparePending.id;
  }
  /** Reset to editing, withdrawing any held preparation and launch. The person names a target again. */
  reset() {
    if (this.sending) return;
    this.phase = "editing"; this.preparation = null; this.receipt = null; this.error = null;
    this.#preparePending = null; this.#launchPending = null;
  }
  /** Withdraw a held preparation (expired or revoked) without touching an in-flight step. */
  forgetPreparation() {
    if (this.sending) return;
    this.preparation = null; this.#launchPending = null;
    if (this.phase === "prepared") this.phase = "editing";
  }

  async prepare(key: string, send: (operationId: string) => Promise<SessionPrepareReceipt>): Promise<LaunchPhase> {
    if (this.sending) return this.phase;
    const id = this.operationIdFor(key);
    this.phase = "preparing"; this.error = null;
    try {
      const receipt = await send(id);
      this.#preparePending = null; this.preparation = receipt; this.receipt = null; this.#launchPending = null;
      this.phase = "prepared";
    } catch (error) {
      if (error instanceof HumanSuperseded) { this.phase = this.preparation ? "prepared" : "editing"; throw error; }
      this.phase = launchFailure(error);
      this.error = error instanceof HumanApiError ? error : null;
      if (this.phase !== "unknown") this.#preparePending = null;
    }
    return this.phase;
  }

  /** Launch the exact held preparation. The caller builds the body from `preparation`; the key binds the idempotency. */
  async launch(key: string, send: (operationId: string) => Promise<SessionLaunchReceipt>): Promise<LaunchPhase> {
    if (this.sending || !this.preparation) return this.phase;
    if (!this.#launchPending || this.#launchPending.key !== key) this.#launchPending = { key, id: this.#newId() };
    const id = this.#launchPending.id;
    this.phase = "launching"; this.error = null;
    try {
      const receipt = await send(id);
      this.#launchPending = null; this.receipt = receipt; this.phase = "launched";
    } catch (error) {
      if (error instanceof HumanSuperseded) { this.phase = "prepared"; throw error; }
      this.phase = launchFailure(error);
      this.error = error instanceof HumanApiError ? error : null;
      if (this.phase !== "unknown") this.#launchPending = null;
    }
    return this.phase;
  }
}