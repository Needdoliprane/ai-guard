// Resume intention of the named dashboard. Pure decisions: no DOM, no network, no storage.
//
// A context lasts at most 300 seconds and the control never renews it. When it expires, or when an OIDC
// session rotates its token, every row is removed and the identity is read again. If the control then offers
// exactly one context with the same attribution, the dashboard may propose to reopen the same view with the
// filters the person had typed. The person confirms: no data is read before that click.
import { grantKey, principalKey } from "./supervision-model.ts";
import type { HumanContext, HumanIdentity, Principal } from "./human-model.ts";

/** The views that keep only typed choices of the person (filters, period, cadence), never a server row. */
export type ResumableView = "supervision" | "journal" | "overview";
/** What removed the context: its own expiry, or the replacement of the dashboard session by a token renewal. */
export type ResumeCause = "context_expired" | "session_rotated";
/** Kept in memory only. It carries identifiers of the account and its grant, what the person typed or selected, and no server row. */
export interface ResumeIntent<P> { principal: string; grant: string; view: ResumableView; cause: ResumeCause; createdAt: number; payload: P }
export const RESUME_MAX_AGE_MS = 15 * 60 * 1000;
export const RESUME_MAX_RETRIES = 3;
/** Delay before the nth automatic identity read: 2 s, 4 s, then 8 s. Nothing is read automatically after that. */
export const resumeRetryDelay = (attempt: number) => 2000 * 2 ** Math.min(Math.max(attempt, 0), RESUME_MAX_RETRIES - 1);
/** A deferred identity read never starts in a hidden tab: the return to the foreground reads the accesses and decides again. */
export const deferredReadAllowed = (input: { visible: boolean }) => input.visible;
const resumableViews: readonly string[] = ["supervision", "journal", "overview"];
/** A revoked or unknown context, a refusal, a sign-out and a deliberate navigation never resume. */
const resumableCauses: readonly string[] = ["context_expired", "session_rotated"];

export function captureIntent<P>(input: { cause: string; principal: Principal | null; context: HumanContext | null; view: string; now: number; budgetStopped: boolean; payload: P }): ResumeIntent<P> | null {
  if (!resumableCauses.includes(input.cause) || !resumableViews.includes(input.view) || !input.principal || !input.context || input.budgetStopped) return null;
  return { principal: principalKey(input.principal), grant: grantKey(input.context), view: input.view as ResumableView, cause: input.cause as ResumeCause, createdAt: input.now, payload: input.payload };
}

export type ResumeDecision = { kind: "offer"; context: HumanContext; reopen: boolean } | { kind: "retry" } |
  { kind: "manual"; reason: "other_principal" | "too_old" | "budget" | "none" | "ambiguous" | "still_expired" | "hidden" };
/**
 * Decides what the freshly read identity allows. "offer" names the one context the control proposes with the
 * same tenant, organization, mode, membership revision and capabilities. Its `reopen` is true only for a
 * resume the person already confirmed, in a visible tab: the view then reopens without another click. In a
 * hidden tab nothing reopens: the offer waits for a click. "retry" asks for one more identity read when that
 * context still carries an elapsed expiry, which happens when the two clocks disagree slightly.
 */
export function resumeCandidate<P>(intent: ResumeIntent<P>, identity: HumanIdentity,
  input: { now: number; visible: boolean; budgetStopped: boolean; attempts: number; confirmed?: boolean }): ResumeDecision {
  if (principalKey(identity.principal) !== intent.principal) return { kind: "manual", reason: "other_principal" };
  if (input.now - intent.createdAt > RESUME_MAX_AGE_MS) return { kind: "manual", reason: "too_old" };
  if (input.budgetStopped) return { kind: "manual", reason: "budget" };
  const equivalent = identity.contexts.filter((context) => grantKey(context) === intent.grant);
  if (equivalent.length > 1) return { kind: "manual", reason: "ambiguous" };
  const context = equivalent[0];
  if (!context) return { kind: "manual", reason: "none" };
  if (Date.parse(context.expires_at) > input.now) return { kind: "offer", context, reopen: input.confirmed === true && input.visible };
  if (!input.visible) return { kind: "manual", reason: "hidden" };
  return input.attempts < RESUME_MAX_RETRIES ? { kind: "retry" } : { kind: "manual", reason: "still_expired" };
}
