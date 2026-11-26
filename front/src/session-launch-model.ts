// Strict wire models of the nominative session-launch journey served by the owned human routes:
// POST /api/v1/human/owned/sessions/prepare, POST /api/v1/human/owned/sessions/launch and
// GET /api/v1/human/owned/sessions/preparations/{operation_id}.
// Pure: no DOM, no network, no clock. The control resolves the target, the grant and the audit; the browser only
// names an opaque target and its expected revision, never a host path, argv, image, credential or provider choice.
// Accepted launch, start issued and runtime-observed running stay separate facts: a launch receipt proves the durable
// launch authority and the retained conversation binding, not a running worker.
import { HumanContractError, boundedText, object, uuid, type HumanContext } from "./human-model.ts";
import { can } from "./access-model.ts";

function require(condition: unknown): asserts condition { if (!condition) throw new HumanContractError(); }
function closed(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  require(object(value) && Object.keys(value).length === fields.length && fields.every((field) => Object.hasOwn(value, field)));
}
const hex64 = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
// RFC3339 instant, bounded as the control bounds it (40 bytes for the preparation deadline).
const instant = (value: unknown, max = 40): value is string =>
  typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= max &&
  /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{1,9})?(Z|[+-]\d\d:\d\d)$/.test(value) && Number.isFinite(Date.parse(value));
// The control carries revisions and audit positions as JSON numbers (u64 / i64), not decimal strings.
const revisionNumber = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 1 && value <= 9223372036854775807;
const auditSeq = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const epochNumber = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
// A canonical hyphenated UUID, never nil: the control refuses a nil operation or target.
const canonicalUuid = (value: unknown): value is string => uuid(value) && value === value.toLowerCase() && value !== "00000000-0000-0000-0000-000000000000";
const nullableText = (value: unknown, max: number): value is string | null => value === null || boundedText(value, max);

/** Upper bound of one prepare or launch body, exactly as the route enforces it. */
export const SESSION_LAUNCH_BODY_BYTES = 4096;
/** The preparation deadline must lie in the future; the control revalidates it against its own clock. */
export const SESSION_PREPARE_TTL_MS = 10 * 60 * 1000;

/** The exact retained intent of one preparation. `expected_target_revision` is the revision the caller read. */
export interface SessionPrepareRequest {
  version: 1; operation_id: string; launch_target_id: string; expected_target_revision: number; expires_at: string;
}
/** The durable receipt of one retained preparation. It confers no grant, reservation or start claim by itself. */
export interface SessionPrepareReceipt {
  version: 1; operation_id: string; preparation_id: string; launch_target_id: string; target_revision: number;
  input_hash: string; expires_at: string; audit_seq: number; audit_hash: string; replayed: boolean;
}
/** The exact launch commit of one retained preparation, consumed by reference and exact fingerprint. */
export interface SessionLaunchRequest {
  version: 1; operation_id: string; launch_target_id: string; expected_target_revision: number;
  preparation_operation_id: string; preparation_input_hash: string;
}
/** The retained conversation binding created during launch. Broker-side facts stay unknown until admission. */
export interface SessionConversationBinding {
  version: 1; tenant_id: string; source_id: string; session_id: string; launch_target_id: string; target_revision: number;
  broker_id: string | null; broker_incarnation: string | null; worker_id: string | null; attempt_id: string | null;
  runtime_instance: string | null; input_epoch: number; provider: string | null; protocol: string | null;
  expires_at: string; status: "prepared";
}
/** The durable receipt of one committed launch: authority and grant, not a running worker. */
export interface SessionLaunchReceipt {
  version: 1; operation_id: string; launch_id: string; session_id: string; launch_target_id: string; target_revision: number;
  preparation_operation_id: string; preparation_id: string; input_hash: string; grant_revision: number; grant_audit_seq: number;
  grant_audit_hash: string; audit_seq: number; audit_hash: string; conversation: SessionConversationBinding; replayed: boolean;
}

