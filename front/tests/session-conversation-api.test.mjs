import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError, HumanSuperseded } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { catalogue, context, identity, secondUuid, uuid } from "./human-fixtures.mjs";

const token = "synthetic-human-token";
const url = "/api/v1/human/owned/sessions/" + secondUuid + "/conversation";
const send = ["granted", "own_and_shared", "available"];
const contexts = [
  { ...context(), operations: catalogue({ session_message_send: send }) },
  context("context-b"),
  { ...context("context-admin", "administration"), operations: catalogue({ session_message_send: send }) },
];
const current = {
  version: 1, tenant_id: "tenant-a", source_id: uuid, session_id: secondUuid,
  input_epoch: 2, provider: "codex", protocol: "codex_app_server_v1",
  expires_at: "2099-01-01T00:00:00Z", status: "active",
};

async function clientFor(t, fetcher) {
  const client = new HumanApiClient(token, { onAuthentication() {}, onContextInvalidated() {} },
    (path, options) => path === "/api/v1/human/identity" ?
      Promise.resolve(Response.json({ ...identity(), contexts })) : fetcher(path, options));
  t.after(() => client.close());
  await client.identity();
  client.selectContext("context-a");
  return client;
}

test("le client lit le canal exact avec le bearer humain et le contexte personnel", async (t) => {
  let calls = 0;
  const client = await clientFor(t, async (path, options) => {
    calls++;
    assert.equal(path, url);
    assert.equal(options.method, "GET");
    assert.equal(options.headers.Authorization, "Bearer " + token);
    assert.equal(options.headers["X-AiGuard-Context"], "context-a");
    assert.equal(options.body, undefined);
    return Response.json(current);
  });
  assert.deepEqual(await client.sessionConversation(secondUuid), current);
  for (const id of ["AAAAAAAA-0000-4000-8000-000000000000", "00000000-0000-0000-0000-000000000000", secondUuid + "?extra=1"]) {
    await assert.rejects(client.sessionConversation(id), HumanContractError);
  }
  for (const id of ["context-b", "context-admin"]) {
    client.selectContext(id);
    await assert.rejects(client.sessionConversation(secondUuid), HumanApiError);
  }
  assert.equal(calls, 1);
});

test("le client refuse une projection hors contrat, hors tenant ou d’une autre session", async (t) => {
  let reply = current;
  const client = await clientFor(t, async () => Response.json(reply));
  for (const change of [
    { session_id: uuid }, { tenant_id: "tenant-b" }, { input_epoch: Number.MAX_SAFE_INTEGER + 1 },
    { status: "running" }, { runtime_instance: "private-runtime" }, { content: "private-instruction" },
    { expires_at: "2099-01-01" }, { source_id: null }, { provider: null },
  ]) {
    reply = { ...current, ...change };
    await assert.rejects(client.sessionConversation(secondUuid), HumanContractError);
  }
  reply = { ...current, status: "prepared", provider: null, protocol: null };
  assert.deepEqual(await client.sessionConversation(secondUuid), reply);
  reply = { ...current, status: "closed" };
  assert.deepEqual(await client.sessionConversation(secondUuid), reply);
});

test("changer de contexte annule la lecture et ignore une réponse arrivée après", async (t) => {
  const held = Promise.withResolvers();
  let signal;
  const client = await clientFor(t, (_path, options) => { signal = options.signal; return held.promise; });
  const pending = client.sessionConversation(secondUuid);
  client.selectContext("context-b");
  assert.equal(signal.aborted, true);
  held.resolve(Response.json(current));
  await assert.rejects(pending, HumanSuperseded);
});
