import assert from "node:assert/strict";
import { test } from "node:test";
import { OidcSession, OidcSessionError, oidcLogoutMessage } from "../src/oidc-session.ts";

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
  let release; let entered; const notices = [];
  const started = new Promise((resolve) => { entered = resolve; });
  const session = new OidcSession(async (path, options) => {
    if (path.endsWith("tokens")) return Response.json(tokens());
    assert.equal(path, "/auth/oidc/logout"); assert.equal(options.headers.Authorization, "Bearer private-access-1");
    entered(); await new Promise((resolve) => { release = resolve; }); throw new Error("https://private-provider.test/private-access-1");
  }, { onClose: (message) => notices.push(message) });
  await session.redeem(); const pending = session.logout().catch((error) => error); await started;
  assert.equal(session.accessToken, null);
  assert.match(notices[0], /onglet/i); assert.match(notices[0], /confirmation.*contrôle.*en cours/i);
  release(); const error = await pending;
  assert.ok(error instanceof OidcSessionError); assert.ok(!error.message.includes("private-access"));
  const message = oidcLogoutMessage(null);
  assert.match(message, /résultat.*contrôle indisponible/i);
  assert.match(message, /session locale.*non confirmée/i);
  assert.match(message, /fournisseur et audit.*non confirmés/i);
  assert.doesNotMatch(message, /session locale.*fermée|private-provider|private-access|https:/i);
});

test("logout with unknown control closure only confirms erasing this tab's tokens", async () => {
  const notices = [];
  const outcome = { version: 1, local_session: "unknown", provider_logout: "not_attempted", audit: "unavailable" };
  const session = new OidcSession(async (path) => {
    if (path === "/auth/oidc/tokens") return Response.json(tokens());
    assert.equal(path, "/auth/oidc/logout");
    return Response.json(outcome);
  }, { onClose: (message) => notices.push(message) });
  await session.redeem();
  assert.deepEqual(await session.logout(), outcome);
  assert.equal(session.accessToken, null);
  assert.equal(notices.length, 1);
  assert.match(notices[0], /onglet/i, "The immediate login notice must identify the browser-only effect");
  assert.doesNotMatch(notices[0], /session locale.*fermée/i,
    "Unknown control closure must not be announced as confirmed by the immediate login notice");
});

test("logout notice keeps the control's unknown closure and unavailable audit visible", () => {
  const message = oidcLogoutMessage({ version: 1, local_session: "unknown", provider_logout: "not_attempted", audit: "unavailable" });
  assert.match(message, /jetons.*effacés.*onglet/i);
  assert.match(message, /session locale.*contrôle non confirmée/i);
  assert.match(message, /fournisseur non confirmée/i);
  assert.match(message, /audit.*indisponible/i);
  assert.doesNotMatch(message, /session locale.*fermée|audit.*enregistré/i);
});

test("logout notice separates confirmed control closure from every unconfirmed provider outcome", () => {
  for (const provider_logout of ["failed", "unavailable", "unsupported", "not_attempted"]) {
    const message = oidcLogoutMessage({ version: 1, local_session: "closed", provider_logout, audit: "recorded" });
    assert.match(message, /session locale fermée.*contrôle/i);
    assert.match(message, /fournisseur non confirmée.*session peut rester ouverte/i);
    assert.match(message, /audit.*enregistré/i);
    assert.doesNotMatch(message, /fournisseur acceptée/i);
  }
});

test("logout notice reports an accepted provider request independently of unavailable audit", () => {
  const message = oidcLogoutMessage({ version: 1, local_session: "closed", provider_logout: "accepted", audit: "unavailable" });
  assert.match(message, /session locale fermée.*contrôle/i);
  assert.match(message, /demande.*fournisseur acceptée/i);
  assert.match(message, /audit.*indisponible/i);
  assert.doesNotMatch(message, /fournisseur.*fermée|audit.*enregistré/i);
});

test("malformed token delivery never grants a bearer and exposes no provider body", async () => {
  const session = new OidcSession(async () => Response.json({ ...tokens(), token_type: "ID", error_description: "private-provider-body" }));
  await assert.rejects(session.redeem(), (error) => error instanceof OidcSessionError && !error.message.includes("private-provider-body"));
  assert.equal(session.accessToken, null); session.close();
});
