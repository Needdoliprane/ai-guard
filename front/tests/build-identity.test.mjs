import assert from "node:assert/strict";
import { test } from "node:test";
import {
  parseBuildStatus, frontendIdentity, identitySummary, identityRows,
  readBuildStatus, BuildIdentityStore,
} from "../src/build-identity.ts";

const status = (component = "dashboard", version = "0.0.1", build = { id: null, revision: null }) =>
  ({ schema_version: 1, product: "ai-guard", version, component, build });
const frontend = frontendIdentity("0.0.1");
const available = (component, version = "0.0.1", build) =>
  ({ kind: "available", identity: parseBuildStatus(status(component, version, build), component) });

test("strict service decoder preserves different valid versions and rejects fields outside the public contract", () => {
  assert.equal(parseBuildStatus(status("gateway", "12.3.4-rc.1+build.02"), "gateway").version, "12.3.4-rc.1+build.02");
  for (const version of ["v0.0.1", "01.0.1", "1.2", "1.2.3-01", "1.2.3+", "1.2.3-", "1.2.3\n", "1.2.3+é", "1.2.3+" + "x".repeat(64)])
    assert.throws(() => parseBuildStatus(status("dashboard", version), "dashboard"));
  for (const bad of [null, [], { ...status(), schema_version: 2 }, { ...status(), product: "other" },
    { ...status(), component: "gateway" }, { ...status(), tenant_id: "private" },
    { ...status(), build: {} }, { ...status(), build: { id: null, revision: null, path: "private" } },
    { ...status(), version: 1 }, { ...status(), build: null }])
    assert.throws(() => parseBuildStatus(bad, "dashboard"));
});

test("build metadata is bounded and explicit; invalid optional compile metadata remains unknown", () => {
  const revision = "aB".repeat(20);
  assert.deepEqual(frontendIdentity("0.0.1", "release_1.0+-", revision),
    { version: "0.0.1", build: { id: "release_1.0+-", revision } });
  assert.equal(frontendIdentity("0.0.1", "x".repeat(64), "a".repeat(64)).build.id.length, 64);
  for (const id of ["", "x".repeat(65), "build secret", "path/file", "é", "bad\n"])
    assert.equal(frontendIdentity("0.0.1", id).build.id, null);
  for (const revision of ["", "a".repeat(39), "a".repeat(41), "g".repeat(40), " a".repeat(20), "a".repeat(40) + "\n"])
    assert.equal(frontendIdentity("0.0.1", undefined, revision).build.revision, null);
  assert.deepEqual(frontend.build, { id: null, revision: null });
  assert.throws(() => parseBuildStatus(status("dashboard", "0.0.1", { id: "bad value", revision: null }), "dashboard"));
  assert.throws(() => parseBuildStatus(status("dashboard", "0.0.1", { id: null, revision: "g".repeat(40) }), "dashboard"));
});

test("canonical build version is an assertion, never an override or independently generated identity", () => {
  assert.equal(frontendIdentity("0.0.1", undefined, undefined, "0.0.1").version, "0.0.1");
  assert.equal(frontendIdentity("0.0.1", "", "", "").version, "0.0.1", "empty Docker ARG means an absent assertion");
  assert.deepEqual(frontendIdentity("0.0.1", "", "", "").build, { id: null, revision: null });
  for (const expected of ["0.0.2", " 0.0.1", "0.0.1\n"])
    assert.throws(() => frontendIdentity("0.0.1", undefined, undefined, expected), /AI_GUARD_VERSION/);
  assert.deepEqual(frontendIdentity("0.0.1").build, { id: null, revision: null });
});

