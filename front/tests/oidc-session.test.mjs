import assert from "node:assert/strict";
import { test } from "node:test";
import { OidcSession, OidcSessionError } from "../src/oidc-session.ts";

const authentication_id = "1b326f51-9d74-40b3-b040-d12cc86a90f6";
const correlation_token = "a".repeat(64);
const CORRELATION_KEY = "aiguard.oidc.correlation_token";
function tokens(generation = 1) {
  return { version: 1, authentication_id, generation, token_type: "Bearer",
    access_token: `private-access-${generation}`, refresh_token: `private-refresh-${generation}`,
    expires_at: new Date(Date.now() + 120_000).toISOString(), expires_in_seconds: 120 };
}
function memoryStorage(initial = {}) {
  const entries = new Map(Object.entries(initial));
  return {
    getItem: (key) => (entries.has(key) ? entries.get(key) : null),
    setItem: (key, value) => { entries.set(key, String(value)); },
    removeItem: (key) => { entries.delete(key); },
    has: (key) => entries.has(key),
  };
}

test("OIDC disabled status is explicit and does not start a provider flow", async () => {
  const requests = [];
  const storage = memoryStorage();
  const session = new OidcSession(async function (path, options) {
    assert.equal(this, undefined, "OIDC must call native browser fetch without a session receiver");
    requests.push({ path, options }); return Response.json({ version: 1, enabled: false });
  }, {}, storage);
  assert.deepEqual(await session.status(), { version: 1, enabled: false });
  assert.equal(requests.length, 1); assert.equal(requests[0].path, "/auth/oidc/status");
  assert.equal(requests[0].options.cache, "no-store");
  assert.equal(session.accessToken, null); session.close();
});

test("start validates and stores correlation_token, and close clears it", async () => {
  const storage = memoryStorage();
  const session = new OidcSession(async (path, options) => {
    if (path.endsWith("status")) return Response.json({ version: 1, enabled: true, issuer: "https://idp.example.test", provider_label: "Test IdP" });
    assert.equal(path, "/auth/oidc/start");
    assert.deepEqual(JSON.parse(options.body), { version: 1 });
    return Response.json({ version: 1, authorization_url: "https://idp.example.test/auth?state=s", correlation_token });
  }, {}, storage);
  const url = await session.start();
  assert.equal(url, "https://idp.example.test/auth?state=s");
  assert.equal(storage.getItem(CORRELATION_KEY), correlation_token);
  session.close();
  assert.equal(storage.has(CORRELATION_KEY), false);
});

test("start rejects missing or malformed correlation_token and unavailable storage", async () => {
  for (const badBody of [
    { version: 1, authorization_url: "https://idp.example.test/auth?state=s" },
    { version: 1, authorization_url: "https://idp.example.test/auth?state=s", correlation_token: "not-hex" },
    { version: 1, authorization_url: "https://idp.example.test/auth?state=s", correlation_token: "A".repeat(64) },
    { version: 1, authorization_url: "https://idp.example.test/auth?state=s", correlation_token, extra: true },
  ]) {
    const storage = memoryStorage();
    const session = new OidcSession(async (path) => {
      if (path.endsWith("status")) return Response.json({ version: 1, enabled: true, issuer: "https://idp.example.test", provider_label: "Test IdP" });
      return Response.json(badBody);
    }, {}, storage);
    await assert.rejects(session.start(), (error) => error instanceof OidcSessionError && error.code === "invalid_request");
    assert.equal(storage.has(CORRELATION_KEY), false);
    session.close();
  }
  const noStorage = new OidcSession(async (path) => {
    if (path.endsWith("status")) return Response.json({ version: 1, enabled: true, issuer: "https://idp.example.test", provider_label: "Test IdP" });
    return Response.json({ version: 1, authorization_url: "https://idp.example.test/auth?state=s", correlation_token });
  }, {}, null);
  await assert.rejects(noStorage.start(), (error) => error instanceof OidcSessionError && error.code === "identity_unavailable");
  noStorage.close();
  const throwingStorage = {
    getItem: () => null,
    setItem: () => { throw new Error("storage disabled"); },
    removeItem: () => {},
  };
  const errStorageSession = new OidcSession(async (path) => {
    if (path.endsWith("status")) return Response.json({ version: 1, enabled: true, issuer: "https://idp.example.test", provider_label: "Test IdP" });
    return Response.json({ version: 1, authorization_url: "https://idp.example.test/auth?state=s", correlation_token });
  }, {}, throwingStorage);
  await assert.rejects(errStorageSession.start(), (error) => error instanceof OidcSessionError && error.code === "identity_unavailable");
  errStorageSession.close();
});

