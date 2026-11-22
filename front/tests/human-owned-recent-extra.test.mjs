// Complementary tests only. The frozen human-owned-recent baseline is untouched.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError } from "../src/human-api.ts";
import { HumanContractError, parseHumanRecentEventPage } from "../src/human-model.ts";
import { JournalStore, journalAnnouncement } from "../src/journal-store.ts";
import { identity } from "./human-fixtures.mjs";
import { adminContext, contextId, eventOf, recentPage, sessionId } from "./supervision-fixtures.mjs";

const session = { kind: "session", resource_id: sessionId(1) };
const snapshot = (items = [], extra = {}) => recentPage(items, { live_cursor: null, stream_mode: "snapshot_only", ...extra });
const tick = () => new Promise((resolve) => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
function clock() {
  const timers = new Map();
  return { now: () => 5_000_000, visible: () => true,
    setTimer: (run, ms) => { const handle = {}; timers.set(handle, { run, ms }); return handle; },
    clearTimer: (handle) => timers.delete(handle), timers };
}
async function client(responder, onInvalidated = () => {}, contexts = [adminContext()]) {
  const requests = [];
  const api = new HumanApiClient("fixture-recent-extra-private-credential", {
    onAuthentication: () => {}, onContextInvalidated: onInvalidated,
  }, async (path, options) => {
    requests.push({ path, options });
    return path.endsWith("/identity") ? Response.json({ ...identity(), contexts }) : responder(path, options);
  });
  await api.identity(); api.selectContext(contextId); requests.length = 0;
  return { api, requests };
}
const refusal = (status, code, retry_action = "review_permissions") =>
  Response.json({ error: { code, message: "Synthetic explicit read refusal", retry_action } }, { status });

test("snapshot marker presence is closed including explicit undefined on first and older pages", () => {
  for (const first of [true, false]) {
    for (const marker of [undefined, null, "unknown", false, {}, []]) {
      const page = snapshot([], { stream_mode: marker });
      assert.ok(Object.hasOwn(page, "stream_mode"));
      assert.throws(() => parseHumanRecentEventPage(page, first), HumanContractError);
    }
    assert.throws(() => parseHumanRecentEventPage(snapshot([], { view: "content" }), first), HumanContractError);
    assert.throws(() => parseHumanRecentEventPage(snapshot([{
      ...eventOf(1, session), content: { command: null, decision: null, result: null, data: { private: "synthetic" } },
    }]), first), HumanContractError);
    assert.throws(() => parseHumanRecentEventPage(snapshot([], { live_cursor: "opaque-live-position" }), first), HumanContractError);
  }
  // Only actual absence is the legacy form. Its first-page null stays invalid.
  assert.throws(() => parseHumanRecentEventPage(recentPage([], { live_cursor: null }), true), HumanContractError);
  assert.equal(parseHumanRecentEventPage(recentPage([], { live_cursor: null }), false).live_cursor, null);
});

test("snapshot Journal older pages retain explicit history mode and never call a stream", async () => {
  const time = clock();
  const { api, requests } = await client((path) => {
    assert.match(path, /^\/api\/v1\/human\/events\/recent\?/);
    return Response.json(path.includes("cursor=") ? snapshot([eventOf(1, session)]) :
      snapshot([eventOf(2, session)], { has_more: true, next_cursor: "older-snapshot-opaque" }));
  });
  const store = new JournalStore(api, time, () => {});
  try {
    await store.open(); await store.loadOlder();
    assert.equal(store.state.phase, "ready");
    assert.equal(store.state.streamMode, "snapshot_only");
    assert.equal(store.state.live, "unavailable");
    assert.deepEqual(store.state.items.map((item) => item.metadata.id), [eventOf(2, session).metadata.id, eventOf(1, session).metadata.id]);
    assert.equal(store.state.loadedAt, 5_000_000);
    assert.ok(store.state.receipt);
    assert.match(journalAnnouncement(store.state), /Direct indisponible pour ce profil/);
    assert.equal(time.timers.size, 0);
    assert.equal(requests.length, 2);
    assert.ok(requests.every(({ path }) => path.startsWith("/api/v1/human/events/recent?")));
  } finally { store.close(); api.close(); }
});

test("a real403 withdraws snapshot rows receipt and profile state instead of preserving stale rights", async () => {
  let deny = false;
  const { api, requests } = await client(() => deny ? refusal(403, "permission_denied") : Response.json(snapshot([eventOf(1, session)])));
  const store = new JournalStore(api, clock(), () => {});
  try {
    await store.open(); assert.equal(store.state.phase, "ready");
    assert.equal(store.state.streamMode, "snapshot_only");
    deny = true; await store.reload();
    assert.equal(store.state.phase, "forbidden");
    assert.deepEqual(store.state.items, []);
    assert.equal(store.state.receipt, null);
    assert.equal(store.state.loadedAt, null);
    assert.equal(store.state.streamMode, null);
    assert.equal(requests.length, 2);
  } finally { store.close(); api.close(); }
});

test("context revocation closes the actual Journal and removes its positive snapshot state", async () => {
  let revoked = false; let store; let invalidated = 0;
  const { api } = await client(() => revoked ? refusal(409, "context_revoked", "select_context") : Response.json(snapshot([eventOf(1, session)])),
    () => { invalidated++; store?.close(); });
  store = new JournalStore(api, clock(), () => {});
  try {
    await store.open(); assert.equal(store.state.streamMode, "snapshot_only");
    revoked = true; await store.reload();
    assert.equal(invalidated, 1);
    assert.equal(store.state.closed, true);
    assert.deepEqual(store.state.items, []);
    assert.equal(store.state.receipt, null);
    assert.equal(store.state.streamMode, null);
    assert.equal(journalAnnouncement(store.state), null);
  } finally { store.close(); api.close(); }
});

test("a late snapshot from a closed Journal cannot publish into a newly selected context", async () => {
  const pending = deferred(); const otherId = "context-recent-other";
  const { api, requests } = await client(() => pending.promise, () => {}, [adminContext(), { ...adminContext(), id: otherId }]);
  let oldChanges = 0; let newChanges = 0;
  const old = new JournalStore(api, clock(), () => { oldChanges++; });
  let current;
  try {
    const opening = old.open(); await tick();
    assert.equal(requests.length, 1);
    old.close(); api.selectContext(otherId);
    current = new JournalStore(api, clock(), () => { newChanges++; });
    const closedChanges = oldChanges;
    pending.resolve(Response.json(snapshot([eventOf(1, session)])));
    await opening;
    assert.equal(oldChanges, closedChanges);
    assert.equal(old.state.closed, true);
    assert.equal(old.state.streamMode, null);
    assert.equal(current.state.phase, "idle");
    assert.equal(current.state.streamMode, null);
    assert.deepEqual(current.state.items, []);
    assert.equal(newChanges, 0);
  } finally { pending.resolve(Response.json(snapshot())); old.close(); current?.close(); api.close(); }
});

test("an initial explicit content503 has no silent metadata fallback or change of granted capabilities", async () => {
  const capabilities = adminContext().capabilities.slice(); const operations = structuredClone(adminContext().operations);
  const { api, requests } = await client((path) => {
    assert.match(path, /view=content/);
    return refusal(503, "identity_unavailable", "retry");
  });
  try {
    await assert.rejects(api.recentEvents({ view: "content", limit: 100 }), (error) => error instanceof HumanApiError && error.status === 503);
    assert.equal(requests.length, 1);
    assert.match(requests[0].path, /view=content/);
    assert.deepEqual(api.context.capabilities, capabilities);
    assert.deepEqual(api.context.operations, operations, "a refused read changes no operation of the context offer");
  } finally { api.close(); }
});

test("an unmarked503 remains an error with no snapshot state stream retry or background polling", async () => {
  const { api, requests } = await client(() => refusal(503, "identity_unavailable", "retry"));
  const time = clock(); const store = new JournalStore(api, time, () => {});
  try {
    await store.open();
    assert.equal(store.state.phase, "error");
    assert.equal(store.state.streamMode, null);
    assert.deepEqual(store.state.items, []);
    assert.equal(store.state.receipt, null);
    assert.ok(store.state.error);
    assert.equal(time.timers.size, 0);
    assert.equal(requests.length, 1);
  } finally { store.close(); api.close(); }
});

test("a transient503 withdraws snapshot data while retaining the same context restrictive profile", async () => {
  let unavailable = false;
  const { api, requests } = await client(() => unavailable ? refusal(503, "identity_unavailable", "retry") : Response.json(snapshot([eventOf(1, session)])));
  const time = clock(); const store = new JournalStore(api, time, () => {});
  try {
    await store.open(); assert.equal(store.state.phase, "ready");
    unavailable = true; await store.reload();
    assert.equal(store.state.phase, "error");
    assert.deepEqual(store.state.items, []);
    assert.equal(store.state.receipt, null);
    assert.equal(store.state.loadedAt, null);
    assert.equal(store.state.hasOlder, false);
    assert.equal(store.state.streamMode, "snapshot_only", "a transient failure must not reactivate Content/Export coverage");
    assert.equal(store.state.live, "unavailable");
    assert.equal(time.timers.size, 0);
    unavailable = false; await store.reload();
    assert.equal(store.state.phase, "ready");
    assert.equal(store.state.items.length, 1);
    assert.equal(requests.length, 3);
    store.close(); assert.equal(store.state.streamMode, null);
  } finally { store.close(); api.close(); }
});

test("an older page cannot silently change a snapshot traversal into an unmarked profile", async () => {
  const { api, requests } = await client((path) => Response.json(path.includes("cursor=") ?
    recentPage([eventOf(1, session)], { live_cursor: null }) :
    snapshot([eventOf(2, session)], { has_more: true, next_cursor: "older-snapshot-opaque" })));
  const time = clock(); const store = new JournalStore(api, time, () => {});
  try {
    await store.open(); assert.equal(store.state.phase, "ready");
    await store.loadOlder();
    assert.equal(store.state.phase, "error");
    assert.deepEqual(store.state.items, []);
    assert.equal(store.state.receipt, null);
    assert.equal(store.state.streamMode, "snapshot_only");
    assert.ok(store.state.error);
    assert.equal(store.state.hasOlder, false);
    assert.equal(requests.length, 2);
    assert.equal(time.timers.size, 0);
  } finally { store.close(); api.close(); }
});
