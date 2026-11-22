import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanSuperseded } from "../src/human-api.ts";
import { HumanContractError, parseHumanIdentity } from "../src/human-model.ts";
import { brokerContext, brokerIdentity, brokerOperations, errorResponse } from "./human-fixtures.mjs";
import { ownedBrokerPage, ownedCatalogue, ownedReadProfiles } from "./owned-brokers-fixture.mjs";

// The owned control announces the profile twice: as read profiles and as the availability of each operation.
const identity = () => ({ ...brokerIdentity(true), contexts: [{ ...brokerContext(true), operations: ownedCatalogue(brokerOperations(true)) }], read_profiles: ownedReadProfiles() });
function make(fetcher) {
  const notices = [];
  return { notices, client: new HumanApiClient("private-human-bearer-123456789", {
    onAuthentication: (e) => notices.push(e.code), onContextInvalidated: (e) => notices.push(e.code),
  }, fetcher) };
}

test("owned: les profils d’identité fermés ne donnent aucune permission", () => {
  assert.deepEqual(parseHumanIdentity(identity()).read_profiles, ownedReadProfiles());
  for (const profiles of [null, {}, { broker_registry: "legacy" }, { ...ownedReadProfiles(), extra: true },
    { ...ownedReadProfiles(), worker_registry: "available" }])
    assert.throws(() => parseHumanIdentity({ ...identity(), read_profiles: profiles }), HumanContractError);
  assert.equal(Object.hasOwn(parseHumanIdentity(brokerIdentity()), "read_profiles"), false);
  // Both announcements come from the same server fact: a context still serving the worker registry contradicts them.
  for (const operation of ["workers_read", "federation_read"])
    assert.throws(() => parseHumanIdentity({ ...identity(), contexts: [{ ...brokerContext(true),
      operations: ownedCatalogue(brokerOperations(true)).map((entry) => entry.operation === operation ? { ...entry, availability: "available" } : entry) }] }), HumanContractError, operation);
});

test("owned: la route et le parser sont choisis par l’identité avant la lecture", async () => {
  const calls = []; const { client } = make(async (path) => {
    calls.push(path); return Response.json(path.endsWith("/identity") ? identity() : ownedBrokerPage());
  });
  try {
    await client.identity(); client.selectContext("context-brokers");
    assert.equal((await client.brokers()).profile, "human_broker_registry_owned_v1");
    assert.deepEqual(calls, ["/api/v1/human/identity", "/api/v1/human/owned/brokers?limit=25"]);
    for (const read of [() => client.workers(), () => client.federationWorkers()])
      await assert.rejects(read(), (e) => e.code === "read_profile_unavailable");
    assert.equal(calls.length, 2, "Aucune route legacy worker/fédération n’est tentée");
  } finally { client.close(); }
});

for (const status of [403, 404, 503]) test(`owned: HTTP ${status} ne déclenche aucun repli historique`, async () => {
  const calls = []; const { client } = make(async (path) => {
    calls.push(path); return path.endsWith("/identity") ? Response.json(identity()) : errorResponse(status, status === 403 ? "permission_denied" : "not_found");
  });
  try {
    await client.identity(); client.selectContext("context-brokers"); await assert.rejects(client.brokers());
    assert.deepEqual(calls, ["/api/v1/human/identity", "/api/v1/human/owned/brokers?limit=25"]);
  } finally { client.close(); }
});

test("owned: changement de profil retire le contexte et une ancienne réponse ne le repeuple pas", async () => {
  let legacy = false; let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const { client, notices } = make(async (path) => {
    if (path.endsWith("/identity")) return Response.json(legacy ? brokerIdentity(true) : identity());
    entered(); await new Promise((resolve) => { release = resolve; }); return Response.json(ownedBrokerPage());
  });
  try {
    await client.identity(); client.selectContext("context-brokers"); const pending = client.brokers(); await started;
    legacy = true; await client.identity(); assert.equal(client.context, null); assert.deepEqual(notices, ["context_revoked"]);
    release(); await assert.rejects(pending, HumanSuperseded);
  } finally { client.close(); }
});

test("owned: 403 est signalé avant lecture ou annulation du corps", async () => {
  let cancelled = false; const body = new ReadableStream({ pull() { return new Promise(() => {}); }, cancel() { cancelled = true; return new Promise(() => {}); } });
  const { client } = make(async (path) => path.endsWith("/identity") ? Response.json(identity()) : new Response(body, { status: 403 }));
  try {
    await client.identity(); client.selectContext("context-brokers");
    await assert.rejects(client.brokers(), (e) => e.status === 403); assert.equal(cancelled, true);
  } finally { client.close(); }
});
