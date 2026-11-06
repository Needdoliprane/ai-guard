import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiClient, ApiError, eventUrl } from "../src/api.ts";

test("l’export reprend tous les filtres actifs sans reprendre la pagination", async () => {
  const filters = {
    q: "printf données",
    agent_id: "agent / 1",
    sandbox_id: "sandbox-2",
    project_id: "projet-3",
    kind: "command.completed",
    decision: "allow",
    from: "2026-10-29T11:00:00.000Z",
    to: "2026-10-29T12:00:00.000Z",
    before: 90,
    after: 10,
    limit: 1,
  };
  let path;
  const client = new ApiClient("secret", async (url) => {
    path = url;
    return new Response("{}");
  });
  await client.export(filters);
  const query = new URL(path, "http://localhost").searchParams;
  for (const name of [
    "q",
    "agent_id",
    "sandbox_id",
    "project_id",
    "kind",
    "decision",
    "from",
    "to",
  ])
    assert.equal(query.get(name), filters[name]);
  for (const name of ["before", "after", "limit"])
    assert.equal(query.has(name), false);
  assert.equal(query.get("format"), "jsonl");
  client.close();
});

test("la période RFC3339 reste identique dans la pagination et l’export", async () => {
  const period = {
    from: "2026-10-29T11:15:00.000Z",
    to: "2026-10-29T12:15:00.000Z",
  };
  const first = new URL(eventUrl(period), "http://localhost");
  const previous = new URL(eventUrl(period, 42), "http://localhost");
  assert.equal(first.searchParams.get("from"), period.from);
  assert.equal(first.searchParams.get("to"), period.to);
  assert.equal(previous.searchParams.get("from"), period.from);
  assert.equal(previous.searchParams.get("to"), period.to);
  assert.equal(previous.searchParams.get("before"), "42");
  let exportPath;
  const client = new ApiClient("secret", async (path) => {
    exportPath = path;
    return new Response("{}");
  });
  await client.export(period);
  const exported = new URL(exportPath, "http://localhost");
  assert.equal(exported.searchParams.get("from"), period.from);
  assert.equal(exported.searchParams.get("to"), period.to);
  assert.equal(exported.searchParams.get("format"), "jsonl");
  client.close();
});

test("fetch conserve son appel libre, requis par la fonction native du navigateur", async () => {
  const client = new ApiClient("secret", async function () {
    assert.equal(
      this,
      undefined,
      "Le client API ne devient pas le récepteur de fetch",
    );
    return Response.json({ counts: {} });
  });
  await client.get("/api/overview");
  client.close();
});

test("la lecture transmet le jeton en en-tête seulement et interdit les redirections", async () => {
  const requests = [];
  const client = new ApiClient("jeton-lecture", async (url, options) => {
    requests.push({ url, options });
    return Response.json({ counts: {} });
  });
  await client.get("/api/overview");
  assert.equal(requests.length, 1, "Une requête authentifiée est envoyée");
  assert.equal(requests[0].url, "/api/overview");
  assert.equal(
    requests[0].options.headers.Authorization,
    "Bearer jeton-lecture",
  );
  assert.equal(requests[0].options.credentials, "omit");
  assert.equal(requests[0].options.redirect, "error");
  assert.equal(requests[0].options.cache, "no-store");
  client.close();
  await assert.rejects(client.get("/api/overview"), /fermée/i);
});

test("les routes de lecture sont bornées à cette origine et l’export est authentifié", async () => {
  const urls = [];
  const client = new ApiClient("secret", async (url, options) => {
    urls.push({ url, auth: options.headers.Authorization });
    return new Response('{"seq":1}\n', {
      headers: { "content-type": "application/x-ndjson" },
    });
  });
  await assert.rejects(
    client.get("https://exemple.invalid/api/events"),
    /lecture/i,
  );
  await assert.rejects(client.get("/api/decide"), /lecture/i);
  const blob = await client.export();
  assert.equal(await blob.text(), '{"seq":1}\n');
  assert.deepEqual(urls, [
    { url: "/api/export?format=jsonl", auth: "Bearer secret" },
  ]);
  client.close();
});

