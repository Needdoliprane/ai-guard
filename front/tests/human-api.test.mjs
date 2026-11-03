import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError, HumanSuperseded, HUMAN_EXPORT_LIMITS } from "../src/human-api.ts";
import { identity, page, resource, event, errorResponse, uuid } from "./human-fixtures.mjs";

function make(fetcher) {
  const notifications = [];
  const client = new HumanApiClient("private-human-bearer-123456789", {
    onAuthentication: (error) => notifications.push(["authentication", error.code]),
    onContextInvalidated: (error) => notifications.push(["context", error.code]),
  }, fetcher);
  return { client, notifications };
}
const identityRoute = (path) => path === "/api/v1/human/identity";

test("les credentials humains n’utilisent que les routes humaines et les contextes proposés", async () => {
  const requests = [];
  const { client } = make(async function (path, options) {
    assert.equal(this, undefined); requests.push({ path, options });
    return Response.json(identityRoute(path) ? identity() : page(options.headers["X-AiGuard-Context"], [resource()]));
  });
  try {
    await assert.rejects(client.resources(), (error) => error.code === "context_required");
    await client.identity(); assert.throws(() => client.selectContext("invented-tenant"), /proposé/);
    client.selectContext("context-a"); await client.resources();
    assert.equal(requests.length, 2);
    assert.ok(requests.every(({ path, options }) => path.startsWith("/api/v1/human/") && options.credentials === "omit" && options.cache === "no-store" && options.redirect === "error" && options.method === "GET"));
    assert.equal(requests[1].options.headers["X-AiGuard-Context"], "context-a");
    assert.ok(requests.every(({ path }) => !path.includes("private-human") && !path.includes("tenant_id")));
  } finally { client.close(); }
});

test("une ancienne lecture ne réactive pas les données d’un autre contexte", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const { client, notifications } = make(async (path, options) => {
    if (identityRoute(path)) return Response.json(identity());
    const context = options.headers["X-AiGuard-Context"];
    if (context === "context-a") { entered(); await new Promise((resolve) => { release = resolve; }); }
    return Response.json(page(context, [resource(context)]));
  });
  try {
    await client.identity(); client.selectContext("context-a");
    const pending = client.resources(); await started;
    client.selectContext("context-b"); release();
    await assert.rejects(pending, HumanSuperseded);
    assert.equal((await client.resources()).context_id, "context-b"); assert.deepEqual(notifications, []);
  } finally { client.close(); }
});

test("401 même sans DTO ferme la session, 403 ressource ne ferme pas le compte", async () => {
  let status = 403;
  const { client, notifications } = make(async (path) => identityRoute(path) ? Response.json(identity()) :
    status === 401 ? new Response("unavailable", { status: 401 }) : errorResponse(403, "permission_denied", "review_permissions"));
  try {
    await client.identity(); client.selectContext("context-a");
    await assert.rejects(client.events({ view: "metadata" }), (error) => error instanceof HumanApiError && error.code === "permission_denied");
    assert.equal(client.context.id, "context-a"); assert.deepEqual(notifications, []);
    status = 401;
    await assert.rejects(client.resources(), (error) => error.status === 401);
    assert.equal(client.context, null); assert.deepEqual(notifications, [["authentication", "authentication_required"]]);
  } finally { client.close(); }
});

test("une version d’autorisation modifiée purge le contexte avant restitution d’une page", async () => {
  const { client, notifications } = make(async (path) => Response.json(identityRoute(path) ? identity() : page("context-a", [], { authorization_version: "b".repeat(64) })));
  try {
    await client.identity(); client.selectContext("context-a");
    await assert.rejects(client.resources(), (error) => error.code === "context_revoked");
    assert.equal(client.context, null); assert.deepEqual(notifications, [["context", "context_revoked"]]);
  } finally { client.close(); }
});

