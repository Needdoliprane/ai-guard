import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient } from "../src/human-api.ts";
import { HumanContractError, parseHumanIdentity } from "../src/human-model.ts";
import { SupervisionStore } from "../src/supervision-store.ts";
import { accessLines, fleetCoverage, supervisionAccess } from "../src/supervision-model.ts";
import { catalogue, identity, errorResponse } from "./human-fixtures.mjs";
import { adminContext, contextId, fleetWorker, hexId, ownedAdminOperations, recentPage, resourcesPage, sessionId, sessionResource, workersPage } from "./supervision-fixtures.mjs";
import { ownedBrokerPage, ownedReadProfiles } from "./owned-brokers-fixture.mjs";

// The owned profile is announced twice by the control: as read profiles, and as the availability of each operation.
const frame = () => ({ ...identity(), contexts: [adminContext(undefined, { operations: ownedAdminOperations() })], read_profiles: ownedReadProfiles() });
function workersFrame(granted = true) {
  const value = frame();
  value.read_profiles.worker_registry = "human_worker_registry_owned_v1";
  const access = granted ? { permission: "granted", scope: "tenant" } : { permission: "not_granted" };
  const context = value.contexts[0];
  context.operations = context.operations.map((entry) => entry.operation === "workers_read" ?
    { operation: entry.operation, ...access, availability: "available" } : entry);
  return value;
}
const clock = () => ({ now: () => 100000, visible: () => true, setTimer: () => 1, clearTimer() {} });
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
const workersRoute = "/api/v1/human/workers?limit=100";
const brokersRoute = "/api/v1/human/owned/brokers?limit=100";
const resourcesRoute = "/api/v1/human/resources?limit=100";

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

test("owned: workers et brokers gardent leurs fenêtres, reçus et fraîcheurs indépendantes", async () => {
  const contradiction = frame();
  contradiction.read_profiles.worker_registry = "human_worker_registry_owned_v1";
  assert.throws(() => parseHumanIdentity(contradiction), HumanContractError);
  const unknown = workersFrame();
  unknown.read_profiles.federation_registry = "human_federation_registry_owned_v1";
  assert.throws(() => parseHumanIdentity(unknown), HumanContractError);

  const first = workersPage([fleetWorker(1)], { has_more: true, next_cursor: "owned-worker-next" });
  const nextRoute = `${workersRoute}&cursor=${first.next_cursor}`;
  const next = workersPage([fleetWorker(2)], { read_receipt: { ...first.read_receipt, audit_event_id: hexId("a", 2) } });
  const calls = []; let store; let now = 100000; let brokerReads = 0; let workersDown = false; let storeBusy = false;
  const expectCalls = (...expected) => assert.deepEqual(calls.splice(0).sort(), expected.sort());
  const client = new HumanApiClient("private-human-bearer-123456789", { onAuthentication: () => store?.close(), onContextInvalidated: () => store?.close() },
    async (path) => {
      calls.push(path);
      if (path === "/api/v1/human/identity") return Response.json(workersFrame());
      assert.equal(storeBusy, false, "Les lectures de la même source SQLite restent séquentielles");
      storeBusy = true;
      await new Promise((resolve) => setImmediate(resolve));
      storeBusy = false;
      if (path === workersRoute) return Response.json(first);
      if (path === nextRoute) return workersDown ? errorResponse(503, "audit_unavailable") : Response.json(next);
      if (path === brokersRoute) {
        const page = ownedBrokerPage(undefined, { context_id: contextId });
        page.read_receipt.audit_event_id = hexId("b", ++brokerReads);
        return Response.json(page);
      }
      if (path === resourcesRoute) return Response.json(resourcesPage([]));
      throw new Error(`unexpected ${path}`);
    });
  try {
    await client.identity(); const context = client.selectContext(contextId);
    store = new SupervisionStore(client, context, { ...clock(), now: () => now }, () => {});
    await store.open();
    expectCalls("/api/v1/human/identity", workersRoute, brokersRoute, resourcesRoute);
    assert.equal(store.state.access.composed, false);
    assert.deepEqual(store.state.workers.value.receipts, [first.read_receipt]);
    assert.equal(store.state.brokers.value.receipt.audit_event_id, hexId("b", 1));

    now += 1000;
    await store.loadMoreWorkers();
    expectCalls(nextRoute);
    assert.deepEqual(store.state.workers.value.items.map((item) => item.worker_id), [first.items[0].worker_id, next.items[0].worker_id]);
    assert.deepEqual(store.state.workers.value.receipts, [first.read_receipt, next.read_receipt]);
    assert.equal(store.state.brokers.loadedAt, 100000);

    now += 1000;
    await store.refresh(true);
    expectCalls(workersRoute, nextRoute, brokersRoute);
    assert.equal(store.state.workers.loadedAt, now);
    assert.equal(store.state.brokers.loadedAt, now);
    assert.equal(store.state.brokers.value.receipt.audit_event_id, hexId("b", 2));

    now += 1000;
    const workers = store.state.workers.value;
    workersDown = true;
    await store.refresh(true);
    expectCalls(workersRoute, nextRoute, brokersRoute);
    assert.equal(store.state.workers.value, workers);
    assert.equal(store.state.workers.stale, true);
    assert.equal(store.state.workers.loadedAt, now - 1000);
    assert.equal(store.state.brokers.stale, false);
    assert.equal(store.state.brokers.loadedAt, now);
    assert.equal(store.state.brokers.value.receipt.audit_event_id, hexId("b", 3));
  } finally { store?.close(); client.close(); }
});

