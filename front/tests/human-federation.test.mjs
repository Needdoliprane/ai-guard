import assert from "node:assert/strict";
import { test } from "node:test";
import { HUMAN_FEDERATION_PAGE_BYTES, canReadHumanFederation, humanFederationWorkersUrl, parseHumanFederationWorkerPage, HumanContractError } from "../src/human-model.ts";
import { HumanApiClient, HumanSuperseded } from "../src/human-api.ts";
import { federationContext, federationIdentity, federationPage, workerPage, brokerPage, broker, errorResponse } from "./human-fixtures.mjs";

test("la composition garde ses deux snapshots, horloges, reçus et brokers sans worker", () => {
  const value = federationPage({ workers: workerPage([], { context_id: "context-federation" }),
    brokers: brokerPage([broker({ state: "unseen", last_heartbeat: null }), broker({ broker_id: "00000000-0000-4000-8000-000000000003", state: "revoked" })], { context_id: "context-federation" }) });
  const parsed = parseHumanFederationWorkerPage(value);
  assert.equal(parsed.workers.items.length, 0); assert.equal(parsed.brokers.items.length, 2);
  assert.equal(parsed.coverage, "partial"); assert.equal(parsed.runtime_inventory, "unknown");
  const clocks = parseHumanFederationWorkerPage(federationPage());
  assert.notEqual(clocks.workers.evaluated_at, clocks.brokers.evaluated_at);
  assert.notEqual(clocks.workers.read_receipt.audit_event_id, clocks.brokers.read_receipt.audit_event_id);
  assert.equal(clocks.workers.consistency, "snapshot_per_page"); assert.equal(clocks.brokers.consistency, "live_keyset");
});

test("la composition V1 fermée refuse une jambe absente, incompatible ou enrichie", () => {
  for (const extra of [{ version: 2 }, { coverage: "complete" }, { consistency: "global_snapshot" }, { runtime_inventory: "complete" },
    { source: "tenant_worker_broker_registry" }, { workers: null }, { brokers: null }, { total: 100 },
    { brokers: { ...federationPage().brokers, credential: "private" } }, { workers: { ...federationPage().workers, items: null } }])
    assert.throws(() => parseHumanFederationWorkerPage(federationPage(extra)), HumanContractError);
  const missing = federationPage(); delete missing.brokers;
  assert.throws(() => parseHumanFederationWorkerPage(missing), HumanContractError);
});

test("la navigation exige les deux droits existants dans le même contexte administration", () => {
  assert.equal(canReadHumanFederation(federationContext()), true);
  for (const context of [null, { ...federationContext(), mode: "personal" },
    { ...federationContext(), capabilities: ["read_worker_registry"] }, { ...federationContext(), capabilities: ["read_broker_registry"] },
    { ...federationContext(), capabilities: ["read_tenant_metadata"] }]) assert.equal(canReadHumanFederation(context), false);
});

test("les deux positions de fédération conservent leurs limites et noms indépendants", () => {
  const url = new URL(humanFederationWorkersUrl({ workerCursor: "worker-position", brokerCursor: "broker-position" }), "http://localhost");
  assert.equal(url.pathname, "/api/v1/human/federation/workers");
  assert.deepEqual([...url.searchParams], [["worker_limit", "25"], ["broker_limit", "25"], ["worker_cursor", "worker-position"], ["broker_cursor", "broker-position"]]);
  assert.equal(new URL(humanFederationWorkersUrl({ workerCursor: null, brokerCursor: "broker-position" }), "http://localhost").searchParams.has("worker_cursor"), false);
  assert.throws(() => humanFederationWorkersUrl({ workerCursor: "bad\ncursor" }), HumanContractError);
});

