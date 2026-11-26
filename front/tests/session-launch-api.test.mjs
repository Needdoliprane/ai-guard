// The real nominative client calls the owned session routes with the human bearer and the selected context,
// decodes the durable receipts, and keeps each refusal distinct. A fetch double stands in for the control.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { catalogue, context, identity, uuid, secondUuid, errorResponse } from "./human-fixtures.mjs";

const hash = "a".repeat(64);
const auditHash = "b".repeat(64);
const future = new Date(Date.now() + 600000).toISOString();
const launchOperations = () => catalogue({ session_launch_prepare: ["granted", "own_and_shared", "available"],
  session_launch: ["granted", "own_and_shared", "available"] });
const launchIdentity = () => ({ ...identity(), contexts: [{ ...context("context-launch", "personal"), operations: launchOperations() }] });
const prepareReceipt = (extra = {}) => ({ version: 1, operation_id: uuid, preparation_id: "prep-1", launch_target_id: secondUuid,
  target_revision: 7, input_hash: hash, expires_at: future, audit_seq: 12, audit_hash: auditHash, replayed: false, ...extra });
const launchReceipt = (extra = {}) => ({ version: 1, operation_id: uuid, launch_id: "launch-1", session_id: "session-1",
  launch_target_id: secondUuid, target_revision: 7, preparation_operation_id: secondUuid, preparation_id: "prep-1", input_hash: hash,
  grant_revision: 3, grant_audit_seq: 13, grant_audit_hash: auditHash, audit_seq: 14, audit_hash: auditHash,
  conversation: { version: 1, tenant_id: "tenant-a", source_id: "source-1", session_id: "session-1", launch_target_id: secondUuid,
    target_revision: 7, broker_id: null, broker_incarnation: null, worker_id: null, attempt_id: null, runtime_instance: null,
    input_epoch: 0, provider: null, protocol: null, expires_at: future, status: "prepared" }, replayed: false, ...extra });
const prepareRequest = { version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7, expires_at: future };
const launchRequest = { version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7,
  preparation_operation_id: secondUuid, preparation_input_hash: hash };

function make(fetcher) {
  const client = new HumanApiClient("private-human-bearer-123456789", {
    onAuthentication: () => {}, onContextInvalidated: () => {},
  }, fetcher);
  return client;
}
const isIdentity = (path) => path === "/api/v1/human/identity";
async function ready(client) { await client.identity(); client.selectContext("context-launch"); }

test("prepare et launch sont des POST owned avec le bearer humain et le contexte sélectionné", async () => {
  const requests = [];
  const client = make(async (path, options) => {
    requests.push({ path, method: options.method, headers: options.headers, body: options.body });
    if (isIdentity(path)) return Response.json(launchIdentity());
    if (path.endsWith("/sessions/prepare")) return Response.json(prepareReceipt());
    if (path.endsWith("/sessions/launch")) return Response.json(launchReceipt());
    return errorResponse(404, "not_found");
  });
  try {
    await ready(client);
    const preparation = await client.sessionPrepare(prepareRequest);
    assert.equal(preparation.target_revision, 7);
    const launch = await client.sessionLaunch(launchRequest);
    assert.equal(launch.session_id, "session-1"); assert.equal(launch.conversation.status, "prepared");
    const prepare = requests.find((r) => r.path.endsWith("/sessions/prepare"));
    const launchCall = requests.find((r) => r.path.endsWith("/sessions/launch"));
    assert.equal(prepare.path, "/api/v1/human/owned/sessions/prepare"); assert.equal(prepare.method, "POST");
    assert.equal(prepare.headers.Authorization, "Bearer private-human-bearer-123456789");
    assert.equal(prepare.headers["X-AiGuard-Context"], "context-launch");
    assert.deepEqual(JSON.parse(prepare.body), prepareRequest);
    assert.equal(launchCall.method, "POST"); assert.deepEqual(JSON.parse(launchCall.body), launchRequest);
    assert.equal(requests.filter((r) => r.method === "POST").length, 2, "aucune lecture historique substituée");
  } finally { client.close(); }
});

test("la lecture d’une préparation retenue est un GET owned par identifiant d’opération", async () => {
  const requests = [];
  const client = make(async (path, options) => {
    requests.push({ path, method: options.method });
    return Response.json(isIdentity(path) ? launchIdentity() : prepareReceipt());
  });
  try {
    await ready(client);
    const receipt = await client.sessionPreparation(uuid);
    assert.equal(receipt.operation_id, uuid);
    const read = requests.find((r) => r.path.includes("/preparations/"));
    assert.equal(read.path, `/api/v1/human/owned/sessions/preparations/${uuid}`); assert.equal(read.method, "GET");
  } finally { client.close(); }
});

test("chaque refus du contrôle reste distinct : conflit, permission, indisponibilité", async () => {
  for (const [status, code] of [[409, "conflict"], [403, "permission_denied"], [503, "identity_unavailable"], [400, "invalid_request"]]) {
    const client = make(async (path) => isIdentity(path) ? Response.json(launchIdentity()) : errorResponse(status, code, status === 409 ? "reread" : "retry"));
    try {
      await ready(client);
      await assert.rejects(client.sessionPrepare(prepareRequest), (error) => error instanceof HumanApiError && error.status === status && error.code === code);
    } finally { client.close(); }
  }
});

test("un reçu qui ne répond pas l’opération demandée est une rupture de contrat, pas un succès", async () => {
  const client = make(async (path) => isIdentity(path) ? Response.json(launchIdentity()) : Response.json(prepareReceipt({ operation_id: secondUuid })));
  try {
    await ready(client);
    await assert.rejects(client.sessionPrepare(prepareRequest), HumanContractError);
  } finally { client.close(); }
});

test("un contexte qui n’offre pas la préparation n’émet aucune requête de lancement", async () => {
  let posts = 0;
  const readOnly = { ...identity(), contexts: [{ ...context("context-launch", "personal"), operations: catalogue() }] };
  const client = make(async (path, options) => { if (options.method === "POST") posts++; return Response.json(isIdentity(path) ? readOnly : prepareReceipt()); });
  try {
    await client.identity(); client.selectContext("context-launch");
    await assert.rejects(client.sessionPrepare(prepareRequest), (error) => error instanceof HumanApiError && error.code === "not_in_this_build");
    assert.equal(posts, 0, "une opération non servie ne substitue aucune route");
  } finally { client.close(); }
});

test("un reçu de lancement 2xx illisible laisse le résultat inconnu", async () => {
  const client = make(async (path) => isIdentity(path) ? Response.json(launchIdentity()) : Response.json({ version: 1 }));
  try {
    await ready(client);
    await assert.rejects(client.sessionLaunch(launchRequest), (error) =>
      error instanceof HumanApiError && error.status === 502 && error.code === "launch_receipt_unavailable");
  } finally { client.close(); }
});