test("common delivery ID and revision detect partial updates without rewriting backend facts", () => {
  const bundle = frontendIdentity("0.0.1", "delivery-A", "a".repeat(40));
  const state = { dashboard: available("dashboard", "0.0.1", { id: "delivery-A", revision: "a".repeat(40) }),
    gateway: available("gateway", "0.0.1", { id: "delivery-B", revision: "b".repeat(40) }) };
  assert.equal(identitySummary(bundle, state).mismatch, true);
  assert.match(identitySummary(bundle, state).message, /Builds différents/);
  assert.equal(identityRows(bundle, state)[2].build.id, "delivery-B");
  const unknown = frontendIdentity("0.0.1");
  assert.equal(identitySummary(unknown, state).mismatch, true, "two known services still disagree when the bundle build is unknown");
  state.gateway = available("gateway", "0.0.1", { id: "delivery-A", revision: "b".repeat(40) });
  assert.equal(identitySummary(bundle, state).mismatch, true, "a revision divergence alone must be visible");
  state.gateway = available("gateway", "0.0.1", { id: "delivery-B", revision: "a".repeat(40) });
  assert.equal(identitySummary(bundle, state).mismatch, true, "a build ID divergence alone must be visible");
  state.gateway = available("gateway");
  assert.equal(identitySummary(bundle, state).mismatch, false, "missing identity is unknown, not invented mismatch");
  assert.match(identityRows(bundle, state)[2].build.id ?? "inconnu", /inconnu/);
});

test("revision comparison uses hexadecimal identity while preserving the received spelling", () => {
  const bundle = frontendIdentity("0.0.1", "delivery-A", "aB".repeat(20));
  const state = { dashboard: available("dashboard", "0.0.1", { id: "delivery-A", revision: "ab".repeat(20) }),
    gateway: available("gateway", "0.0.1", { id: "delivery-A", revision: "AB".repeat(20) }) };
  assert.equal(identitySummary(bundle, state).mismatch, false);
  assert.equal(identityRows(bundle, state)[2].build.revision, "AB".repeat(20));
  state.gateway = available("gateway", "0.0.1", { id: "delivery-a", revision: "AB".repeat(20) });
  assert.equal(identitySummary(bundle, state).mismatch, true, "build IDs retain their case-sensitive identity");
});

test("presentation preserves the bundle identity and never assigns its version to unknown services", () => {
  const unknown = { dashboard: { kind: "loading" }, gateway: { kind: "unavailable", reason: "unsupported" } };
  const rows = identityRows(frontend, unknown);
  assert.equal(rows[0].version, "0.0.1"); assert.equal(rows[0].label, "Interface");
  assert.equal(rows[1].version, null); assert.equal(rows[2].version, null);
  assert.match(rows[2].message, /Indisponible/); assert.match(rows[2].message, /ancienne/);
  assert.equal(identitySummary(frontend, unknown).mismatch, false);
  const mixed = { dashboard: available("dashboard", "0.0.2"), gateway: available("gateway") };
  assert.equal(identitySummary(frontend, mixed).mismatch, true);
  assert.match(identitySummary(frontend, mixed).message, /Versions différentes/);
  assert.equal(identityRows(frontend, mixed)[1].version, "0.0.2");
  assert.equal(identitySummary(frontend, { dashboard: available("dashboard"), gateway: { kind: "unavailable", reason: "network" } }).complete, false);
  assert.equal(identitySummary(frontend, { dashboard: available("dashboard"), gateway: available("gateway") }).complete, true);
});

test("public reads use fixed routes, no session credential, no redirect/cache, and preserve the actual component", async () => {
  const calls = [];
  const request = async function (path, options) {
    assert.equal(this, undefined); calls.push({ path, options });
    return Response.json(status(path.includes("gateway") ? "gateway" : "dashboard", "0.0.2"));
  };
  assert.equal((await readBuildStatus("dashboard", request)).identity.version, "0.0.2");
  assert.equal((await readBuildStatus("gateway", request)).identity.component, "gateway");
  assert.deepEqual(calls.map((call) => call.path), ["/api/v1/status", "/api/v1/gateway/status"]);
  for (const { options } of calls) {
    assert.equal(options.method, "GET"); assert.equal(options.credentials, "omit");
    assert.equal(options.cache, "no-store"); assert.equal(options.redirect, "error");
    assert.equal(options.referrerPolicy, "no-referrer"); assert.deepEqual(options.headers, { Accept: "application/json" });
    assert.ok(options.signal instanceof AbortSignal); assert.equal(options.body, undefined);
  }
});

