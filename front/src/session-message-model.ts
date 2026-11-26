// Strict wire models of the SendSessionMessage journey served by the owned human routes:
// POST /api/v1/human/owned/sessions/messages and GET /api/v1/human/owned/sessions/messages/{operation_id}.
// Pure: no DOM, no network, no clock. The control revalidates the personal SendSessionMessage context, the session's
// owner ACL, the retained binding (STOP, deadline, epoch pin) and the masking policy before the mandatory audit and the
// message row commit together. The browser only names the session, the input epoch it read and the short instruction;
// the body is protected tenant business data, never duplicated into the audit, URLs, argv, logs or errors.
// Truthful delivery: queued (durably recorded) != transmitted (a complete frame written to the exact owned channel) !=
// acknowledged (an exact correlated official protocol response) != unknown (possible emission, no reconcilable ack).
// None of these states means the instruction was executed.
import { HumanContractError, boundedText, object, uuid, type HumanContext } from "./human-model.ts";
import { can } from "./access-model.ts";
import type { SessionLaunchReceipt } from "./session-launch-model.ts";

function require(condition: unknown): asserts condition { if (!condition) throw new HumanContractError(); }
function closed(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  require(object(value) && Object.keys(value).length === fields.length && fields.every((field) => Object.hasOwn(value, field)));
}
const hex64 = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const epochNumber = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const auditSeq = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
// A canonical hyphenated UUID, never nil: the control refuses a nil operation or session.
const canonicalUuid = (value: unknown): value is string => uuid(value) && value === value.toLowerCase() && value !== "00000000-0000-0000-0000-000000000000";

/** Upper bound of one send body, exactly as the route enforces it. */
export const SESSION_MESSAGE_BODY_BYTES = 16384;
/** Upper bound of one short instruction body, in bytes, exactly as the control validates it. */
export const SESSION_MESSAGE_CONTENT_BYTES = 8192;

/** The exact intent of one SendSessionMessage operation: the session, the pinned input epoch and the instruction. */
export interface SessionMessageRequest {
  version: 1; operation_id: string; session_id: string; expected_input_epoch: number; content: string;
}
/** The exact correlated protocol acknowledgement of one transmitted message. */
export interface SessionMessageAck { rpc_id: number; turn_id: string; }
/** Truthful delivery status of one retained message. None means the instruction was executed. */
export type SessionMessageStatus = "queued" | "transmitted" | "acknowledged" | "unknown";
/** The durable receipt of one SendSessionMessage operation: metadata and receipts only, never the instruction body. */
export interface SessionMessageReceipt {
  version: 1; operation_id: string; message_id: string; session_id: string; input_hash: string; content_hash: string;
  binding_fingerprint: string; input_epoch: number; status: SessionMessageStatus; ack: SessionMessageAck | null;
  audit_seq: number; audit_hash: string; replayed: boolean;
}

/**
 * The destination of a send journey, derived only from a real retained launch receipt's conversation binding.
 * The browser never invents a session or an epoch: an arbitrary UUID is not a binding. null when the binding's
 * session is not a canonical UUID the message contract can pin.
 */
export interface SessionMessageDestination {
  sessionId: string; inputEpoch: number; tenantId: string; sourceId: string; launchTargetId: string;
  provider: string | null; protocol: string | null; expiresAt: string;
}
export function messageDestination(receipt: SessionLaunchReceipt): SessionMessageDestination | null {
  const binding = receipt.conversation;
  if (!canonicalUuid(binding.session_id)) return null;
  return { sessionId: binding.session_id, inputEpoch: binding.input_epoch, tenantId: binding.tenant_id,
    sourceId: binding.source_id, launchTargetId: binding.launch_target_id, provider: binding.provider,
    protocol: binding.protocol, expiresAt: binding.expires_at };
}
/** The original deadline keeps priority over new input: an elapsed binding can no longer be sent to. */
export function destinationExpired(destination: SessionMessageDestination, now: number): boolean {
  return Date.parse(destination.expiresAt) <= now;
}

/** Why a short instruction cannot be sent, checked against the control's own bounds before any request. */
export type ContentProblem = "empty" | "too_long" | "control";
// The control admits newline (10) and tab (9) only; every other C0/C1 control code point is refused, because a
// terminal or a framed protocol could reinterpret it. This mirrors Rust `char::is_control` minus '\n' and '\t'.
function forbiddenControl(content: string): boolean {
  for (const character of content) {
    const code = character.codePointAt(0) ?? 0;
    const control = code <= 0x1f || (code >= 0x7f && code <= 0x9f);
    if (control && code !== 0x0a && code !== 0x09) return true;
  }
  return false;
}
export function messageContentProblem(content: string): ContentProblem | null {
  if (content.length === 0) return "empty";
  if (new TextEncoder().encode(content).length > SESSION_MESSAGE_CONTENT_BYTES) return "too_long";
  if (forbiddenControl(content)) return "control";
  return null;
}
export const CONTENT_PROBLEM_TEXT: Readonly<Record<ContentProblem, string>> = {
  empty: "La consigne ne peut pas être vide.",
  too_long: "La consigne dépasse 8192 octets : raccourcissez-la. Le contrôle refuse un corps trop long avant tout envoi.",
  control: "La consigne contient un caractère de contrôle interdit. Seuls le saut de ligne et la tabulation sont admis.",
};

