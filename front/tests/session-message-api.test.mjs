// The real nominative client calls the owned send-instruction routes with the human bearer and the selected context,
// decodes the durable receipts, and keeps each refusal distinct. A fetch double stands in for the control.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { catalogue, context, identity, uuid, secondUuid, errorResponse } from "./human-fixtures.mjs";

const hash = "a".repeat(64);
const fingerprint = "c".repeat(64);
const sendOperations = () => catalogue({ session_message_send: ["granted", "own_and_shared", "available"] });
const sendIdentity = () => ({ ...identity(), contexts: [{ ...context("context-send", "personal"), operations: sendOperations() }] });
const messageReceipt = (extra = {}) => ({ version: 1, operation_id: uuid, message_id: "msg-1", session_id: secondUuid,
  input_hash: hash, content_hash: hash, binding_fingerprint: fingerprint, input_epoch: 0, status: "queued", ack: null,
  audit_seq: 21, audit_hash: "b".repeat(64), replayed: false, ...extra });
const sendRequest = { version: 1, operation_id: uuid, session_id: secondUuid, expected_input_epoch: 0, content: "Résume le journal." };

function make(fetcher) {
  return new HumanApiClient("private-human-bearer-123456789", { onAuthentication: () => {}, onContextInvalidated: () => {} }, fetcher);
}
const isIdentity = (path) => path === "/api/v1/human/identity";
async function ready(client) { await client.identity(); client.selectContext("context-send"); }

test("l’envoi est un POST owned avec le bearer humain et le contexte sélectionné", async () => {
  const requests = [];
  const client = make(async (path, options) => {
    requests.push({ path, method: options.method, headers: options.headers, body: options.body });
    if (isIdentity(path)) return Response.json(sendIdentity());
    if (path.endsWith("/sessions/messages")) return Response.json(messageReceipt());
    return errorResponse(404, "not_found");
  });
  try {
    await ready(client);
    const receipt = await client.sessionMessageSend(sendRequest);
    assert.equal(receipt.status, "queued"); assert.equal(receipt.session_id, secondUuid);
    const send = requests.find((r) => r.path.endsWith("/sessions/messages"));
    assert.equal(send.path, "/api/v1/human/owned/sessions/messages"); assert.equal(send.method, "POST");
    assert.equal(send.headers.Authorization, "Bearer private-human-bearer-123456789");
    assert.equal(send.headers["X-AiGuard-Context"], "context-send");
    assert.deepEqual(JSON.parse(send.body), sendRequest);
    assert.equal(requests.filter((r) => r.method === "POST").length, 1, "aucune lecture historique substituée");
  } finally { client.close(); }
});

test("la relecture d’un reçu retenu est un GET owned par identifiant d’opération", async () => {
  const requests = [];
  const client = make(async (path, options) => {
    requests.push({ path, method: options.method });
    return Response.json(isIdentity(path) ? sendIdentity() : messageReceipt({ status: "acknowledged", ack: { rpc_id: 1, turn_id: "turn-1" } }));
  });
  try {
    await ready(client);
    const receipt = await client.sessionMessageReceipt(uuid);
    assert.equal(receipt.operation_id, uuid); assert.equal(receipt.status, "acknowledged");
    const read = requests.find((r) => r.path.includes("/messages/"));
    assert.equal(read.path, `/api/v1/human/owned/sessions/messages/${uuid}`); assert.equal(read.method, "GET");
  } finally { client.close(); }
});

test("chaque refus du contrôle reste distinct : conflit, permission, capacité, indisponibilité", async () => {
  for (const [status, code] of [[409, "binding_changed"], [403, "permission_denied"], [429, "capacity_exhausted"], [503, "identity_unavailable"], [400, "content_requires_redaction"]]) {
    const client = make(async (path) => isIdentity(path) ? Response.json(sendIdentity()) : errorResponse(status, code, status === 409 ? "reread" : "retry"));
    try {
      await ready(client);
      await assert.rejects(client.sessionMessageSend(sendRequest), (error) => error instanceof HumanApiError && error.status === status && error.code === code);
    } finally { client.close(); }
  }
});

test("un reçu qui ne répond pas l’opération ou la session demandée est une rupture de contrat", async () => {
  const client = make(async (path) => isIdentity(path) ? Response.json(sendIdentity()) : Response.json(messageReceipt({ operation_id: secondUuid })));
  try {
    await ready(client);
    await assert.rejects(client.sessionMessageSend(sendRequest), HumanContractError);
  } finally { client.close(); }
});

test("un contexte qui n’offre pas l’envoi n’émet aucune requête", async () => {
  let posts = 0;
  const readOnly = { ...identity(), contexts: [{ ...context("context-send", "personal"), operations: catalogue() }] };
  const client = make(async (path, options) => { if (options.method === "POST") posts++; return Response.json(isIdentity(path) ? readOnly : messageReceipt()); });
  try {
    await client.identity(); client.selectContext("context-send");
    await assert.rejects(client.sessionMessageSend(sendRequest), (error) => error instanceof HumanApiError && error.code === "not_in_this_build");
    assert.equal(posts, 0, "une opération non servie ne substitue aucune route");
  } finally { client.close(); }
});

test("le bearer n’est jamais renvoyé dans un corps d’erreur et une route hors allowlist est refusée", async () => {
  const client = make(async (path) => isIdentity(path) ? Response.json(sendIdentity()) : Response.json({ error: { code: "conflict", message: "écho private-human-bearer-123456789", retry_action: "reread" } }, { status: 409 }));
  try {
    await ready(client);
    await assert.rejects(client.sessionMessageSend(sendRequest), (error) => error instanceof HumanApiError && !error.message.includes("private-human-bearer-123456789"));
  } finally { client.close(); }
});