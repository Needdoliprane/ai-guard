// Delegated membership and share changes: what the editors may offer, what a change adds and removes, and the life
// of one change request. Recovery uses the supplied client and time, with no DOM or storage.
//
// Rust authorizes every change and applies the delegation rules before any effect. This module only narrows what an
// editor offers, by those same published rules, so that a person is not invited to ask for a change the control
// refuses. The rules are stated over capabilities (what an actor may hand over); they never open a view or a read:
// the editors themselves are offered only from the operation catalogue (`delegationOffers`).
import { CAPABILITIES, HumanContractError, type Capability, type HumanContext, type HumanIdentity, type Principal } from "./human-model.ts";
import { HumanApiError, HumanSuperseded, type HumanApiClient } from "./human-api.ts";
import { access, can } from "./access-model.ts";
import { grantKey, principalKey } from "./supervision-model.ts";
import { adminUrls, grantChangeBody, membershipChangeBody, GRANT_SHARES_MAX, SHARE_PERMISSIONS, type GrantChange, type GrantChangeReceipt,
  type GrantShare, type HumanGrant, type Membership, type MembershipChange, type MembershipChangeReceipt, type SharePermission } from "./admin-model.ts";

const DELEGATED_OPERATION = { membership: "memberships_manage", grant: "grants_manage" } as const;
type DelegatedChangeKind = keyof typeof DELEGATED_OPERATION;

/** Editors offered by the catalogue of the context: granted and served, nothing else. */
export function delegationOffers(context: HumanContext | null) {
  return { memberships: can(context, DELEGATED_OPERATION.membership), grants: can(context, DELEGATED_OPERATION.grant) };
}

/** Reserved to the control: a person may keep or remove them, never add or reactivate them. */
export const CONTROL_ONLY_CAPABILITIES: readonly Capability[] = ["read_broker_registry", "read_worker_registry", "read_memberships",
  "read_configuration", "launch_session", "send_session_message"];
export const CAPABILITY_LABELS: Readonly<Record<Capability, string>> = {
  read_metadata: "Métadonnées", read_content: "Contenus", export: "Export", read_tenant_metadata: "Métadonnées de l’organisation",
  manage_memberships: "Gestion des appartenances", manage_grants: "Gestion des partages", read_broker_registry: "Registre des brokers",
  read_worker_registry: "Registre des workers", approve_launch: "Approbation de lancement", read_memberships: "Lecture des appartenances",
  read_configuration: "Configuration", launch_session: "Lancement de session", send_session_message: "Consigne à une session",
  approve_egress: "Approbation des sorties réseau",
};
export const PERMISSION_LABELS: Readonly<Record<SharePermission, string>> = { metadata: "Métadonnées", content: "Contenu", export: "Export" };

const holds = (context: HumanContext, capability: Capability) => context.capabilities.includes(capability);
const has = (list: readonly Capability[], capability: Capability) => list.includes(capability);

export interface CapabilityChoice {
  capability: Capability; label: string;
  /** Carried by the membership now: it can always be kept or removed. */
  current: boolean;
  /** Can be added by this context: never a control-only capability, never one the context does not hold. */
  addable: boolean;
  reason: "control_only" | "not_held" | null;
}
export function capabilityChoices(context: HumanContext, membership: Membership): CapabilityChoice[] {
  return CAPABILITIES.map((capability) => {
    const current = has(membership.capabilities, capability);
    const controlOnly = has(CONTROL_ONLY_CAPABILITIES, capability);
    const addable = !controlOnly && holds(context, capability);
    return { capability, label: CAPABILITY_LABELS[capability], current, addable,
      reason: controlOnly ? "control_only" : addable ? null : "not_held" };
  });
}
export const CHOICE_REASON: Readonly<Record<"control_only" | "not_held", string>> = {
  control_only: "Réservée au contrôle : elle peut être conservée ou retirée, jamais ajoutée ni réactivée ici.",
  not_held: "Ce contexte ne détient pas cette capacité : il ne peut pas l’ajouter.",
};