test("les vues workers lisent les contrats v1 sans ouvrir les mutations ou un autre tenant", async () => {
  const seen = [];
  const client = new ApiClient("viewer", async (path, options) => {
    seen.push({ path, method: options.method, auth: options.headers.Authorization });
    return Response.json({ items: [], next_cursor: null });
  });
  const id = "00000000-0000-4000-8000-000000000001";
  for (const path of [
    "/api/v1/federation/workers?limit=50",
    "/api/v1/workers?limit=50",
    `/api/v1/workers?limit=50&cursor=${id}`,
    `/api/v1/workers/${id}`,
    "/api/v1/quota",
    "/api/v1/usage?limit=50",
  ]) await client.get(path);
  assert.equal(seen.length, 6);
  assert.ok(seen.every((request) => request.method === "GET" && request.auth === "Bearer viewer"));
  for (const path of [
    "/api/v1/workers/admit",
    `/api/v1/workers/${id}/stop`,
    "/api/v1/quota?tenant=autre",
    "/api/v1/workers?tenant_id=autre",
    "/api/v1/usage?cursor=../secrets",
    "/api/v1/workers?limit=100000",
    "/api/v1/workers#secret",
  ]) await assert.rejects(client.get(path), /lecture/i);
  assert.equal(seen.length, 6, "Aucun appel hors des lectures bornées");
  client.close();
});

test("l’identité de lecture vient de la route authentifiée sans sélecteur de client", async () => {
  const seen = [];
  const client = new ApiClient("viewer-a", async (path, options) => {
    seen.push({ path, auth: options.headers.Authorization, method: options.method });
    return Response.json({ tenant_id: "tenant-a", role: "viewer" });
  });
  assert.deepEqual(await client.get("/api/v1/identity"), { tenant_id: "tenant-a", role: "viewer" });
  for (const path of ["/api/v1/identity?tenant_id=tenant-b", "/api/v1/identity#tenant-b", "/api/v1/identity/"])
    await assert.rejects(client.get(path), /lecture/i);
  assert.deepEqual(seen, [{ path: "/api/v1/identity", auth: "Bearer viewer-a", method: "GET" }]);
  client.close();
});

test("une session de lecture exige une identité viewer complète et conserve seulement son périmètre", async () => {
  const valid = new ApiClient("viewer", async () => Response.json({ tenant_id: "tenant-A_1", role: "viewer", extra: "hors du modèle" }));
  assert.deepEqual(await valid.identity(), { tenant_id: "tenant-A_1", role: "viewer" });
  valid.close();
  for (const identity of [null, [], {}, { tenant_id: "tenant-a", role: "runner" },
    { tenant_id: "tenant-a", role: "worker" }, { tenant_id: "", role: "viewer" },
    { tenant_id: "a".repeat(129), role: "viewer" }, { tenant_id: "../tenant-b", role: "viewer" }]) {
    const client = new ApiClient("viewer", async () => Response.json(identity));
    await assert.rejects(client.identity(), /Identité|Jeton refusé/);
    client.close();
  }
});

test("un refus de lecture termine la session même si son appelant ignore la réponse", async () => {
  for (const status of [401, 403]) {
    let expired = 0;
    const client = new ApiClient("viewer", async () => new Response(null, { status }), () => expired++);
    await assert.rejects(client.get("/api/agents"), (error) => error instanceof ApiError && error.status === status);
    assert.equal(expired, 1, `Le refus ${status} signale la fin de la session`);
    await assert.rejects(client.get("/api/overview"), /fermée/i,
      "Aucune nouvelle lecture ne part avec le credential refusé");
    assert.equal(expired, 1, "La fin de session n’est signalée qu’une fois");
  }
});

test("un refus tardif d’une session fermée n’invalide pas la session suivante", async () => {
  let release;
  let expired = 0;
  const client = new ApiClient("viewer-a", () => new Promise((resolve) => { release = resolve; }), () => expired++);
  const pending = client.get("/api/agents");
  client.close();
  release(new Response(null, { status: 403 }));
  await assert.rejects(pending, (error) => error instanceof ApiError && error.status === 403);
  assert.equal(expired, 0, "La continuation A ne signale plus de fin de session après fermeture");
});

test("un refus SSE utilise une seule notification centrale ou historique", async () => {
  for (const central of [true, false]) {
    const signals = [];
    const client = new ApiClient("viewer", async () => new Response(null, { status: 403 }),
      central ? () => signals.push("central") : undefined);
    await client.listen(0, () => assert.fail("Un flux refusé ne fournit aucun événement"), () => {},
      () => signals.push("historique"));
    assert.deepEqual(signals, central ? ["central"] : ["historique"],
      "Le rappel central est prioritaire et le rappel historique reste disponible en son absence");
    await assert.rejects(client.get("/api/overview"), /fermée/i);
  }
});
