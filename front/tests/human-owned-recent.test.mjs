// Test-only snapshot profile contract over the existing decoder and Journal.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient } from "../src/human-api.ts";
import { HumanContractError, parseHumanRecentEventPage } from "../src/human-model.ts";
import { JournalStore } from "../src/journal-store.ts";
import { identity } from "./human-fixtures.mjs";
import { adminContext, contextId, recentPage } from "./supervision-fixtures.mjs";

const snapshot = (extra = {}) => recentPage([], { live_cursor: null, stream_mode: "snapshot_only", ...extra });

test("owned metadata snapshot accepts an explicit snapshot_only first page", () => {
  const page = parseHumanRecentEventPage(snapshot(), true);
  assert.equal(page.stream_mode, "snapshot_only");
  assert.equal(page.live_cursor, null);
  assert.equal(page.view, "metadata");
  assert.equal(parseHumanRecentEventPage(snapshot(), false).live_cursor, null);
});

test("snapshot_only never relaxes the legacy live cursor or content contract", () => {
  for (const [body, first] of [
    [recentPage([], { live_cursor: null }), true],
    [snapshot({ stream_mode: "unknown" }), true],
    [snapshot({ view: "content" }), true],
    [snapshot({ live_cursor: "live-opaque" }), true],
    [snapshot({ live_cursor: "live-opaque" }), false],
    [snapshot({ has_more: true, next_cursor: null }), true],
  ]) assert.throws(() => parseHumanRecentEventPage(body, first), HumanContractError);
  assert.equal(parseHumanRecentEventPage(recentPage([]), true).live_cursor, "live-cursor-opaque");
});

test("the real human client keeps a snapshot Journal explicit without stream or background retry", async () => {
  const requests = [];
  const api = new HumanApiClient("fixture-owned-recent-private-credential", {
    onAuthentication: () => {}, onContextInvalidated: () => {},
  }, async (url) => {
    requests.push(url);
    if (url.endsWith("/identity")) return Response.json({ ...identity(), contexts: [adminContext()] });
    assert.match(url, /^\/api\/v1\/human\/events\/recent\?/);
    return Response.json(snapshot());
  });
  await api.identity(); api.selectContext(contextId); requests.length = 0;
  let visible = true;
  const timers = new Map();
  const time = { now: () => 5_000_000, visible: () => visible,
    setTimer: (run, ms) => { const handle = {}; timers.set(handle, { run, ms }); return handle; },
    clearTimer: (handle) => { timers.delete(handle); } };
  const store = new JournalStore(api, time, () => {});
  try {
    await store.open();
    assert.equal(store.state.phase, "ready");
    assert.equal(store.state.loadedAt, 5_000_000);
    assert.equal(store.state.live, "unavailable");
    assert.match(store.state.liveMessage, /indisponible pour ce profil/);
    assert.equal(requests.length, 1);
    assert.equal(timers.size, 0);
    visible = false; store.suspend(); visible = true; store.resume();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(requests.length, 2, "returning to the tab keeps the normal one-time revalidation read");
    assert.equal(store.state.live, "unavailable");
    assert.equal(timers.size, 0, "no stream retry or polling loop is scheduled");
    await store.reload();
    assert.equal(requests.length, 3, "the explicit refresh performs a real read");
    assert.ok(requests.every((url) => url.startsWith("/api/v1/human/events/recent?")));
  } finally { store.close(); api.close(); }
});