export interface MembershipDraft { capabilities: Capability[]; active: boolean }
/** The draft an editor starts from: the membership as read. */
export const membershipDraft = (membership: Membership): MembershipDraft => ({ capabilities: canonical(membership.capabilities), active: membership.active });
/** Capabilities in the closed order of the control, each once. */
export function canonical(capabilities: readonly Capability[]): Capability[] { return CAPABILITIES.filter((capability) => has(capabilities, capability)); }
export const draftHolds = (draft: MembershipDraft, capability: Capability) => has(draft.capabilities, capability);
export function toggleCapability(draft: MembershipDraft, capability: Capability, on: boolean): MembershipDraft {
  return { ...draft, capabilities: canonical(on ? [...draft.capabilities, capability] : draft.capabilities.filter((item) => item !== capability)) };
}
/** The identity of an intended change: the same key keeps the same operation identifier. */
export const membershipChangeKey = (membership: Membership, draft: MembershipDraft) =>
  JSON.stringify([membership.membership_id, membership.revision, canonical(draft.capabilities), draft.active]);
export interface MembershipDiff {
  added: Capability[]; removed: Capability[];
  activation: "kept" | "deactivated" | "reactivated";
  /** After the change the account keeps no access in this organization. */
  removesAllAccess: boolean;
}
export function membershipDiff(membership: Membership, draft: MembershipDraft): MembershipDiff {
  const desired = CAPABILITIES.filter((capability) => has(draft.capabilities, capability));
  return {
    added: desired.filter((capability) => !has(membership.capabilities, capability)),
    removed: CAPABILITIES.filter((capability) => has(membership.capabilities, capability) && !has(desired, capability)),
    activation: membership.active === draft.active ? "kept" : draft.active ? "reactivated" : "deactivated",
    removesAllAccess: membership.active && (!draft.active || desired.length === 0),
  };
}
export type MembershipProblem = "own" | "no_change" | "control_only_added" | "not_held_added" | "reactivation_control_only" | "reactivation_not_held";
export const MEMBERSHIP_PROBLEM_TEXT: Readonly<Record<MembershipProblem, string>> = {
  own: "Votre propre appartenance ne se modifie pas ici : son changement est réservé au contrôle.",
  no_change: "Aucun changement par rapport à la révision lue.",
  control_only_added: "Une capacité réservée au contrôle ne peut pas être ajoutée.",
  not_held_added: "Une capacité que ce contexte ne détient pas ne peut pas être ajoutée.",
  reactivation_control_only: "Réactiver cette appartenance réactiverait une capacité réservée au contrôle : retirez-la d’abord.",
  reactivation_not_held: "Réactiver cette appartenance réactiverait une capacité que ce contexte ne détient pas : retirez-la d’abord.",
};
/** Every reason the control would refuse this change by its delegation rules. An empty list does not authorize it. */
export function membershipProblems(context: HumanContext, membership: Membership, draft: MembershipDraft): MembershipProblem[] {
  if (membership.own) return ["own"];
  const diff = membershipDiff(membership, draft);
  const problems = new Set<MembershipProblem>();
  if (!diff.added.length && !diff.removed.length && diff.activation === "kept") problems.add("no_change");
  for (const capability of diff.added) {
    if (has(CONTROL_ONLY_CAPABILITIES, capability)) problems.add("control_only_added");
    else if (!holds(context, capability)) problems.add("not_held_added");
  }
  if (diff.activation === "reactivated") for (const capability of draft.capabilities) {
    if (has(CONTROL_ONLY_CAPABILITIES, capability)) problems.add("reactivation_control_only");
    else if (!holds(context, capability)) problems.add("reactivation_not_held");
  }
  return [...problems];
}