/** The closed body of one preparation, checked against its bounds before any request. */
export function sessionPrepareBody(request: SessionPrepareRequest): string {
  require(request.version === 1 && canonicalUuid(request.operation_id) && canonicalUuid(request.launch_target_id) &&
    revisionNumber(request.expected_target_revision) && instant(request.expires_at, 40));
  const body = JSON.stringify({ version: 1, operation_id: request.operation_id, launch_target_id: request.launch_target_id,
    expected_target_revision: request.expected_target_revision, expires_at: request.expires_at });
  require(new TextEncoder().encode(body).length <= SESSION_LAUNCH_BODY_BYTES);
  return body;
}
/** The closed body of one launch, consuming the exact retained preparation fingerprint. */
export function sessionLaunchBody(request: SessionLaunchRequest): string {
  require(request.version === 1 && canonicalUuid(request.operation_id) && canonicalUuid(request.launch_target_id) &&
    revisionNumber(request.expected_target_revision) && canonicalUuid(request.preparation_operation_id) &&
    hex64(request.preparation_input_hash));
  const body = JSON.stringify({ version: 1, operation_id: request.operation_id, launch_target_id: request.launch_target_id,
    expected_target_revision: request.expected_target_revision, preparation_operation_id: request.preparation_operation_id,
    preparation_input_hash: request.preparation_input_hash });
  require(new TextEncoder().encode(body).length <= SESSION_LAUNCH_BODY_BYTES);
  return body;
}

/** The receipt answers exactly the operation and target the request named; a divergent receipt is a contract break. */
export function parseSessionPrepareReceipt(value: unknown, expected: { operationId: string; launchTargetId?: string }): SessionPrepareReceipt {
  closed(value, ["version", "operation_id", "preparation_id", "launch_target_id", "target_revision", "input_hash", "expires_at", "audit_seq", "audit_hash", "replayed"]);
  require(value.version === 1 && canonicalUuid(value.operation_id) && value.operation_id === expected.operationId &&
    boundedText(value.preparation_id, 256) && canonicalUuid(value.launch_target_id) &&
    (expected.launchTargetId === undefined || value.launch_target_id === expected.launchTargetId) &&
    revisionNumber(value.target_revision) && hex64(value.input_hash) && instant(value.expires_at, 40) &&
    auditSeq(value.audit_seq) && hex64(value.audit_hash) && typeof value.replayed === "boolean");
  return { version: 1, operation_id: value.operation_id, preparation_id: value.preparation_id, launch_target_id: value.launch_target_id,
    target_revision: value.target_revision, input_hash: value.input_hash, expires_at: value.expires_at,
    audit_seq: value.audit_seq, audit_hash: value.audit_hash, replayed: value.replayed };
}
function conversation(value: unknown, expected: { sessionId: string; launchTargetId: string; targetRevision: number }): SessionConversationBinding {
  closed(value, ["version", "tenant_id", "source_id", "session_id", "launch_target_id", "target_revision", "broker_id", "broker_incarnation",
    "worker_id", "attempt_id", "runtime_instance", "input_epoch", "provider", "protocol", "expires_at", "status"]);
  require(value.version === 1 && boundedText(value.tenant_id, 128) && boundedText(value.source_id, 256) &&
    boundedText(value.session_id, 256) && value.session_id === expected.sessionId && canonicalUuid(value.launch_target_id) &&
    value.launch_target_id === expected.launchTargetId && revisionNumber(value.target_revision) && value.target_revision === expected.targetRevision &&
    nullableText(value.broker_id, 256) && nullableText(value.broker_incarnation, 256) && nullableText(value.worker_id, 256) &&
    nullableText(value.attempt_id, 256) && nullableText(value.runtime_instance, 256) && epochNumber(value.input_epoch) &&
    nullableText(value.provider, 128) && nullableText(value.protocol, 128) && instant(value.expires_at, 40) && value.status === "prepared");
  return { version: 1, tenant_id: value.tenant_id, source_id: value.source_id, session_id: value.session_id,
    launch_target_id: value.launch_target_id, target_revision: value.target_revision, broker_id: value.broker_id as string | null,
    broker_incarnation: value.broker_incarnation as string | null, worker_id: value.worker_id as string | null,
    attempt_id: value.attempt_id as string | null, runtime_instance: value.runtime_instance as string | null,
    input_epoch: value.input_epoch, provider: value.provider as string | null, protocol: value.protocol as string | null,
    expires_at: value.expires_at, status: "prepared" };
}
export function parseSessionLaunchReceipt(value: unknown, expected: { operationId: string; launchTargetId: string }): SessionLaunchReceipt {
  closed(value, ["version", "operation_id", "launch_id", "session_id", "launch_target_id", "target_revision", "preparation_operation_id",
    "preparation_id", "input_hash", "grant_revision", "grant_audit_seq", "grant_audit_hash", "audit_seq", "audit_hash", "conversation", "replayed"]);
  require(value.version === 1 && canonicalUuid(value.operation_id) && value.operation_id === expected.operationId &&
    boundedText(value.launch_id, 256) && boundedText(value.session_id, 256) && canonicalUuid(value.launch_target_id) &&
    value.launch_target_id === expected.launchTargetId && revisionNumber(value.target_revision) &&
    canonicalUuid(value.preparation_operation_id) && boundedText(value.preparation_id, 256) && hex64(value.input_hash) &&
    revisionNumber(value.grant_revision) && auditSeq(value.grant_audit_seq) && hex64(value.grant_audit_hash) &&
    auditSeq(value.audit_seq) && hex64(value.audit_hash) && typeof value.replayed === "boolean");
  const binding = conversation(value.conversation, { sessionId: value.session_id, launchTargetId: value.launch_target_id, targetRevision: value.target_revision });
  return { version: 1, operation_id: value.operation_id, launch_id: value.launch_id, session_id: value.session_id,
    launch_target_id: value.launch_target_id, target_revision: value.target_revision, preparation_operation_id: value.preparation_operation_id,
    preparation_id: value.preparation_id, input_hash: value.input_hash, grant_revision: value.grant_revision,
    grant_audit_seq: value.grant_audit_seq, grant_audit_hash: value.grant_audit_hash, audit_seq: value.audit_seq,
    audit_hash: value.audit_hash, conversation: binding, replayed: value.replayed };
}

