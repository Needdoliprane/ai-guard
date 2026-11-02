import assert from "node:assert/strict";
import { test } from "node:test";
import { OidcSession, OidcSessionError } from "../src/oidc-session.ts";

const authentication_id = "1b326f51-9d74-40b3-b040-d12cc86a90f6";
function tokens(generation = 1) {
  return { version: 1, authentication_id, generation, token_type: "Bearer",
    access_token: `private-access-${generation}`, refresh_token: `private-refresh-${generation}`,
    expires_at: new Date(Date.now() + 120_000).toISOString(), expires_in_seconds: 120 };
}

test("OIDC disabled status is explicit and does not start a provider flow", async () => {
  const requests = [];
  const session = new OidcSession(async function (path, options) {
    assert.equal(this, undefined, "OIDC must call native browser fetch without a session receiver");
    requests.push({ path, options }); return Response.json({ version: 1, enabled: false });
  });
  assert.deepEqual(await session.status(), { version: 1, enabled: false });
  assert.equal(requests.length, 1); assert.equal(requests[0].path, "/auth/oidc/status");
  assert.equal(requests[0].options.cache, "no-store");
  assert.equal(session.accessToken, null); session.close();
});

test("one-time browser delivery keeps tokens in memory and business credentials out of URLs", async () => {
  const requests = [];
  const session = new OidcSession(async (path, options) => {
    requests.push({ path, options }); return Response.json(tokens());
  });
  try {
    await session.redeem(); assert.equal(session.accessToken, "private-access-1");
    assert.equal(requests[0].path, "/auth/oidc/tokens");
    assert.equal(requests[0].options.credentials, "same-origin");
    assert.equal(requests[0].options.redirect, "error");
    assert.deepEqual(JSON.parse(requests[0].options.body), { version: 1 });
    assert.ok(!requests[0].path.includes("private"));
  } finally { session.close(); }
  assert.equal(session.accessToken, null);
});

test("refresh is serialized and stale responses cannot restore a closed session", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; }); let refreshes = 0;
  const session = new OidcSession(async (path, options) => {
    if (path.endsWith("tokens")) return Response.json(tokens());
    refreshes++; assert.equal(options.headers.Authorization, "Bearer private-access-1");
    assert.deepEqual(JSON.parse(options.body), { version: 1, authentication_id, generation: 1, refresh_token: "private-refresh-1" });
    entered(); await new Promise((resolve) => { release = resolve; }); return Response.json(tokens(2));
  });
  await session.redeem(); const first = session.refresh(); const second = session.refresh();
  await started; session.close(); release(); await Promise.allSettled([first, second]);
  assert.equal(refreshes, 1); assert.equal(session.accessToken, null);
});

test("logout clears local tokens before the server or IdP can fail", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const session = new OidcSession(async (path, options) => {
    if (path.endsWith("tokens")) return Response.json(tokens());
    assert.equal(path, "/auth/oidc/logout"); assert.equal(options.headers.Authorization, "Bearer private-access-1");
    entered(); await new Promise((resolve) => { release = resolve; }); throw new Error("network unavailable");
  });
  await session.redeem(); const pending = session.logout().catch((error) => error); await started;
  assert.equal(session.accessToken, null); release(); const error = await pending;
  assert.ok(error instanceof OidcSessionError); assert.ok(!error.message.includes("private-access"));
});

test("malformed token delivery never grants a bearer and exposes no provider body", async () => {
  const session = new OidcSession(async () => Response.json({ ...tokens(), token_type: "ID", error_description: "private-provider-body" }));
  await assert.rejects(session.redeem(), (error) => error instanceof OidcSessionError && !error.message.includes("private-provider-body"));
  assert.equal(session.accessToken, null); session.close();
});