test("l’export borné ne restitue pas de snapshot partiel ni d’écho du bearer", async () => {
  let endless = false; let reads = 0;
  const { client } = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    reads++;
    return Response.json(page("context-a", [event("content", "session-a", { echo: "private-human-bearer-123456789" })], {
      view: "content", next_cursor: endless ? `cursor-${reads}` : null, has_more: endless, selection_complete: !endless,
    }));
  });
  try {
    await client.identity(); client.selectContext("context-a");
    const selection = { view: "content", resource: resource().resource };
    const exported = await client.exportSelection(selection, new AbortController().signal, () => {});
    const data = await exported.text(); assert.ok(!data.includes("private-human-bearer")); assert.ok(data.includes("[REDACTED]"));
    assert.equal(JSON.parse(data).selection_complete, true); assert.equal(JSON.parse(data).pages[0].items[0].metadata.id, uuid);
    endless = true; reads = 0;
    await assert.rejects(client.exportSelection(selection, new AbortController().signal, () => {}), /25 pages/);
    assert.equal(reads, HUMAN_EXPORT_LIMITS.pages);
  } finally { client.close(); }
});

test("SSE humain interprète son erreur terminale et ne traite pas une séquence legacy", async () => {
  const { client, notifications } = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    const body = `id: opaque-cursor\nevent: human_audit\ndata: ${JSON.stringify(page("context-a", [], { view: "metadata" }))}\n\nevent: error\ndata: ${JSON.stringify({ error: { code: "context_revoked", message: "Contexte retiré", retry_action: "select_context" } })}\n\n`;
    return new Response(body, { headers: { "content-type": "text/event-stream" } });
  });
  try {
    await client.identity(); client.selectContext("context-a"); let batches = 0;
    await client.listen({ view: "metadata" }, new AbortController().signal, () => batches++, () => {});
    assert.equal(batches, 1); assert.equal(client.context, null); assert.deepEqual(notifications, [["context", "context_revoked"]]);
  } finally { client.close(); }
});

test("la limite de réponse et l’annulation d’export ne livrent aucun contenu partiel", async () => {
  let oversized = true; let reads = 0;
  const { client } = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    reads++;
    if (oversized) return Response.json({ padding: "x".repeat(256 * 1024) });
    return Response.json(page("context-a", [], { view: "metadata", has_more: true, next_cursor: `cursor-${reads}`, selection_complete: false }));
  });
  try {
    await client.identity(); client.selectContext("context-a");
    await assert.rejects(client.resources(), /trop volumineuse/);
    oversized = false; reads = 0; const cancellation = new AbortController();
    await assert.rejects(client.exportSelection({ view: "metadata", resource: resource().resource }, cancellation.signal,
      () => cancellation.abort()), (error) => error.name === "AbortError");
    assert.equal(reads, 1, "Annuler après la première page empêche la suivante");
  } finally { client.close(); }
});

test("un401 retardé du même client ferme aussi le contexte sélectionné depuis la demande", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const { client, notifications } = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    entered(); await new Promise((resolve) => { release = resolve; });
    return errorResponse(401, "credential_invalid", "authenticate");
  });
  try {
    await client.identity(); client.selectContext("context-a");
    const pending = client.resources().catch((error) => error); await started;
    client.selectContext("context-b"); release();
    const rejected = await pending;
    assert.ok(rejected instanceof HumanApiError && rejected.status === 401, "Le refus du credential est indépendant de l’ancienne génération de contexte");
    assert.equal(client.context, null); assert.equal(notifications.length, 1); assert.equal(notifications[0][0], "authentication");
  } finally { client.close(); }
});

test("un401 ne dépend jamais de la lecture d’un corps invalide", async () => {
  let bodyRead = false;
  const { client, notifications } = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    const response = new Response("not-json", { status: 401 });
    const reader = response.body.getReader.bind(response.body);
    response.body.getReader = (...args) => { bodyRead = true; return reader(...args); };
    return response;
  });
  try {
    await client.identity(); client.selectContext("context-a");
    await assert.rejects(client.resources(), (error) => error.status === 401);
    assert.equal(bodyRead, false, "Le statut401 ferme la session sans parser son corps");
    assert.equal(client.context, null); assert.equal(notifications.length, 1);
  } finally { client.close(); }
});