test("owned: le refus et le retrait du droit workers ne laissent aucune ancienne fenêtre", { timeout: 5000 }, async () => {
  const calls = []; let store; let refuseWorkers = false; let workersGranted = true; let holdDetail = false;
  const detailStarted = deferred(); const detailHeld = deferred(); const errorBodyHeld = deferred(); const accessDoubt = deferred();
  const client = new HumanApiClient("private-human-bearer-123456789", { onAuthentication: () => store?.close(), onContextInvalidated: () => store?.close() },
    async (path, options) => {
      calls.push(path);
      if (path === "/api/v1/human/identity") return Response.json(workersFrame(workersGranted));
      if (path === workersRoute) {
        if (refuseWorkers) return new Response(new ReadableStream({ pull: () => errorBodyHeld.promise }), { status: 403 });
        return Response.json(workersPage([fleetWorker(1)]));
      }
      if (path === brokersRoute) return Response.json(ownedBrokerPage(undefined, { context_id: contextId }));
      if (path === resourcesRoute) return Response.json(resourcesPage([sessionResource(1)]));
      if (path.startsWith("/api/v1/human/events/recent?")) {
        if (holdDetail) { detailStarted.resolve(options.signal); await detailHeld.promise; }
        return Response.json(recentPage([], { stream_mode: "snapshot_only", live_cursor: null }));
      }
      throw new Error(`unexpected ${path}`);
    });
  try {
    await client.identity(); const context = client.selectContext(contextId);
    store = new SupervisionStore(client, context, clock(), () => {}, () => accessDoubt.resolve());
    await store.open(); await store.readSessionEvents(sessionId(1));
    assert.equal(store.state.session.phase, "ready");

    holdDetail = true;
    const oldDetail = store.readSessionEvents(sessionId(1));
    const detailSignal = await detailStarted.promise;
    refuseWorkers = true;
    const refreshing = store.refresh(true);
    await accessDoubt.promise;
    assert.equal(detailSignal.aborted, true);
    for (const source of [store.state.workers, store.state.brokers, store.state.resources]) assert.equal(source.value, null);
    assert.equal(store.state.session, null);

    workersGranted = false;
    await client.identity();
    assert.equal(client.context, null);
    assert.equal(store.state.closed, true);
    detailHeld.resolve(); await Promise.all([oldDetail, refreshing]);
    assert.equal(store.state.session, null, "Une réponse retenue ne repeuple pas l’ancien contexte");

    refuseWorkers = false;
    const start = calls.length;
    store = new SupervisionStore(client, client.selectContext(contextId), clock(), () => {});
    await store.open();
    assert.deepEqual(calls.slice(start).sort(), [brokersRoute, resourcesRoute].sort());
    assert.equal(store.state.workers.phase, "not_granted");
    assert.equal(store.state.brokers.phase, "ready");
    assert.ok(!calls.some((path) => path.startsWith("/api/v1/human/federation/")));
  } finally { detailHeld.resolve(); errorBodyHeld.resolve(); store?.close(); client.close(); }
});
