// Newest-first event read and stream start position, on the real named client over a recorded fetch.
import assert from "node:assert/strict";
import { test } from "node:test";
import { lstatSync, readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { HumanApiClient, HumanApiError, HumanRecentUnavailable } from "../src/human-api.ts";
import { HumanContractError, humanRecentEventsUrl, parseHumanRecentEventPage } from "../src/human-model.ts";
import { identity } from "./human-fixtures.mjs";
import { adminContext, contextId, eventNotFound, eventOf, eventsPage, recentPage, sessionId } from "./supervision-fixtures.mjs";

const session = { kind: "session", resource_id: sessionId(1) };
const events = (count, ref = session) => Array.from({ length: count }, (_, index) => eventOf(count - index, ref));
function client(responder) {
  const requests = []; const invalidated = [];
  const api = new HumanApiClient("fixture-human-secret-123456789", { onAuthentication: () => {}, onContextInvalidated: (error) => invalidated.push(error.code) },
    async (path, options) => { requests.push(path); return responder(path, options); });
  return { api, requests, invalidated };
}
const ready = async (responder) => {
  const made = client((path, options) => path.endsWith("/identity") ? Response.json({ ...identity(), contexts: [adminContext()] }) : responder(path, options));
  await made.api.identity(); made.api.selectContext(contextId); made.requests.length = 0;
  return made;
};
const sse = (frames) => new Response(new ReadableStream({ start(controller) { for (const frame of frames) controller.enqueue(new TextEncoder().encode(frame)); controller.close(); } }),
  { headers: { "content-type": "text/event-stream" } });
const frame = (id, items) => `event: human_audit\nid: ${id}\ndata: ${JSON.stringify(eventsPage(items))}\n\n`;

test("le flux part de la position fournie, puis reprend à la dernière position reçue", async () => {
  let streams = 0; const stop = new AbortController(); const pages = []; const positions = [];
  const { api, requests } = await ready(() => ++streams === 1 ? sse([frame("resume-opaque-1", events(2))]) : sse([]));
  const listening = api.listen({ view: "metadata", limit: 100, cursor: "live-cursor-opaque" }, stop.signal, (page, position) => { pages.push(page); positions.push(position); }, () => { if (streams >= 2) stop.abort(); });
  await listening;
  assert.deepEqual(positions, ["resume-opaque-1"], "chaque lot est remis avec sa position, pour qu’un appelant puisse reprendre après un refus temporaire");
  assert.equal(requests[0], "/api/v1/human/stream?limit=100&view=metadata&cursor=live-cursor-opaque", "la première connexion part de la position de la page lue");
  assert.equal(requests[1], "/api/v1/human/stream?limit=100&view=metadata&cursor=resume-opaque-1", "la reconnexion reprend après le dernier lot reçu");
  assert.equal(pages.length, 1); assert.equal(pages[0].items.length, 2);
  api.close();
});

test("sans position de départ le flux garde son comportement publié", async () => {
  const stop = new AbortController();
  const { api, requests } = await ready(() => { stop.abort(); return sse([]); });
  await api.listen({ view: "metadata" }, stop.signal, () => {}, () => {});
  assert.deepEqual(requests, ["/api/v1/human/stream?limit=25&view=metadata"]);
  api.close();
});

test("la lecture récente demande la route additive avec sa sélection et valide la page reçue", async () => {
  assert.equal(humanRecentEventsUrl({ view: "metadata", limit: 100 }), "/api/v1/human/events/recent?limit=100&view=metadata");
  assert.equal(humanRecentEventsUrl({ view: "metadata", limit: 100, resource: session, cursor: "older-opaque" }),
    `/api/v1/human/events/recent?limit=100&view=metadata&resource_kind=session&resource_id=${sessionId(1)}&cursor=older-opaque`);
  let body = recentPage(events(3), { has_more: true, next_cursor: "older-opaque" });
  const { api, requests } = await ready(() => Response.json(body));
  const first = await api.recentEvents({ view: "metadata", limit: 100 });
  assert.deepEqual(requests, ["/api/v1/human/events/recent?limit=100&view=metadata"]);
  assert.equal(first.live_cursor, "live-cursor-opaque"); assert.equal(first.next_cursor, "older-opaque"); assert.equal(first.items.length, 3);
  body = recentPage(events(2), { live_cursor: null });
  const older = await api.recentEvents({ view: "metadata", limit: 100, cursor: "older-opaque" });
  assert.equal(older.live_cursor, null);
  // A page for another resource, view or context is never published.
  body = recentPage(events(2, { kind: "session", resource_id: sessionId(2) }));
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 100, resource: session }), HumanContractError);
  body = recentPage(events(1), { view: "content" });
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 100 }), HumanContractError);
  api.close();
});