test("old servers, HTTP errors, non-JSON, malformed or oversized bodies remain unavailable", async () => {
  for (const [response, reason] of [
    [new Response(null, { status: 404 }), "unsupported"], [new Response(null, { status: 503 }), "http"],
    [new Response("<html>old server</html>"), "invalid"],
    [new Response("{", { headers: { "content-type": "application/json" } }), "invalid"],
    [Response.json(status("gateway")), "invalid"],
    [Response.json({ ...status(), private: "never displayed" }), "invalid"],
    [new Response(" ".repeat(4097), { headers: { "content-type": "application/json" } }), "invalid"],
    [new Response("{}", { headers: { "content-type": "application/json", "content-length": "4097" } }), "invalid"],
  ]) assert.deepEqual(await readBuildStatus("dashboard", async () => response), { kind: "unavailable", reason });
  assert.deepEqual(await readBuildStatus("gateway", async () => { throw new Error("private detail"); }),
    { kind: "unavailable", reason: "network" });
});

test("deadline includes a blocked response body and cancellation does not wait for its source", async () => {
  let cancelled = false;
  const body = new ReadableStream({ pull() { return new Promise(() => {}); }, cancel() { cancelled = true; return new Promise(() => {}); } });
  const result = await readBuildStatus("dashboard", async () => new Response(body, { headers: { "content-type": "application/json" } }), 10);
  assert.deepEqual(result, { kind: "unavailable", reason: "timeout" }); assert.equal(cancelled, true);
  assert.deepEqual(await readBuildStatus("gateway", () => new Promise(() => {}), 10), { kind: "unavailable", reason: "timeout" });
});

test("byte bound applies across chunks and malformed UTF-8 is refused", async () => {
  const chunks = new ReadableStream({ start(controller) {
    controller.enqueue(new Uint8Array(2048)); controller.enqueue(new Uint8Array(2049)); controller.close();
  } });
  assert.deepEqual(await readBuildStatus("dashboard", async () => new Response(chunks, { headers: { "content-type": "application/json" } })),
    { kind: "unavailable", reason: "invalid" });
  assert.deepEqual(await readBuildStatus("dashboard", async () => new Response(new Uint8Array([0xff]), { headers: { "content-type": "application/json" } })),
    { kind: "unavailable", reason: "invalid" });
  const fragmented = new ReadableStream({ start(controller) {
    for (let index = 0; index < 129; index++) controller.enqueue(new Uint8Array(0));
    controller.close();
  } });
  assert.deepEqual(await readBuildStatus("dashboard", async () => new Response(fragmented, { headers: { "content-type": "application/json" } })),
    { kind: "unavailable", reason: "invalid" });
});

test("refresh is explicit and serialized; delayed identity reads never publish after close", async () => {
  const pending = []; const publications = [];
  const store = new BuildIdentityStore(async (path) => new Promise((resolve) => pending.push({ path, resolve })));
  store.subscribe((state) => publications.push(state));
  const first = store.refresh(); const second = store.refresh();
  assert.equal(pending.length, 2, "only one request per service while in flight");
  for (const item of pending) item.resolve(Response.json(status(item.path.includes("gateway") ? "gateway" : "dashboard")));
  await Promise.all([first, second]); assert.equal(publications.at(-1).gateway.kind, "available");
  assert.equal(pending.length, 2, "no background poll after completion");
  const last = store.refresh(); const count = publications.length; store.close();
  for (const item of pending.slice(2)) item.resolve(Response.json(status(item.path.includes("gateway") ? "gateway" : "dashboard")));
  await last; assert.equal(publications.length, count, "closed view cannot republish a late response");
});
