// Strict wire model of the SendSessionMessage journey: closed bodies and receipts, truthful distinct statuses,
// content bounds mirroring the control, and a destination derived from the current channel. Pure.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanContractError } from "../src/human-model.ts";
import { catalogue, context, uuid, secondUuid } from "./human-fixtures.mjs";
import { SESSION_MESSAGE_BODY_BYTES, SESSION_MESSAGE_CONTENT_BYTES, messageContentProblem, messageDestination,
  destinationExpired, parseSessionMessageReceipt, sessionMessageBody, sessionMessageUrls, sessionMessageOffers,
  sessionMessageAvailable } from "../src/session-message-model.ts";

const hash = "a".repeat(64);
const fingerprint = "c".repeat(64);
const future = new Date(Date.now() + 600000).toISOString();
const NUL = String.fromCharCode(0);
const BELL = String.fromCharCode(7);
const ESC = String.fromCharCode(27);
const request = (extra = {}) => ({ version: 1, operation_id: uuid, session_id: secondUuid, expected_input_epoch: 0, content: "Résume le journal.", ...extra });
const receipt = (extra = {}) => ({ version: 1, operation_id: uuid, message_id: "msg-1", session_id: secondUuid,
  input_hash: hash, content_hash: hash, binding_fingerprint: fingerprint, input_epoch: 0, status: "queued", ack: null,
  audit_seq: 21, audit_hash: "b".repeat(64), replayed: false, ...extra });
const conversation = (extra = {}) => ({ version: 1, tenant_id: "tenant-a", source_id: "source-1", session_id: secondUuid,
  input_epoch: 4, provider: "codex", protocol: "codex_app_server_v1", expires_at: future, status: "active", ...extra });

test("le corps est fermé et porte l’intention exacte : session, époque épinglée, consigne", () => {
  const body = JSON.parse(sessionMessageBody(request()));
  assert.deepEqual(Object.keys(body).sort(), ["content", "expected_input_epoch", "operation_id", "session_id", "version"]);
  assert.equal(body.version, 1); assert.equal(body.session_id, secondUuid); assert.equal(body.expected_input_epoch, 0);
  assert.equal(body.content, "Résume le journal.");
});

test("les bornes du corps et du contenu sont celles du contrôle", () => {
  assert.equal(SESSION_MESSAGE_CONTENT_BYTES, 8192); assert.equal(SESSION_MESSAGE_BODY_BYTES, 16384);
  assert.throws(() => sessionMessageBody(request({ operation_id: "00000000-0000-0000-0000-000000000000" })), HumanContractError);
  assert.throws(() => sessionMessageBody(request({ session_id: "session-1" })), HumanContractError);
  assert.throws(() => sessionMessageBody(request({ expected_input_epoch: -1 })), HumanContractError);
  assert.throws(() => sessionMessageBody(request({ content: "" })), HumanContractError);
  assert.throws(() => sessionMessageBody(request({ content: "x".repeat(8193) })), HumanContractError);
});

test("le contenu n’admet que le saut de ligne et la tabulation comme caractères de contrôle", () => {
  assert.equal(messageContentProblem("ligne\n\tindentée"), null);
  assert.equal(messageContentProblem(""), "empty");
  assert.equal(messageContentProblem("x".repeat(8193)), "too_long");
  assert.equal(messageContentProblem("avant" + NUL + "après"), "control");
  assert.equal(messageContentProblem("sonnerie" + BELL), "control");
  assert.equal(messageContentProblem("échappement" + ESC), "control");
  assert.throws(() => sessionMessageBody(request({ content: "avant" + NUL + "après" })), HumanContractError);
});

test("le reçu est fermé et distingue les quatre statuts véridiques", () => {
  for (const status of ["queued", "transmitted", "acknowledged", "unknown"]) {
    const ack = status === "acknowledged" ? { rpc_id: 1, turn_id: "turn-1" } : null;
    const parsed = parseSessionMessageReceipt(receipt({ status, ack }), { operationId: uuid, sessionId: secondUuid });
    assert.equal(parsed.status, status);
    assert.deepEqual(parsed.ack, ack);
  }
  assert.throws(() => parseSessionMessageReceipt(receipt({ status: "executed" }), { operationId: uuid }), HumanContractError);
});

test("un acquittement n’existe que pour un statut acknowledged, et réciproquement", () => {
  assert.throws(() => parseSessionMessageReceipt(receipt({ status: "queued", ack: { rpc_id: 1, turn_id: "turn-1" } }), { operationId: uuid }), HumanContractError);
  assert.throws(() => parseSessionMessageReceipt(receipt({ status: "acknowledged", ack: null }), { operationId: uuid }), HumanContractError);
  assert.throws(() => parseSessionMessageReceipt(receipt({ status: "acknowledged", ack: { rpc_id: 1, turn_id: "" } }), { operationId: uuid }), HumanContractError);
});

test("un reçu qui ne répond pas l’opération ou la session demandée est une rupture de contrat", () => {
  assert.throws(() => parseSessionMessageReceipt(receipt({ operation_id: secondUuid }), { operationId: uuid }), HumanContractError);
  assert.throws(() => parseSessionMessageReceipt(receipt({ session_id: uuid }), { operationId: uuid, sessionId: secondUuid }), HumanContractError);
  assert.throws(() => parseSessionMessageReceipt(receipt({ binding_fingerprint: "pas-un-hash" }), { operationId: uuid }), HumanContractError);
  const extra = receipt(); extra.surprise = true;
  assert.throws(() => parseSessionMessageReceipt(extra, { operationId: uuid }), HumanContractError);
});

test("la destination exige une session canonique, active et dans son échéance", () => {
  const now = Date.parse(future) - 1;
  const destination = messageDestination(conversation(), now);
  assert.equal(destination.sessionId, secondUuid); assert.equal(destination.inputEpoch, 4);
  assert.equal(destination.tenantId, "tenant-a"); assert.equal(destination.sourceId, "source-1");
  assert.equal(destination.provider, "codex"); assert.equal(destination.protocol, "codex_app_server_v1");
  assert.equal(messageDestination(conversation({ session_id: "session-1" }), now), null);
  for (const status of ["prepared", "closed"]) assert.equal(messageDestination(conversation({ status }), now), null);
  assert.equal(messageDestination(conversation(), Date.parse(future)), null);
  assert.equal(destinationExpired(destination, Date.parse(future) + 1), true);
  assert.equal(destinationExpired(destination, Date.parse(future) - 1), false);
});

test("les URL owned sont fermées et l’offre vient du catalogue, pas d’un droit implicite", () => {
  assert.equal(sessionMessageUrls.send(), "/api/v1/human/owned/sessions/messages");
  assert.equal(sessionMessageUrls.receipt(uuid), `/api/v1/human/owned/sessions/messages/${uuid}`);
  assert.throws(() => sessionMessageUrls.receipt("pas-un-uuid"), HumanContractError);
  const sendContext = { ...context("context-send", "personal"), operations: catalogue({ session_message_send: ["granted", "own_and_shared", "available"] }) };
  assert.deepEqual(sessionMessageOffers(sendContext), { send: true });
  assert.equal(sessionMessageAvailable(sendContext), true);
  // La lecture seule, l’administration et l’approbation de lancement n’impliquent pas l’envoi.
  const readOnly = { ...context("context-read", "personal"), operations: catalogue({ launch_approve: ["granted", "own_and_shared", "available"], session_launch: ["granted", "own_and_shared", "available"] }) };
  assert.equal(sessionMessageAvailable(readOnly), false);
  assert.equal(sessionMessageAvailable(null), false);
});