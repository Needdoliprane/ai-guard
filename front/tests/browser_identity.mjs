// Fixtures de frontière d’identité exclusivement : aucun code de test dans le bundle.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require("playwright")); }
catch { ({ chromium } = require(join(process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES, "playwright"))); }
const root = fileURLToPath(new URL("..", import.meta.url));
const aId = "aaaaaaaa-0000-4000-8000-000000000001";
const bId = "bbbbbbbb-0000-4000-8000-000000000001";
const worker = (tenant) => {
  const id = tenant === "a" ? aId : bId;
  return {
    worker_id: id, state: "in_doubt", quota_held: true, revision: 1,
    launch: { version: 1, session_id: id, profile: "strict", runtime_kind: "docker",
      provider: `ONLY_${tenant.toUpperCase()}_PROVIDER`, cpu_millis: 1000, memory_bytes: 1024,
      pids_limit: 64, timeout_seconds: 60, network: "none" },
    attempt: { attempt_id: id, stable_name: `ONLY_${tenant.toUpperCase()}_RUNTIME`, container_id: null, volume_name: "volume" },
    last_observation: null, observed_started_at: null, allocation_inconsistent: false,
    unknown_reason: null, cleanup_removed: null, cleanup_error: null,
  };
};
const allocation = (tenant) => {
  const id = tenant === "a" ? aId : bId;
  return { worker_id: id, attempt_id: id, terminal_observation_id: id,
    started_at: "2026-10-29T12:00:00Z", finished_at: "2026-10-29T12:00:01Z",
    duration_ns: "1000000000", cpu_millicore_nanoseconds: "1000000000000", memory_byte_nanoseconds: "1024000000000",
    method_version: 1, billable: false,
    source: { authority_id: `ONLY_${tenant.toUpperCase()}_AUTHORITY`, runtime_kind: "docker", container_id: "container", spec_hash: "a".repeat(64), provenance: "observed" } };
};
let bOutage = true;
let aDenied = false;
let summaryDeniedStatus = null;
let detailDeniedStatus = null;
let identityMode = "normal";
const streams = new Map();
const requests = [];
const json = (response, value) => {
  response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
  response.end(JSON.stringify(value));
};
const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  if (url.pathname.startsWith("/api/")) {
    const tenant = request.headers.authorization === "Bearer fixture-viewer-a" ? "a" :
      request.headers.authorization === "Bearer fixture-viewer-b" ? "b" : null;
    requests.push({ path: request.url, tenant, method: request.method });
    if (!tenant) { response.writeHead(401); response.end(); return; }
    if (url.pathname === "/api/v1/identity") {
      if (identityMode === "outage") { response.writeHead(503); response.end(); return; }
      return json(response, identityMode === "invalid" ? { tenant_id: "", role: "viewer" } :
        { tenant_id: identityMode === "long" ? "b".repeat(128) : `tenant-${tenant}`, role: identityMode === "runner" ? "runner" : "viewer" });
    }
    if (url.pathname === "/api/stream") {
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
      response.write(": ready\n\n");
      streams.set(tenant, response);
      request.on("close", () => { if (streams.get(tenant) === response) streams.delete(tenant); });
      return;
    }
    if (tenant === "a" && summaryDeniedStatus && ["/api/overview", "/api/agents"].includes(url.pathname)) {
      response.writeHead(url.pathname === "/api/overview" ? 500 : summaryDeniedStatus);
      response.end(); return;
    }
    if (tenant === "a" && detailDeniedStatus && url.pathname === `/api/v1/workers/${aId}`) {
      response.writeHead(detailDeniedStatus); response.end(); return;
    }
    if ((tenant === "b" && bOutage) || (tenant === "a" && aDenied)) {
      response.writeHead(tenant === "a" ? 403 : 500); response.end(); return;
    }
    const mark = `ONLY_${tenant.toUpperCase()}_`;
    if (url.pathname === "/api/overview") return json(response, {
      counts: { agents: tenant === "a" ? 741 : 1, commands: tenant === "a" ? 742 : 2 },
      health: { collector: "ok", engine: "unknown" }, resources: { cpu_percent: null, memory_bytes: null },
      timestamp: "2026-10-29T12:00:00Z",
    });
    if (url.pathname === "/api/agents") return json(response, { items: [{ id: `${mark}AGENT`, name: `${mark}AGENT`, status: "running" }] });
    if (url.pathname === "/api/sandboxes") return json(response, { items: [{ id: `${mark}SANDBOX`, engine: "docker", image: `${mark}IMAGE`, network: "none" }] });
    if (url.pathname === "/api/integrity") return json(response, { valid: true, event_count: 14, head_hash: `${mark}HASH` });
    if (url.pathname === "/api/events") return json(response, {
      items: [{ seq: url.searchParams.has("before") ? 13 : 14, id: `${mark}EVENT`, timestamp: "2026-10-29T12:00:00Z",
        kind: "command.completed", agent_id: `${mark}AGENT`, command: { argv: ["printf", `${mark}COMMAND`] } }],
      next_cursor: url.searchParams.has("before") ? null : 14,
    });
    if (url.pathname === "/api/v1/workers") return json(response, { items: [worker(tenant)], next_cursor: url.searchParams.has("cursor") ? null : worker(tenant).worker_id });
    if (url.pathname === `/api/v1/workers/${worker(tenant).worker_id}`) return json(response, worker(tenant));
    if (url.pathname === "/api/v1/quota") return json(response, {
      policy: { version: 1, max_active_workers: 3, max_cpu_millis: 3000, max_memory_bytes: 4096, max_timeout_seconds: 60 },
      held_workers: 1, held_cpu_millis: 1000, held_memory_bytes: 1024,
    });
    if (url.pathname === "/api/v1/usage") return json(response, { items: [allocation(tenant)], next_cursor: url.searchParams.has("cursor") ? null : worker(tenant).worker_id });
    if (url.pathname === "/api/export") {
      response.writeHead(200, { "content-type": "application/x-ndjson" });
      response.end(JSON.stringify({ id: `${mark}EXPORT` }) + "\n"); return;
    }
    response.writeHead(404); response.end(); return;
  }
  if (url.pathname === "/healthz") return json(response, { status: "ok" });
  try {
    const path = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
    if (path.includes("..")) throw new Error("Invalid path");
    const body = await readFile(join(root, "dist", path));
    response.writeHead(200, { "content-type": { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" }[extname(path)] || "application/octet-stream" });
    response.end(body);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined, args: ["--no-sandbox"] });
const context = await browser.newContext({ viewport: { width: 1440, height: 1050 } });
const page = await context.newPage();
page.setDefaultTimeout(5000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
let downloads = 0;
page.on("download", () => downloads++);
// Le transport réel termine avant la barrière. Sa continuation est retenue même
// après abort : les gardes de l’application doivent aussi refuser ce résultat.
await page.addInitScript(() => {
  const nativeFetch = window.fetch;
  window.__identityHeld = [];
  window.__identityHold = false;
  window.__identityRelease = () => {
    const held = window.__identityHeld.splice(0);
    window.__identityHold = false;
    held.forEach((item) => item.release());
  };
  const hold = (path) => new Promise((release) => window.__identityHeld.push({ path, release }));
  window.fetch = async (...args) => {
    const response = await nativeFetch(...args);
    const a = args[1]?.headers?.Authorization === "Bearer fixture-viewer-a";
    const path = String(args[0]);
    if (a && path === "/api/stream" && response.body) {
      const reader = response.body.getReader();
      return new Response(new ReadableStream({ async pull(controller) {
        try {
          const chunk = await reader.read();
          if (chunk.done) { controller.close(); return; }
          if (window.__identityHold) await hold(path);
          controller.enqueue(chunk.value);
        } catch (error) { controller.error(error); }
      } }), { status: response.status, headers: response.headers });
    }
    if (a && window.__identityHold && path !== "/api/v1/identity") {
      const body = await response.text();
      await hold(path);
      return new Response(body, { status: response.status, headers: response.headers });
    }
    return response;
  };
});
const login = async (tenant) => {
  await page.getByLabel("Jeton de lecture").fill(`fixture-viewer-${tenant}`);
  await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
};
const navigate = async (name) => page.getByRole("link", { name, exact: true }).click();
const assertNoA = async () => {
  const contents = await page.locator("body").textContent();
  assert.ok(!contents.includes("ONLY_A_") && !contents.includes(aId), "Aucune donnée A dans le DOM sous identité B");
  assert.ok(!(await page.locator(".metric-value").allTextContents()).some((value) => ["741", "742"].includes(value)),
    "Les comptes A ne survivent pas au changement d’identité");
  assert.equal(await page.locator("dialog[open]").count(), 0, "Aucun détail A ne se rouvre");
  await page.getByText("Client : tenant-b", { exact: true }).first().waitFor();
};
const waitHeld = async (fragment) => page.waitForFunction((path) => window.__identityHeld.some((item) => item.path.includes(path)), fragment);
try {
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  for (const failure of ["outage", "invalid", "runner"]) {
    identityMode = failure;
    const previousDataRequests = requests.filter((request) => request.path !== "/api/v1/identity").length;
    await login("a");
    await page.locator(".login-error:not([hidden])").waitFor();
    assert.equal(await page.getByLabel("Jeton de lecture").inputValue(), "");
    assert.equal(await page.locator(".app-shell").count(), 0, "Une identité indisponible, invalide ou non-viewer ne crée pas de session");
    assert.equal(requests.filter((request) => request.path !== "/api/v1/identity").length, previousDataRequests,
      "Aucune lecture de données avant identité viewer validée");
  }
  identityMode = "normal";
  await login("a");
  await page.getByText("Client : tenant-a", { exact: true }).first().waitFor();
  await page.getByText("Flux connecté", { exact: true }).waitFor();
  await navigate("Agents"); await page.getByText("ONLY_A_AGENT", { exact: true }).first().waitFor();
  await navigate("Sandboxes"); await page.getByText("ONLY_A_SANDBOX", { exact: true }).first().waitFor();
  // Une panne de la première lecture ne masque jamais le refus d’une autre.
  // Un détail déjà ouvert appartient lui aussi à la session révoquée.
  for (const status of [401, 403]) {
    await navigate("Agents");
    await page.getByRole("button", { name: "Détails", exact: true }).first().click();
    await page.locator("dialog[open]").waitFor();
    summaryDeniedStatus = status;
    streams.get("a").write(`id: ${status + 100}\nevent: audit\ndata: ${JSON.stringify({ seq: status + 100,
      id: `ONLY_A_REVOCATION_${status}`, timestamp: "2026-10-29T12:00:00Z", kind: "command.completed" })}\n\n`);
    await page.getByLabel("Jeton de lecture").waitFor();
    assert.equal(await page.locator(".app-shell, dialog").count(), 0,
      `Le refus ${status} ferme la session et le détail malgré overview500`);
    assert.ok(!(await page.locator("body").textContent()).includes("ONLY_A_"), "Aucune donnée révoquée ne reste affichée");
    await page.getByText("Votre session a expiré ou le jeton a été refusé.", { exact: true }).waitFor();
    summaryDeniedStatus = null;
    await login("a"); await page.getByText("Flux connecté", { exact: true }).waitFor();
  }
  // La fermeture du dialogue abandonne son affichage, pas le refus du serveur.
  detailDeniedStatus = 403;
  await navigate("Workers"); await page.getByText(aId, { exact: true }).first().waitFor();
  await page.evaluate(() => { window.__identityHold = true; });
  await page.getByRole("button", { name: "Détails", exact: true }).click();
  await waitHeld(`/workers/${aId}`);
  await page.keyboard.press("Escape");
  await page.evaluate(() => window.__identityRelease());
  await page.getByLabel("Jeton de lecture").waitFor();
  assert.equal(await page.locator(".app-shell, dialog").count(), 0, "Le refus tardif du détail fermé invalide encore sa session");
  detailDeniedStatus = null;
  await login("a"); await page.getByText("Flux connecté", { exact: true }).waitFor();
  for (const name of ["Workers", "Quotas", "Allocations"]) {
    await navigate(name); await page.getByText("Lecture réussie", { exact: true }).waitFor();
  }
  await page.evaluate(() => { window.__identityHold = true; });
  await navigate("Actions");
  await page.getByRole("button", { name: "Charger les précédents", exact: true }).click();
  await waitHeld("before=14");
  await navigate("Vue d’ensemble");
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await waitHeld("/api/overview"); await waitHeld("/api/agents"); await waitHeld("/api/sandboxes"); await waitHeld("/api/integrity");
  await page.getByRole("button", { name: "Exporter JSONL", exact: true }).click(); await waitHeld("/api/export");
  await navigate("Workers");
  await page.getByRole("button", { name: "Page suivante", exact: true }).click(); await waitHeld("workers?limit=50&cursor=");
  await page.getByRole("button", { name: "Détails", exact: true }).click(); await waitHeld(`/workers/${aId}`);
  await page.keyboard.press("Escape");
  await navigate("Quotas"); await page.getByRole("button", { name: "Actualiser", exact: true }).click(); await waitHeld("/api/v1/quota");
  await navigate("Allocations"); await page.getByRole("button", { name: "Page suivante", exact: true }).click(); await waitHeld("usage?limit=50&cursor=");
  streams.get("a").write(`id: 15\nevent: audit\ndata: ${JSON.stringify({ seq: 15, id: "ONLY_A_LATE_EVENT", timestamp: "2026-10-29T12:00:00Z", kind: "command.completed", command: { argv: ["ONLY_A_LATE_COMMAND"] } })}\n\n`);
  await waitHeld("/api/stream");
  const heldPaths = await page.evaluate(() => window.__identityHeld.map((item) => item.path));
  await page.getByRole("button", { name: "Se déconnecter", exact: true }).click();
  await login("b");
  await page.getByText("Client : tenant-b", { exact: true }).first().waitFor();
  await page.getByText("Journal indisponible", { exact: true }).waitFor();
  await assertNoA();
  await page.evaluate(() => window.__identityRelease());
  for (const name of ["Vue d’ensemble", "Agents", "Sandboxes", "Workers", "Quotas", "Allocations", "Actions"]) {
    await navigate(name);
    if (["Workers", "Quotas", "Allocations"].includes(name)) await page.getByText("Données indisponibles", { exact: true }).waitFor();
    if (name === "Actions") await page.getByText("Journal indisponible", { exact: true }).waitFor();
    await assertNoA();
  }
  assert.equal(downloads, 0, "L’export A tardif ne télécharge rien sous B");
  assert.equal(await page.getByText("Aucun événement correspondant", { exact: true }).count(), 0, "B500 ne devient pas une absence d’activité");
  bOutage = false;
  await navigate("Workers");
  await page.getByText(bId, { exact: true }).first().waitFor();
  bOutage = true;
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await page.getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
  assert.ok((await page.locator(".control-view").textContent()).includes(bId), "Seule la dernière lecture du même client est conservée");
  await assertNoA();
  // Un ancien refus A ne doit pas déconnecter une nouvelle session B valide.
  await page.getByRole("button", { name: "Se déconnecter", exact: true }).click();
  await login("a"); await page.getByText("Flux connecté", { exact: true }).waitFor();
  aDenied = true;
  await page.evaluate(() => { window.__identityHold = true; });
  await page.getByRole("button", { name: "Actualiser", exact: true }).click(); await waitHeld("/api/overview");
  await navigate("Workers"); await waitHeld("/api/v1/workers");
  await page.getByRole("button", { name: "Se déconnecter", exact: true }).click();
  bOutage = false;
  await login("b"); await page.getByText("Flux connecté", { exact: true }).waitFor();
  await page.evaluate(() => window.__identityRelease());
  await navigate("Workers"); await page.getByText(bId, { exact: true }).first().waitFor(); await assertNoA();
  await page.getByRole("button", { name: "Se déconnecter", exact: true }).click();
  identityMode = "long";
  await login("b"); await page.getByText(`Client : ${"b".repeat(128)}`, { exact: true }).first().waitFor();
  await mkdir(join(root, "tests", "artifacts"), { recursive: true });
  for (const size of [{ width: 320, height: 568 }, { width: 390, height: 844 }, { width: 844, height: 390 }]) {
    await page.setViewportSize(size);
    await navigate("Workers");
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true,
      `Un identifiant client valide de 128 caractères ne déborde pas à ${size.width}px`);
    const commandPositions = await page.locator(".view-stepper").boundingBox();
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    assert.deepEqual(await page.locator(".view-stepper").boundingBox(), commandPositions,
      "L’identité longue ne déplace pas les commandes fixes");
    await page.getByRole("button", { name: "Suivant : Quotas", exact: true }).click();
    assert.equal(await page.evaluate(() => document.querySelector(".page h1").getBoundingClientRect().top >=
      document.querySelector(".control-dock").getBoundingClientRect().bottom), true, "Le titre reste visible après navigation");
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await page.screenshot({ path: join(root, "tests", "artifacts", `editorial-long-identity-${size.width}.png`) });
  }
  assert.deepEqual(errors, [], "Aucune exception navigateur");
  assert.ok(requests.every((request) => request.method === "GET" && !request.path.includes("tenant_id=")), "Les credentials seuls déterminent le client");
  process.stdout.write(`Contrôle identité réussi : identité viewer validée avant données, indisponibilité/invalide/runner refusés, refus401/403 malgré overview500, refus403 d’un détail fermé, A→B avec B500, ${heldPaths.length} réponses/flux A tardifs ignorés, pagination/détail/export, refus A403 tardif, péremption limitée au même client.\n`);
} catch (error) {
  process.stderr.write(JSON.stringify({ errors, requests, page: await page.locator("body").innerText() }, null, 2) + "\n");
  throw error;
} finally {
  await browser.close(); server.closeAllConnections(); await new Promise((resolve) => server.close(resolve));
}