export const sessionLaunchUrls = {
  prepare: () => "/api/v1/human/owned/sessions/prepare",
  launch: () => "/api/v1/human/owned/sessions/launch",
  preparation: (operationId: string) => { require(canonicalUuid(operationId)); return `/api/v1/human/owned/sessions/preparations/${operationId}`; },
};

/**
 * The journey is offered only by the catalogue of the selected context: prepare and launch are distinct mutations,
 * each revalidated by the control. A read-only context offers neither; a visible button is never an authorization.
 */
export function sessionLaunchOffers(context: HumanContext | null) {
  return { prepare: can(context, "session_launch_prepare"), launch: can(context, "session_launch") };
}
/** Whether the destination is offered at all: prepare is the entry of the journey. */
export function sessionLaunchAvailable(context: HumanContext | null): boolean {
  return can(context, "session_launch_prepare");
}
/** An opaque target reference is a canonical UUID plus a positive revision; the control resolves everything else. */
export function targetReferenceProblem(launchTargetId: string, expectedRevision: string): "target" | "revision" | null {
  if (!canonicalUuid(launchTargetId.trim())) return "target";
  if (!/^[1-9][0-9]{0,18}$/.test(expectedRevision.trim()) || BigInt(expectedRevision.trim()) > 9223372036854775807n) return "revision";
  return null;
}
export const TARGET_PROBLEM_TEXT: Readonly<Record<"target" | "revision", string>> = {
  target: "La cible doit être l’identifiant opaque (UUID) publié par le contrôle pour ce projet.",
  revision: "La révision attendue doit être un entier positif publié par le contrôle.",
};
/** A launch consumes the exact preparation it names; an expired preparation can no longer launch. */
export function preparationExpired(receipt: SessionPrepareReceipt, now: number): boolean {
  return Date.parse(receipt.expires_at) <= now;
}