/** A permission may be added to a share only when the context holds its capability. */
export function permissionAddable(context: HumanContext, permission: SharePermission): boolean {
  return permission === "metadata" ? holds(context, "read_metadata") || holds(context, "read_tenant_metadata") :
    permission === "content" ? holds(context, "read_content") : holds(context, "export");
}
export interface ShareDraft { principal: Principal; display_name: string; permissions: SharePermission[] }
/** Members who may receive a new share: never the actor, never the owner, never a principal already in the list. */
export function shareCandidates(members: Membership[], grant: HumanGrant, self: Principal, draft: ShareDraft[]): Membership[] {
  const excluded = new Set([principalKey(self), principalKey(grant.owner), ...draft.map((share) => principalKey(share.principal))]);
  const seen = new Set<string>();
  return members.filter((member) => {
    const key = principalKey(member.principal);
    if (excluded.has(key) || seen.has(key) || !member.active || !member.account_active) return false;
    seen.add(key); return true;
  });
}
export interface ShareDiff {
  added: ShareDraft[]; removed: GrantShare[];
  changed: { principal: Principal; display_name: string; added: SharePermission[]; removed: SharePermission[] }[];
}
export function shareDiff(grant: HumanGrant, draft: ShareDraft[]): ShareDiff {
  const before = new Map(grant.shares.map((share) => [principalKey(share.principal), share]));
  const after = new Map(draft.map((share) => [principalKey(share.principal), share]));
  const diff: ShareDiff = { added: [], removed: [], changed: [] };
  for (const [key, share] of after) {
    const previous = before.get(key);
    if (!previous) { diff.added.push(share); continue; }
    const added = SHARE_PERMISSIONS.filter((permission) => share.permissions.includes(permission) && !previous.permissions.includes(permission));
    const removed = SHARE_PERMISSIONS.filter((permission) => previous.permissions.includes(permission) && !share.permissions.includes(permission));
    if (added.length || removed.length) diff.changed.push({ principal: share.principal, display_name: share.display_name, added, removed });
  }
  for (const [key, share] of before) if (!after.has(key)) diff.removed.push(share);
  return diff;
}
export type ShareProblem = "no_change" | "self" | "owner" | "duplicate" | "empty" | "not_addable" | "too_many";
export const SHARE_PROBLEM_TEXT: Readonly<Record<ShareProblem, string>> = {
  no_change: "Aucun changement par rapport à la révision lue.",
  self: "Vous ne pouvez pas vous partager une ressource.",
  owner: "Le propriétaire ne reçoit pas de partage et ne change pas ici.",
  duplicate: "Un membre ne figure qu’une fois dans la liste.",
  empty: "Un partage sans permission n’existe pas : retirez ce membre de la liste.",
  not_addable: "Une permission que ce contexte ne détient pas ne peut pas être ajoutée.",
  too_many: `Au plus ${GRANT_SHARES_MAX} partages par ressource.`,
};
export function shareProblems(context: HumanContext, grant: HumanGrant, draft: ShareDraft[], self: Principal): ShareProblem[] {
  const problems = new Set<ShareProblem>();
  const keys = draft.map((share) => principalKey(share.principal));
  if (keys.includes(principalKey(self))) problems.add("self");
  if (keys.includes(principalKey(grant.owner))) problems.add("owner");
  if (new Set(keys).size !== keys.length) problems.add("duplicate");
  if (draft.some((share) => !share.permissions.length)) problems.add("empty");
  if (draft.length > GRANT_SHARES_MAX) problems.add("too_many");
  const diff = shareDiff(grant, draft);
  if ([...diff.added.flatMap((share) => share.permissions), ...diff.changed.flatMap((share) => share.added)].some((permission) => !permissionAddable(context, permission)))
    problems.add("not_addable");
  if (!diff.added.length && !diff.removed.length && !diff.changed.length) problems.add("no_change");
  return [...problems];
}

/**
 * The life of one change request. One operation identifier per intended change, kept across retries until the
 * control gives a definitive answer (200, 400, 403, 404, 409). A 503, a network failure, a timeout or an answer that
 * cannot be read leave the outcome unknown: a retry sends the same identifier, so the control replays the same
 * change or returns its receipt.
 */