test("le client composition utilise une lecture humaine unique et accepte sa limite composée", async () => {
  const requests = [];
  const client = new HumanApiClient("fixture-federation-private", { onAuthentication() {}, onContextInvalidated() {} }, async (path, options) => {
    requests.push({ path, options });
    return path.endsWith("/identity") ? Response.json(federationIdentity()) : new Response(" ".repeat(260 * 1024) + JSON.stringify(federationPage()));
  });
  try {
    await client.identity(); client.selectContext("context-federation"); const page = await client.federationWorkers({ brokerCursor: "broker-position" });
    assert.equal(page.brokers.items.length, 1); assert.equal(requests.length, 2);
    assert.equal(requests[1].path, "/api/v1/human/federation/workers?worker_limit=25&broker_limit=25&broker_cursor=broker-position");
    assert.equal(requests[1].options.headers["X-AiGuard-Context"], "context-federation");
    assert.equal(requests[1].options.credentials, "omit"); assert.equal(requests[1].options.cache, "no-store");
  } finally { client.close(); }
});

test("chaque jambe est liée au contexte et à sa version ; un changement purge l'ensemble", async () => {
  for (const leg of ["workers", "brokers"]) for (const change of [{ context_id: "other-context" }, { authorization_version: "b".repeat(64) }]) {
    let invalidations = 0;
    const frame = federationPage(); Object.assign(frame[leg], change);
    const client = new HumanApiClient("fixture-federation-private", { onAuthentication() {}, onContextInvalidated() { invalidations++; } },
      async (path) => Response.json(path.endsWith("/identity") ? federationIdentity() : frame));
    try {
      await client.identity(); client.selectContext("context-federation");
      await assert.rejects(client.federationWorkers(), (error) => error.code === "context_revoked");
      assert.equal(client.context, null); assert.equal(invalidations, 1);
    } finally { client.close(); }
  }
});

test("une erreur de composition ne devient jamais deux pages vides ou un succès partiel", async () => {
  for (const status of [403, 503]) {
    const client = new HumanApiClient("fixture-federation-private", { onAuthentication() {}, onContextInvalidated() {} },
      async (path) => path.endsWith("/identity") ? Response.json(federationIdentity()) : errorResponse(status, status === 403 ? "permission_denied" : "audit_unavailable"));
    try {
      await client.identity(); client.selectContext("context-federation");
      await assert.rejects(client.federationWorkers(), (error) => error.status === status);
    } finally { client.close(); }
  }
});

test("une réponse composée tardive ne survit pas au changement de contexte", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const client = new HumanApiClient("fixture-federation-private", { onAuthentication() {}, onContextInvalidated() {} }, async (path) => {
    if (path.endsWith("/identity")) return Response.json(federationIdentity());
    entered(); await new Promise((resolve) => { release = resolve; }); return Response.json(federationPage());
  });
  try {
    await client.identity(); client.selectContext("context-federation"); const pending = client.federationWorkers();
    await started; client.clearContext(); release(); await assert.rejects(pending, HumanSuperseded);
  } finally { client.close(); }
});

test("la limite composée ne permet pas à une projection de dépasser256Kio", async () => {
  const brokers = Array.from({ length: 100 }, (_, index) => broker({ broker_id: `10000000-0000-4000-8000-${(index + 1).toString(16).padStart(12, "0")}`,
    authorities: Array.from({ length: 16 }, () => ({ authority_id: "x".repeat(256), runtime_kind: "docker", authority_id_redacted: false })) }));
  assert.throws(() => parseHumanFederationWorkerPage(federationPage({ brokers: brokerPage(brokers, { context_id: "context-federation" }) })), HumanContractError);
  const client = new HumanApiClient("fixture-federation-private", { onAuthentication() {}, onContextInvalidated() {} }, async (path) =>
    path.endsWith("/identity") ? Response.json(federationIdentity()) : new Response(" ".repeat(HUMAN_FEDERATION_PAGE_BYTES) + JSON.stringify(federationPage())));
  try {
    await client.identity(); client.selectContext("context-federation");
    await assert.rejects(client.federationWorkers(), /Réponse trop volumineuse/);
  } finally { client.close(); }
});