test("un401 à corps suspendu ferme immédiatement sans attendre les octets", async () => {
  let body; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const { client, notifications } = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    const response = new Response(new ReadableStream({ start(controller) { body = controller; } }, { highWaterMark: 0 }), { status: 401 });
    entered(); return response;
  });
  try {
    await client.identity(); client.selectContext("context-a");
    const pending = client.resources().catch((error) => error); await started;
    await new Promise((resolve) => setImmediate(resolve));
    const whileSuspended = { context: client.context, notifications: [...notifications] };
    // Always release the real stream before assertions, including the unchanged RED implementation.
    try { body.enqueue(new TextEncoder().encode("invalid")); body.close(); } catch { /* Already cancelled by immediate401 handling. */ }
    const rejected = await pending;
    assert.equal(whileSuspended.context, null, "Le contexte doit être retiré avant la fin du corps401");
    assert.equal(whileSuspended.notifications.length, 1, "L’expiration est annoncée avant réception du corps");
    assert.equal(rejected.status, 401);
  } finally { try { body?.close(); } catch {} client.close(); }
});

test("le401 tardif d’un client fermé ne ferme pas une nouvelle session", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const old = make(async (path) => {
    if (identityRoute(path)) return Response.json(identity());
    entered(); await new Promise((resolve) => { release = resolve; }); return errorResponse(401, "credential_invalid", "authenticate");
  });
  const fresh = make(async (path) => Response.json(identityRoute(path) ? identity() : page("context-b", [resource("session-b")])));
  try {
    await old.client.identity(); old.client.selectContext("context-a");
    const pending = old.client.resources().catch((error) => error); await started; old.client.close();
    await fresh.client.identity(); fresh.client.selectContext("context-b"); release();
    assert.ok(await pending instanceof HumanSuperseded);
    assert.deepEqual(old.notifications, []); assert.deepEqual(fresh.notifications, []);
    assert.equal((await fresh.client.resources()).context_id, "context-b");
  } finally { old.client.close(); fresh.client.close(); }
});

test("migration_required conserve son action opérateur et le credential", async () => {
  const { client, notifications } = make(async (path) => identityRoute(path) ? Response.json(identity()) :
    errorResponse(503, "migration_required", "contact_operator"));
  try {
    await client.identity(); client.selectContext("context-a");
    await assert.rejects(client.resources(), (error) => error.code === "migration_required" && error.retryAction === "contact_operator");
    assert.equal(client.context.id, "context-a"); assert.deepEqual(notifications, []);
  } finally { client.close(); }
});

test("les refus de capacité conservent leur code et leur action sans relance ni retrait du contexte", async () => {
  for (const [status, code, action] of [
    [429, "capacity_exhausted", "close_unused_streams"],
    [503, "stream_admission_unavailable", "contact_operator"],
    [429, "audit_read_budget_exhausted", "contact_operator"],
  ]) {
    let refusedCalls = 0;
    const { client, notifications } = make(async (path) => {
      if (identityRoute(path)) return Response.json(identity());
      refusedCalls++; return errorResponse(status, code, action);
    });
    try {
      await client.identity(); client.selectContext("context-a");
      await assert.rejects(code === "audit_read_budget_exhausted" ? client.resources() :
        client.listen({ view: "metadata" }, new AbortController().signal,
          () => assert.fail("a refused stream must deliver no page"), () => {}),
      (error) => error instanceof HumanApiError && error.status === status && error.code === code && error.retryAction === action);
      assert.equal(refusedCalls, 1);
      assert.equal(client.context.id, "context-a");
      assert.deepEqual(notifications, []);
    } finally { client.close(); }
  }
});

test("SSE garde le dernier ID d’un frame complet après interruption d’un ID seul", async () => {
  const paths = []; let streams = 0;
  const { client } = make(async (path, options) => {
    if (identityRoute(path)) return Response.json(identity());
    paths.push(path); assert.equal(options.headers["Last-Event-ID"], undefined);
    const body = streams++ === 0 ?
      `id: confirmed-opaque\nevent: human_audit\ndata: ${JSON.stringify(page("context-a", [], { view: "metadata" }))}\n\nid: unconfirmed-opaque\n` :
      `event: error\ndata: ${JSON.stringify({ error: { code: "context_revoked", message: "Contexte retiré", retry_action: "select_context" } })}\n\n`;
    return new Response(body, { headers: { "content-type": "text/event-stream" } });
  });
  try {
    await client.identity(); client.selectContext("context-a"); let batches = 0;
    await client.listen({ view: "metadata" }, new AbortController().signal, () => batches++, () => {});
    assert.equal(batches, 1); assert.equal(paths.length, 2);
    assert.equal(new URL(paths[1], "http://localhost").searchParams.get("cursor"), "confirmed-opaque");
  } finally { client.close(); }
});