export type ChangePhase = "ready" | "sending" | "applied" | "unknown" | "conflict" | "refused" | "invalid" | "not_found";
function changeFailure(error: unknown): Exclude<ChangePhase, "ready" | "sending" | "applied"> {
  if (error instanceof InvalidChangeRequest) return "invalid";
  const status = error instanceof HumanApiError ? error.status : null;
  if (status === 409) return "conflict";
  if (status === 403 || status === 401) return "refused";
  if (status === 404) return "not_found";
  if (status === 400) return "invalid";
  return "unknown";
}
export const DELEGATED_OUTCOME_UNKNOWN = "Le résultat du premier envoi reste inconnu.";
const REPLAY_REFUSAL = {
  refused: "Le contrôle a refusé la reprise. Faites vérifier vos droits sur ce périmètre.",
  invalid: "Le contrôle a refusé la demande comme invalide. Demandez à l’opérateur de vérifier la demande.",
  unknown: "La reprise n’a pas fourni de reçu. Relisez vos accès.",
};
const SUBJECT_REFUSAL = {
  membership: {
    conflict: "Le contrôle signale un conflit. Demandez à l’opérateur de vérifier la révision.",
    not_found: "L’appartenance est introuvable dans ce périmètre. Demandez à l’opérateur de vérifier l’appartenance.",
  },
  grant: {
    conflict: "Le contrôle signale un conflit. Relisez les partages et les membres avant de confirmer un nouveau changement.",
    not_found: "La ressource n’est plus disponible pour gérer ses partages dans ce périmètre.",
  },
};
export function delegatedReplayRefusal(kind: DelegatedChangeKind, error: unknown) {
  const phase = changeFailure(error);
  const reason = phase === "conflict" || phase === "not_found" ? SUBJECT_REFUSAL[kind][phase] : REPLAY_REFUSAL[phase];
  return `${reason} ${DELEGATED_OUTCOME_UNKNOWN}`;
}
export function delegatedContextLost(error: unknown): boolean {
  return error instanceof HumanApiError && error.status === 403 &&
    ["context_required", "context_invalid", "context_expired", "context_revoked"].includes(error.code);
}
export interface ChangeHooks<R> {
  applied: (receipt: R) => void;
  /** 409: the membership or grant changed since it was read. The caller reads it again. */
  conflict: () => void;
  /** 403: the editor is withdrawn and the accesses are read again. */
  refused: () => void;
  /** 404: the subject is no longer in the organization. */
  missing?: () => void;
}
export class ChangeFlow<R> {
  phase: ChangePhase = "ready";
  receipt: R | null = null;
  #pending: { key: string; id: string } | null = null;
  #hooks: ChangeHooks<R>; #newId: () => string;
  constructor(hooks: ChangeHooks<R>, newId: () => string = () => globalThis.crypto.randomUUID()) { this.#hooks = hooks; this.#newId = newId; }
  /** The identifier the next request of this exact change carries. */
  operationIdFor(key: string): string {
    if (!this.#pending || this.#pending.key !== key) this.#pending = { key, id: this.#newId() };
    return this.#pending.id;
  }
  get pendingOperation() { return this.#pending?.id ?? null; }
  async submit(key: string, send: (operationId: string) => Promise<R>): Promise<ChangePhase> {
    if (this.phase === "sending") return this.phase;
    const id = this.operationIdFor(key);
    this.phase = "sending";
    try {
      const receipt = await send(id);
      this.#pending = null; this.receipt = receipt; this.phase = "applied"; this.#hooks.applied(receipt);
    } catch (error) {
      this.phase = changeFailure(error);
      if (this.phase !== "unknown") this.#pending = null;
      if (this.phase === "conflict") this.#hooks.conflict();
      if (this.phase === "refused") this.#hooks.refused();
      if (this.phase === "not_found") this.#hooks.missing?.();
    }
    return this.phase;
  }
}

interface MembershipRequest { kind: "membership"; membershipId: string; operationId: string; body: string }
interface GrantRequest { kind: "grant"; operationId: string; body: string }
type DelegatedRequest = MembershipRequest | GrantRequest;
type DelegatedReceipt = { kind: "membership"; receipt: MembershipChangeReceipt } | { kind: "grant"; receipt: GrantChangeReceipt };
type DelegatedChangeApi = Pick<HumanApiClient, "changeMembership" | "changeGrant" | "selectContext">;
interface PendingDelegatedChange {
  principal: string; contextKey: string; tenant: string; organization: string; request: DelegatedRequest;
  phase: "sending" | "unknown"; unavailable: boolean;
}

class InvalidChangeRequest extends Error {}
function prepareChangeBody(serialize: () => string) {
  try { return serialize(); }
  catch (error) {
    // A local validation failure occurs before a POST. A malformed receipt can still leave the outcome unknown.
    if (error instanceof HumanContractError) throw new InvalidChangeRequest(error.message);
    throw error;
  }
}

/** One immutable request in the current dashboard session, independent of the editor that sent it. */
export class DelegatedChangeRecovery {
  #api: DelegatedChangeApi;
  #pending: PendingDelegatedChange | null = null;
  #offer: Pick<HumanContext, "id" | "expires_at"> | null = null;
  constructor(api: DelegatedChangeApi) { this.#api = api; }

  get phase() { return this.#pending?.phase ?? null; }
  get kind() { return this.#pending?.request.kind ?? null; }
  get operationId() { return this.#pending?.request.operationId ?? null; }
  get unavailable() { return this.#pending?.unavailable ?? false; }
  withdrawOffer() { this.#offer = null; }
  noteUnavailable() { if (this.#pending?.phase === "unknown") this.#pending.unavailable = true; }
  clear() { this.#pending = null; this.withdrawOffer(); }
  abandon() {
    if (this.phase !== "unknown") return false;
    this.clear();
    return true;
  }

  async submitMembership(principal: Principal, context: HumanContext, membershipId: string, change: MembershipChange, signal?: AbortSignal) {
    const body = prepareChangeBody(() => { adminUrls.membershipChange(membershipId); return membershipChangeBody(change); });
    const request: MembershipRequest = { kind: "membership", membershipId, operationId: change.operation_id, body };
    return this.#submit(principal, context, request, () => this.#sendMembership(request, signal));
  }
  async submitGrant(principal: Principal, context: HumanContext, change: GrantChange, owner: Principal, signal?: AbortSignal) {
    const body = prepareChangeBody(() => grantChangeBody(change, owner));
    const request: GrantRequest = { kind: "grant", operationId: change.operation_id, body };
    return this.#submit(principal, context, request, () => this.#sendGrant(request, signal));
  }
  #submit<R>(principal: Principal, context: HumanContext, request: DelegatedRequest, send: () => Promise<R>) {
    if (this.#pending || context.mode !== "administration" || !can(context, DELEGATED_OPERATION[request.kind])) throw new HumanSuperseded();
    return this.#send({ principal: principalKey(principal), contextKey: grantKey(context), tenant: context.tenant_id,
      organization: context.organization_scope, request, phase: "sending", unavailable: false }, send);
  }
  #equivalent(pending: PendingDelegatedChange, context: HumanContext) {
    return grantKey(context) === pending.contextKey && can(context, DELEGATED_OPERATION[pending.request.kind]);
  }
  selectContext(principal: Principal | undefined, select: () => HumanContext) {
    if (this.phase === "sending") return null;
    const context = select();
    const pending = this.#pending;
    const keep = pending?.phase === "unknown" && principal !== undefined &&
      principalKey(principal) === pending.principal && this.#equivalent(pending, context);
    if (keep) this.withdrawOffer();
    else this.clear();
    return { context, keepChange: keep };
  }
  revalidate(identity: HumanIdentity, now: number) {
    this.withdrawOffer();
    const pending = this.#pending;
    if (!pending) return;
    if (principalKey(identity.principal) !== pending.principal) { this.clear(); return "identity_changed"; }
    const sameScope = identity.contexts.filter((context) =>
      context.tenant_id === pending.tenant && context.organization_scope === pending.organization);
    const matches = sameScope.filter((context) => this.#equivalent(pending, context));
    const managementKnown = sameScope.some((context) =>
      context.mode === "administration" || access(context, DELEGATED_OPERATION[pending.request.kind])?.permission === "not_granted");
    if (!matches.length && (identity.contexts_complete || managementKnown)) {
      this.clear();
      return "withdrawn";
    }
    const current = matches.filter((context) => Date.parse(context.expires_at) > now);
    if (pending.phase === "unknown" && current.length === 1) {
      const { id, expires_at } = current[0]!;
      this.#offer = { id, expires_at };
    }
  }
  #currentOffer(now: number) {
    const pending = this.#pending; const offer = this.#offer;
    if (pending?.phase !== "unknown" || !offer || Date.parse(offer.expires_at) <= now) return null;
    return { pending, offer };
  }
  offer(now: number) {
    const current = this.#currentOffer(now);
    if (!current) return null;
    const { pending: { request, tenant, organization }, offer } = current;
    const scope = { contextId: offer.id, expiresAt: offer.expires_at, tenant, organization };
    if (request.kind === "grant") {
      const { expected_revision, shares } = JSON.parse(request.body) as GrantChange;
      return { ...scope, kind: request.kind, expectedRevision: expected_revision, shareCount: shares.length };
    }
    const { expected_revision, capabilities, active } = JSON.parse(request.body) as MembershipChange;
    return { ...scope, kind: request.kind, expectedRevision: expected_revision, capabilities, active };
  }
  async retry(now: number, signal?: AbortSignal): Promise<DelegatedReceipt | null> {
    const current = this.#currentOffer(now);
    if (!current) return null;
    const { pending, offer } = current;
    const request = pending.request;
    return this.#send({ ...pending, phase: "sending" }, async () => {
      const context = this.#api.selectContext(offer.id);
      if (!this.#equivalent(pending, context))
        throw new HumanApiError(403, "permission_denied", "Ce contexte ne permet plus de reprendre ce changement.", "review_permissions");
      return request.kind === "membership" ? { kind: request.kind, receipt: await this.#sendMembership(request, signal) } :
        { kind: request.kind, receipt: await this.#sendGrant(request, signal) };
    });
  }
  #sendMembership(request: MembershipRequest, signal?: AbortSignal) {
    return this.#api.changeMembership(request.membershipId, JSON.parse(request.body) as MembershipChange, signal);
  }
  #sendGrant(request: GrantRequest, signal?: AbortSignal) {
    // The immutable body was validated against the owner before the first POST.
    return this.#api.changeGrant(JSON.parse(request.body) as GrantChange, undefined, signal);
  }
  async #send<R>(pending: PendingDelegatedChange, send: () => Promise<R>): Promise<R> {
    this.#pending = pending; this.withdrawOffer();
    try {
      const receipt = await send();
      if (this.#pending !== pending) throw new HumanSuperseded();
      this.clear();
      return receipt;
    } catch (error) {
      if (this.#pending === pending) {
        if (changeFailure(error) === "unknown" || delegatedContextLost(error)) {
          pending.phase = "unknown";
          pending.unavailable = error instanceof HumanApiError && error.status === 503;
        } else this.clear();
      }
      throw error;
    }
  }
}

export const CHANGE_PHASE_TEXT: Readonly<Record<Exclude<ChangePhase, "ready" | "applied">, string>> = {
  sending: "Envoi du changement au contrôle…",
  unknown: "Résultat inconnu : réessayer reprend la même opération.",
  conflict: "Le contenu a changé depuis votre lecture : il est relu. Vérifiez la nouvelle révision puis recommencez.",
  refused: "Changement refusé par le contrôle : l’éditeur est retiré et vos accès sont relus.",
  invalid: "Changement refusé : la demande ne respecte pas le contrat. Rien n’a été modifié.",
  not_found: "Ce sujet n’est plus dans votre organisation : rien n’a été modifié.",
};