/** The closed body of one send, checked against its bounds before any request. The body is tenant business data. */
export function sessionMessageBody(request: SessionMessageRequest): string {
  require(request.version === 1 && canonicalUuid(request.operation_id) && canonicalUuid(request.session_id) &&
    epochNumber(request.expected_input_epoch) && messageContentProblem(request.content) === null);
  const body = JSON.stringify({ version: 1, operation_id: request.operation_id, session_id: request.session_id,
    expected_input_epoch: request.expected_input_epoch, content: request.content });
  require(new TextEncoder().encode(body).length <= SESSION_MESSAGE_BODY_BYTES);
  return body;
}

const STATUSES: readonly SessionMessageStatus[] = ["queued", "transmitted", "acknowledged", "unknown"];
function ack(value: unknown): SessionMessageAck | null {
  if (value === null) return null;
  closed(value, ["rpc_id", "turn_id"]);
  require(typeof value.rpc_id === "number" && Number.isSafeInteger(value.rpc_id) && value.rpc_id >= 0 &&
    boundedText(value.turn_id, 128) && (value.turn_id as string).length > 0);
  return { rpc_id: value.rpc_id, turn_id: value.turn_id as string };
}
/** The receipt answers exactly the operation (and session) the request named; a divergent receipt is a contract break. */
export function parseSessionMessageReceipt(value: unknown, expected: { operationId: string; sessionId?: string }): SessionMessageReceipt {
  closed(value, ["version", "operation_id", "message_id", "session_id", "input_hash", "content_hash", "binding_fingerprint",
    "input_epoch", "status", "ack", "audit_seq", "audit_hash", "replayed"]);
  require(value.version === 1 && canonicalUuid(value.operation_id) && value.operation_id === expected.operationId &&
    boundedText(value.message_id, 256) && canonicalUuid(value.session_id) &&
    (expected.sessionId === undefined || value.session_id === expected.sessionId) &&
    hex64(value.input_hash) && hex64(value.content_hash) && hex64(value.binding_fingerprint) &&
    epochNumber(value.input_epoch) && STATUSES.includes(value.status as SessionMessageStatus) &&
    auditSeq(value.audit_seq) && hex64(value.audit_hash) && typeof value.replayed === "boolean");
  const parsedAck = ack(value.ack);
  // An acknowledgement is recorded only by an exact correlated typed response: ack present iff acknowledged.
  require((parsedAck === null) === (value.status !== "acknowledged"));
  return { version: 1, operation_id: value.operation_id, message_id: value.message_id as string, session_id: value.session_id,
    input_hash: value.input_hash, content_hash: value.content_hash, binding_fingerprint: value.binding_fingerprint,
    input_epoch: value.input_epoch, status: value.status as SessionMessageStatus, ack: parsedAck,
    audit_seq: value.audit_seq, audit_hash: value.audit_hash, replayed: value.replayed };
}

export const sessionMessageUrls = {
  send: () => "/api/v1/human/owned/sessions/messages",
  receipt: (operationId: string) => { require(canonicalUuid(operationId)); return `/api/v1/human/owned/sessions/messages/${operationId}`; },
};

/**
 * The journey is offered only by the catalogue of the selected context: SendSessionMessage is a distinct mutation,
 * revalidated by the control. Read, administration, LaunchSession and ApproveLaunch never imply it; a read-only
 * context offers nothing here, and a visible button is never an authorization.
 */
export function sessionMessageOffers(context: HumanContext | null) {
  return { send: can(context, "session_message_send") };
}
/** Whether the send destination is offered at all by this context. */
export function sessionMessageAvailable(context: HumanContext | null): boolean {
  return can(context, "session_message_send");
}

/** Truthful, distinct delivery labels. None of these states means the instruction was executed. */
export const MESSAGE_STATUS_TEXT: Readonly<Record<SessionMessageStatus, string>> = {
  queued: "Enregistrée durablement (queued). Aucune trame n’a encore été écrite au canal.",
  transmitted: "Trame complète écrite au canal exact (transmitted). Aucune réponse officielle corrélée n’est encore reçue.",
  acknowledged: "Réponse officielle corrélée reçue (acknowledged). Le fournisseur a accepté l’entrée.",
  unknown: "Émission possible sans réponse réconciliable (unknown). La livraison reste incertaine ; aucun renvoi automatique.",
};
export const MESSAGE_STATUS_BADGE: Readonly<Record<SessionMessageStatus, string>> = {
  queued: "neutral", transmitted: "blue", acknowledged: "teal", unknown: "amber",
};
export const MESSAGE_NOT_EXECUTED = "Aucun de ces états ne signifie que la consigne a été exécutée. L’exécution observée reste un fait distinct, non prouvé ici.";
export const MESSAGE_UNKNOWN_WARNING = "Une émission a pu avoir lieu sans réponse réconciliable : l’incertitude est conservée et la consigne n’est jamais renvoyée automatiquement. Réessayer reprend la même opération.";