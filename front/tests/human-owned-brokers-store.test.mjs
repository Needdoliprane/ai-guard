import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient } from "../src/human-api.ts";
import { SupervisionStore } from "../src/supervision-store.ts";
import { accessLines, fleetCoverage, supervisionAccess } from "../src/supervision-model.ts";
import { identity, errorResponse } from "./human-fixtures.mjs";
import { adminContext, contextId, ownedAdminOperations, resourcesPage, sessionResource } from "./supervision-fixtures.mjs";
import { catalogue } from "./human-fixtures.mjs";
import { ownedBrokerPage, ownedReadProfiles } from "./owned-brokers-fixture.mjs";

// The owned profile is announced twice by the control: as read profiles, and as the availability of each operation.
const frame = () => ({ ...identity(), contexts: [adminContext(undefined, { operations: ownedAdminOperations() })], read_profiles: ownedReadProfiles() });
const clock = () => ({ now: () => 100000, visible: () => true, setTimer: () => 1, clearTimer() {} });
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };

test("owned: Supervision lit les brokers disponibles et distingue workers indisponibles d’un refus ou d’un vide", async () => {
  const calls = []; let store;
  const client = new HumanApiClient("private-human-bearer-123456789", { onAuthentication: () => store?.close(), onContextInvalidated: () => store?.close() },
    async (path) => {
      calls.push(path);
      if (path.endsWith("/identity")) return Response.json(frame());
      if (path.startsWith("/api/v1/human/owned/brokers?")) return Response.json(ownedBrokerPage(undefined, { context_id: contextId }));
      if (path.startsWith("/api/v1/human/resources?")) return Response.json(resourcesPage([sessionResource(1)]));
      throw new Error(`unexpected ${path}`);
    });
  try {
    await client.identity(); const context = client.selectContext(contextId);
    store = new SupervisionStore(client, context, clock(), () => {}); await store.open();
    assert.deepEqual(calls.sort(), ["/api/v1/human/identity", "/api/v1/human/owned/brokers?limit=100", "/api/v1/human/resources?limit=100"].sort());
    assert.equal(store.state.brokers.phase, "ready"); assert.equal(store.state.brokers.value.profile, "human_broker_registry_owned_v1");
    assert.equal(store.state.workers.phase, "unavailable"); assert.equal(store.state.workers.value, null);
    assert.equal(store.state.workers.loadedAt, null); assert.equal(store.state.access.composed, false);
    const workers = accessLines(context).find((line) => line.key === "workers");
    assert.equal(workers.state, "unavailable"); assert.match(workers.detail, /^Opération workers_read accordée .*profil de sources adopté/);
    const summary = (phase) => ({ phase, loaded: 0, hasMore: false, stale: false, capped: false });
    const coverage = fleetCoverage({ workers: summary("unavailable"), brokers: summary("ready"), resources: summary("ready"), contextsComplete: true });
    assert.match(coverage.notes.find((n) => n.source === "workers").text, /activité inconnue/);
  } finally { store?.close(); client.close(); }
});

test("owned: un contexte workers seul permet de comprendre son profil sans émettre de lecture legacy", async () => {
  const reads = { readProfiles: ownedReadProfiles(), readBudgetStopped: false };
  const context = adminContext(["read_worker_registry"], { operations: catalogue({ workers_read: ["granted", "tenant", "profile_unavailable"] }) });
  const access = supervisionAccess(context); assert.equal(access.any, true); assert.equal(access.workers, false); assert.equal(access.workersUnavailable, true);
  const store = new SupervisionStore(reads, context, clock(), () => {});
  try { await store.open(); assert.equal(store.state.workers.phase, "unavailable"); assert.equal(store.state.brokers.phase, "not_granted"); }
  finally { store.close(); }
});

test("owned: une panne conserve une page datée périmée,403 retire données et lecture ressource en vol", async () => {
  let mode = "ok"; let store; const entered = deferred(); const held = deferred(); let doubts = 0;
  const client = new HumanApiClient("private-human-bearer-123456789", { onAuthentication: () => store?.close(), onContextInvalidated: () => store?.close() },
    async (path) => {
      if (path.endsWith("/identity")) return Response.json(frame());
      if (path.startsWith("/api/v1/human/owned/brokers?")) {
        if (mode === "down") return errorResponse(503, "audit_unavailable");
        if (mode === "refused") { await entered.promise; return errorResponse(403, "permission_denied"); }
        return Response.json(ownedBrokerPage(undefined, { context_id: contextId }));
      }
      if (path.startsWith("/api/v1/human/resources?")) {
        if (mode === "refused") { entered.resolve(); await held.promise; }
        return Response.json(resourcesPage([sessionResource(1)]));
      }
      throw new Error(`unexpected ${path}`);
    });
  try {
    await client.identity(); const context = client.selectContext(contextId);
    store = new SupervisionStore(client, context, clock(), () => {}, () => { doubts++; }); await store.open();
    const value = store.state.brokers.value; const date = store.state.brokers.loadedAt;
    mode = "down"; await store.refresh(); assert.equal(store.state.brokers.value, value);
    assert.equal(store.state.brokers.stale, true); assert.equal(store.state.brokers.loadedAt, date);
    mode = "refused"; const refreshing = store.refresh(); await entered.promise;
    // Wait for the broker refusal's synchronous state publication, without resolving the held resource.
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(store.state.brokers.phase, "forbidden"); assert.equal(store.state.brokers.value, null);
    assert.equal(store.state.resources.value, null); assert.equal(store.state.resources.busy, false); assert.equal(doubts, 1);
    held.resolve(); await refreshing;
    assert.equal(store.state.resources.value, null, "La réponse ancienne ne repeuple aucune source retirée");
    assert.equal(store.state.session, null);
  } finally { held.resolve(); store?.close(); client.close(); }
});