test("le budget partagé ne reprend qu'après validation des deux projections", async () => {
  let phase = "budget"; let calls = 0;
  const client = new HumanApiClient("fixture-federation-private", { onAuthentication() {}, onContextInvalidated() {} }, async (path) => {
    calls++;
    return path.endsWith("/identity") ? Response.json(federationIdentity()) : phase === "budget" ? errorResponse(429, "audit_read_budget_exhausted", "contact_operator") :
      Response.json(federationPage(phase === "malformed" ? { brokers: null } : {}));
  });
  try {
    await client.identity(); client.selectContext("context-federation");
    await assert.rejects(client.federationWorkers(), (error) => error.code === "audit_read_budget_exhausted");
    const stopped = calls; await assert.rejects(client.federationWorkers(), (error) => error.code === "audit_read_budget_exhausted"); assert.equal(calls, stopped);
    phase = "malformed"; client.allowReadAfterOperatorAction();
    await assert.rejects(client.federationWorkers(), HumanContractError); assert.equal(client.readBudgetStopped, true);
    phase = "success"; client.allowReadAfterOperatorAction(); await client.federationWorkers(); assert.equal(client.readBudgetStopped, false);
  } finally { client.close(); }
});

test("un401 retire la session humaine pour la composition entière", async () => {
  let authentication = 0;
  const client = new HumanApiClient("fixture-federation-private", { onAuthentication() { authentication++; }, onContextInvalidated() {} }, async (path) =>
    path.endsWith("/identity") ? Response.json(federationIdentity()) : errorResponse(401, "identity_revoked", "authenticate"));
  try {
    await client.identity(); client.selectContext("context-federation");
    await assert.rejects(client.federationWorkers(), (error) => error.status === 401);
    assert.equal(client.context, null); assert.equal(authentication, 1); await assert.rejects(client.federationWorkers(), HumanSuperseded);
  } finally { client.close(); }
});

for (const target of ["federation", "identity"]) {
  test(`headers403 ${target} publie le refus avant un corps et un cancel bloqués, sans déconnecter la personne`, async () => {
    let phase = "ready"; let released = false; let cancelled = false; let controller; let releaseCancel;
    let headersReceived; let authentication = 0; let published;
    const headers = new Promise((resolve) => { headersReceived = resolve; });
    const cancellation = new Promise((resolve) => { releaseCancel = resolve; });
    const refusalBody = new ReadableStream({
      start(value) { controller = value; },
      cancel() { cancelled = true; return cancellation; },
    });
    const client = new HumanApiClient("fixture-federation-private", { onAuthentication() { authentication++; }, onContextInvalidated() {} }, async (path) => {
      const selected = target === "identity" ? path.endsWith("/identity") : path.includes("/federation/workers?");
      if (phase === "forbidden" && selected) {
        const response = new Response(refusalBody, { status: 403, headers: { "content-type": "application/json" } });
        headersReceived(); return response;
      }
      return Response.json(path.endsWith("/identity") ? federationIdentity() : federationPage());
    });
    let pending;
    try {
      await client.identity(); client.selectContext("context-federation"); await client.federationWorkers();
      phase = "forbidden";
      pending = (target === "identity" ? client.identity() : client.federationWorkers()).then(
        () => { published = { status: "unexpected_success" }; }, (error) => { published = error; });
      await headers;
      // A real event-loop checkpoint after fetch delivered its Response. No body
      // bytes, stream completion or cancellation acknowledgement are released.
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(released, false);
      assert.equal(published?.status, 403, "headers403 doit notifier le refus avant la libération du corps ou de cancel");
      assert.equal(authentication, 0, "Un refus de permission403 ne ferme pas l’authentification humaine comme un401");
      phase = "ready";
      assert.equal((await client.identity()).display_name, federationIdentity().display_name, "Le credential reste utilisable pour relire les accès proposés");
    } finally {
      released = true;
      if (!cancelled) controller.close();
      releaseCancel(); await pending; client.close();
    }
  });
}