test("one-time browser delivery keeps tokens in memory and consumes correlation_token immediately", async () => {
  const requests = [];
  const storage = memoryStorage({ [CORRELATION_KEY]: correlation_token });
  const session = new OidcSession(async (path, options) => {
    assert.equal(storage.has(CORRELATION_KEY), false, "correlation_token must be removed before network request");
    requests.push({ path, options }); return Response.json(tokens());
  }, {}, storage);
  try {
    await session.redeem(); assert.equal(session.accessToken, "private-access-1");
    assert.equal(requests[0].path, "/auth/oidc/tokens");
    assert.equal(requests[0].options.credentials, "same-origin");
    assert.equal(requests[0].options.redirect, "error");
    assert.deepEqual(JSON.parse(requests[0].options.body), { version: 1, correlation_token });
    assert.ok(!requests[0].path.includes("private"));
    assert.equal(storage.has(CORRELATION_KEY), false);
  } finally { session.close(); }
  assert.equal(session.accessToken, null);
});

test("redeem fails closed when correlation_token is missing or invalid without calling /tokens", async () => {
  for (const initial of [{}, { [CORRELATION_KEY]: "invalid-token" }, { [CORRELATION_KEY]: "A".repeat(64) }]) {
    const storage = memoryStorage(initial);
    let called = false;
    const session = new OidcSession(async () => {
      called = true; return Response.json(tokens());
    }, {}, storage);
    await assert.rejects(session.redeem(), (error) => error instanceof OidcSessionError && error.code === "authentication_required");
    assert.equal(called, false);
    assert.equal(storage.has(CORRELATION_KEY), false);
    assert.equal(session.accessToken, null);
  }
});

test("refresh is serialized and stale responses cannot restore a closed session", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; }); let refreshes = 0;
  const storage = memoryStorage({ [CORRELATION_KEY]: correlation_token });
  const session = new OidcSession(async (path, options) => {
    if (path.endsWith("tokens")) return Response.json(tokens());
    refreshes++; assert.equal(options.headers.Authorization, "Bearer private-access-1");
    assert.deepEqual(JSON.parse(options.body), { version: 1, authentication_id, generation: 1, refresh_token: "private-refresh-1" });
    entered(); await new Promise((resolve) => { release = resolve; }); return Response.json(tokens(2));
  }, {}, storage);
  await session.redeem(); const first = session.refresh(); const second = session.refresh();
  await started; session.close(); release(); await Promise.allSettled([first, second]);
  assert.equal(refreshes, 1); assert.equal(session.accessToken, null);
});

test("logout clears local tokens before the server or IdP can fail", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const storage = memoryStorage({ [CORRELATION_KEY]: correlation_token });
  const session = new OidcSession(async (path, options) => {
    if (path.endsWith("tokens")) return Response.json(tokens());
    assert.equal(path, "/auth/oidc/logout"); assert.equal(options.headers.Authorization, "Bearer private-access-1");
    entered(); await new Promise((resolve) => { release = resolve; }); throw new Error("network unavailable");
  }, {}, storage);
  await session.redeem(); const pending = session.logout().catch((error) => error); await started;
  assert.equal(session.accessToken, null); release(); const error = await pending;
  assert.ok(error instanceof OidcSessionError); assert.ok(!error.message.includes("private-access"));
});

test("malformed token delivery never grants a bearer and exposes no provider body", async () => {
  const storage = memoryStorage({ [CORRELATION_KEY]: correlation_token });
  const session = new OidcSession(async () => Response.json({ ...tokens(), token_type: "ID", error_description: "private-provider-body" }), {}, storage);
  await assert.rejects(session.redeem(), (error) => error instanceof OidcSessionError && !error.message.includes("private-provider-body"));
  assert.equal(session.accessToken, null); session.close();
});