test("la page récente est fermée : clé en trop, ordre absent ou position de direct incohérente sont refusés", () => {
  const valid = recentPage(events(2), { has_more: true, next_cursor: "older-opaque" });
  assert.equal(parseHumanRecentEventPage(valid, true).items.length, 2);
  assert.equal(parseHumanRecentEventPage(recentPage([], {}), true).items.length, 0, "une première page vide garde sa position de direct");
  const invalid = [
    [{ ...valid, total: 2 }, true, "clé en trop"],
    [{ ...valid, order: undefined }, true, "ordre absent"],
    [{ ...valid, order: "oldest_first" }, true, "ordre inattendu"],
    [{ ...valid, live_cursor: null }, true, "première page sans position de direct"],
    [{ ...valid, live_cursor: "live-cursor-opaque" }, false, "page plus ancienne avec position de direct"],
    [{ ...valid, live_cursor: "older-opaque" }, true, "position de direct identique au curseur de page"],
    [{ ...valid, has_more: true, next_cursor: null }, true, "has_more sans curseur"],
    [{ ...valid, coverage: "tenant" }, true, "couverture inattendue"],
    [{ ...valid, items: [...valid.items, valid.items[0]] }, true, "événement en double"],
    [{ ...valid, items: Array.from({ length: 101 }, (_, index) => eventOf(index, session)) }, true, "plus de 100 éléments"],
    [{ ...valid, read_receipt: { ...valid.read_receipt, extra: true } }, true, "reçu ouvert"],
    [{ ...valid, items: [{ ...valid.items[0], metadata: { ...valid.items[0].metadata, kind: "control.identity.access" } }] }, true, "reçu d’audit présenté comme événement"],
  ];
  for (const [value, first, label] of invalid) {
    const { order, ...rest } = value; const candidate = order === undefined ? rest : value;
    assert.throws(() => parseHumanRecentEventPage(candidate, first), HumanContractError, label);
  }
});

test("un 404 sans filtre signale la route absente ; avec filtre il reste ambigu tant que la route n’a pas répondu", async () => {
  let mode = "absent";
  const { api, invalidated } = await ready(() => mode === "absent" ? Response.json(eventNotFound(), { status: 404 }) : Response.json(recentPage(events(1))));
  // A gateway that predates the route answers the ordinary not-found contract of the event detail.
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 100 }), (error) => error instanceof HumanRecentUnavailable && error.ambiguous === false);
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 25, resource: session }), (error) => error instanceof HumanRecentUnavailable && error.ambiguous === true,
    "un 404 filtré n’est pas présenté comme un refus de ressource avant que la route ait répondu");
  mode = "present"; await api.recentEvents({ view: "metadata", limit: 100 });
  mode = "absent";
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 25, resource: session }), (error) => error instanceof HumanApiError && error.code === "not_found",
    "une fois la route établie, le même 404 est le refus de cette ressource");
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 100 }), HumanRecentUnavailable, "sans filtre la route ne répond jamais 404 lorsqu’elle existe");
  assert.deepEqual(invalidated, [], "aucun de ces 404 ne ferme le contexte");
  api.close();
});

// Opt-in: exact Rust responses captured by tests/human_recent_events.rs in the same private QA run.
const directory = process.env.AIGUARD_SUPERVISION_WIRE_DIR;
const wire = (name) => {
  assert.ok(isAbsolute(directory)); const path = join(directory, name); const metadata = lstatSync(path);
  assert.ok(metadata.isFile() && !metadata.isSymbolicLink(), `${name} was captured by its Rust parent`); assert.equal(metadata.mode & 0o777, 0o600);
  return JSON.parse(readFileSync(path, "utf8"));
};
test("the real newest-first pages decode, and the real absent-route answer equals a real resource refusal", { skip: !directory }, async () => {
  const first = parseHumanRecentEventPage(wire("recent-first.json"), true);
  const older = parseHumanRecentEventPage(wire("recent-older.json"), false);
  assert.equal(first.items.length, 100); assert.equal(first.has_more, true); assert.ok(older.items.length > 0);
  const firstIds = new Set(first.items.map((item) => item.metadata.id));
  assert.ok(older.items.every((item) => !firstIds.has(item.metadata.id)));
  const received = first.items.map((item) => Date.parse(item.metadata.received_at));
  assert.deepEqual(received, [...received].sort((left, right) => right - left), "real pages are newest first");
  const absent = wire("recent-absent.json"); const refused = wire("recent-filtered-not-found.json");
  assert.deepEqual(absent.error.code, "not_found"); assert.deepEqual(Object.keys(absent.error).sort(), Object.keys(refused.error).sort());
  assert.equal(absent.error.code, refused.error.code); assert.equal(absent.error.retry_action, refused.error.retry_action);
  assert.deepEqual(absent, eventNotFound(), "the test double of the older gateway is the real body");
  const { api } = await ready(() => Response.json(absent, { status: 404 }));
  await assert.rejects(api.recentEvents({ view: "metadata", limit: 100 }), HumanRecentUnavailable);
  api.close();
});
