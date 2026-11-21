// Fixtures de relations uniquement ; aucun serveur ou agent simulé dans le produit.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { federationPage } from "./federation-fixture.mjs";

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require("playwright")); }
catch { ({ chromium } = require(join(process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES, "playwright"))); }
const root = fileURLToPath(new URL("..", import.meta.url));
const scenario = process.env.AI_GUARD_UNIFIED_SCENARIO || "full";
const ids = ["00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002", "00000000-0000-4000-8000-000000000003"];
const session = "10000000-0000-4000-8000-000000000001";
let partialProvenance = false;
function withProvenance(worker) {
  return {
    ...worker,
    spec_hash: "a".repeat(64),
    allocation_inconsistent: true,
    unknown_reason: "allocation history is inconsistent; explicit reconciliation required",
    launch: { ...worker.launch, authority_id: "fixture-launch-authority", image_id: partialProvenance ? undefined : `sha256:${"b".repeat(64)}`,
      credential_names: ["FIXTURE_EXCLUDED_FIELD"] },
    attempt: { ...worker.attempt, container_id: "c".repeat(64) },
    last_observation: {
      observed_at: "2026-11-06T01:02:00.000Z", status: "exited", running: false,
      started_at: null, finished_at: null, exit_code: 0, oom_killed: false, restart_count: 0,
      inspection: { authority_id: "fixture-inspection-authority", runtime_kind: "docker",
        container_id: partialProvenance ? null : "d".repeat(64),
        image_id: partialProvenance ? null : `sha256:${"e".repeat(64)}`,
        spec_hash: partialProvenance ? undefined : "f".repeat(64) },
    },
  };
}
const worker = (tenant, related = true) => ({
  worker_id: tenant === "b" ? ids[2] : related ? ids[1] : ids[0],
  state: "in_doubt", quota_held: true, revision: 1,
  launch: { version: 1, session_id: related ? session : ids[0], profile: "strict", runtime_kind: "docker",
    provider: "generic", cpu_millis: 1000, memory_bytes: 1024, pids_limit: 64, timeout_seconds: 60, network: "none" },
  attempt: { attempt_id: ids[0], stable_name: `runtime-${tenant}`, container_id: null, volume_name: "volume" },
  last_observation: null, observed_started_at: null, allocation_inconsistent: false,
  unknown_reason: "Inspection indisponible", cleanup_removed: null, cleanup_error: null,
});
let workerMode = "outage";
let entityMode = "data";
let agentName = "Agent A";
const requests = [];
const streams = new Set();
const json = (response, value) => {
  if (response.federationRead) value = federationPage(value);
  response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
  response.end(JSON.stringify(value));
};
const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  response.federationRead = url.pathname === "/api/v1/federation/workers";
  if (url.pathname.startsWith("/api/")) {
    const tenant = request.headers.authorization === "Bearer fixture-a" ? "a" :
      request.headers.authorization === "Bearer fixture-b" ? "b" : null;
    requests.push({ path: request.url, method: request.method, tenant });
    if (!tenant) { response.writeHead(401); response.end(); return; }
    if (url.pathname === "/api/v1/identity") return json(response, { tenant_id: tenant, role: "viewer" });
    if (url.pathname === "/api/stream") {
      response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": ready\n\n");
      streams.add(response); request.on("close", () => streams.delete(response)); return;
    }
    if (["/api/agents", "/api/sandboxes"].includes(url.pathname) && entityMode === "outage" && tenant === "a") {
      response.writeHead(503); response.end(); return;
    }
    if (url.pathname === "/api/agents") return json(response, entityMode === "invalid" && tenant === "a" ? {} : {
      items: [{ id: `agent-${tenant}`, name: tenant === "a" ? agentName : "Agent B", provenance: "observed",
        ...(entityMode === "unidentified" ? {} : { session_id: session, sandbox_id: `sandbox-${tenant}`,
          field_provenance: { session_id: "declared", sandbox_id: "declared" } }) }],
    });
    if (url.pathname === "/api/sandboxes") return json(response, { items: entityMode === "missing" ? [] : [{ id: `sandbox-${tenant}`,
      session_id: session, engine: "docker", network: "none" }] });
    if (url.pathname === "/api/overview") return json(response, { counts: { agents: 1 }, health: { collector: "ok" } });
    if (url.pathname === "/api/integrity") return json(response, { valid: true });
    if (url.pathname === "/api/events") return json(response, { items: [], next_cursor: null });
    if (url.pathname === "/api/export" && scenario === "summary-notice") {
      response.writeHead(502); response.end(); return;
    }
    if (url.pathname === "/api/v1/federation/workers") {
      if (workerMode === "forbidden") { response.writeHead(403); response.end(); return; }
      if (workerMode === "outage" && tenant === "a") { response.writeHead(503); response.end(); return; }
      const related = tenant === "b" || url.searchParams.has("cursor") || ["single", "updated"].includes(workerMode);
      const base = worker(tenant, related);
      const item = scenario === "worker-provenance" ? withProvenance(base) : base;
      if (workerMode === "updated") item.worker_id = ids[2];
      // The first page intentionally has a matching runtime name but an unrelated session.
      if (!related) item.attempt.stable_name = "Agent A";
      return json(response, { items: [item], next_cursor: related ? null : ids[0] });
    }
    if (url.pathname === `/api/v1/workers/${worker(tenant).worker_id}`) {
      const item = worker(tenant);
      return json(response, scenario === "worker-provenance" ? withProvenance(item) : item);
    }
    if (url.pathname === "/api/v1/usage") return json(response, { items: [{
      worker_id: worker(tenant).worker_id, attempt_id: ids[0], terminal_observation_id: ids[0],
      started_at: "2026-11-01T12:00:00Z", finished_at: "2026-11-01T12:00:01Z", duration_ns: "1000000000",
      cpu_millicore_nanoseconds: "1000000000000", memory_byte_nanoseconds: "1024000000000", method_version: 1, billable: false,
      source: { authority_id: "fixture-authority", runtime_kind: "docker", container_id: "runtime-a", spec_hash: "a".repeat(64), provenance: "observed" },
    }], next_cursor: null });
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
await page.addInitScript(() => {
  const nativeFetch = window.fetch;
  window.__unifiedHold = "";
  window.__unifiedHeld = [];
  window.__unifiedRelease = () => { window.__unifiedHeld.splice(0).forEach((release) => release()); };
  window.fetch = async (...args) => {
    const response = await nativeFetch(...args);
    if (window.__unifiedHold && String(args[0]).startsWith(window.__unifiedHold)) {
      window.__unifiedHold = "";
      const body = await response.text();
      await new Promise((release) => window.__unifiedHeld.push(release));
      return new Response(body, { status: response.status, headers: response.headers });
    }
    return response;
  };
});
const login = async (tenant) => {
  await page.getByLabel("Jeton de lecture").fill(`fixture-${tenant}`);
  await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
  await page.getByText(`Client : ${tenant}`, { exact: true }).first().waitFor();
  await page.getByText("Flux connecté", { exact: true }).waitFor();
};
const navigate = async (name) => page.getByRole("link", { name, exact: true }).click();
const modal = () => page.getByRole("dialog");
const links = () => modal().locator(".agent-worker-links");
const workerReads = () => requests.filter(({ path }) => path.startsWith("/api/v1/federation/workers?"));
const openAgent = async () => {
  await navigate("Agents");
  await page.getByRole("button", { name: "Détails", exact: true }).click();
};
const hold = async (path) => page.evaluate((value) => { window.__unifiedHold = value; }, path);
const held = async () => page.waitForFunction(() => window.__unifiedHeld.length > 0);
const release = async () => page.evaluate(async () => {
  window.__unifiedRelease();
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
});
const artifacts = process.env.AI_GUARD_UNIFIED_ARTIFACTS || join(root, "tests", "artifacts");
let eventSeq = 20;
const emit = (kind) => {
  const event = { seq: ++eventSeq, id: `regression-${eventSeq}`, kind, timestamp: "2026-11-01T12:00:00Z" };
  for (const stream of streams) stream.write(`id: ${event.seq}\nevent: audit\ndata: ${JSON.stringify(event)}\n\n`);
};
const restart = async () => {
  workerMode = "single"; entityMode = "data";
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await login("a");
};
const openUsageWorker = async () => {
  await navigate("Allocations");
  await page.locator(".control-view").getByRole("button", { name: "Worker", exact: true }).click();
  await modal().getByRole("button", { name: "sandbox-a", exact: true }).waitFor();
};
const source = () => modal().getByRole("region", { name: "Sandboxes correspondantes", exact: true });
const focusEvidence = async () => page.evaluate(() => {
  const active = document.activeElement;
  const modal = document.querySelector("dialog[open]");
  return { connected: !!active?.isConnected, inside: !!modal?.contains(active),
    usable: active instanceof HTMLElement && active.matches("button:not(:disabled), a[href], input:not(:disabled), [tabindex]") &&
      !active.closest("[hidden], [inert]") && active.getClientRects().length > 0,
    tag: active?.tagName, key: active?.getAttribute("data-focus-key") };
});
const runRegression = async () => {
  const findings = [];
  if (scenario === "worker-provenance") {
    await restart();
    await navigate("Workers");
    const workerDetails = page.locator(".control-view tbody tr").filter({ hasText: ids[1] })
      .getByRole("button", { name: "Détails", exact: true });
    for (const partial of [false, true]) {
      partialProvenance = partial;
      await workerDetails.click();
      const source = withProvenance(worker("a"));
      const inspection = source.last_observation.inspection;
      await modal().getByRole("heading", { name: "Dernière observation runtime", exact: true }).waitFor();
      for (const [label, expected] of [
        ["Autorité déclarée du lancement", source.launch.authority_id],
        ["Empreinte de l’image de lancement", source.launch.image_id],
        ["Empreinte du plan de lancement", source.spec_hash],
        ["Autorité de la dernière inspection enregistrée", inspection.authority_id],
        ["Runtime de l’inspection enregistrée", inspection.runtime_kind],
        ["Conteneur inspecté", inspection.container_id],
        ["Empreinte de l’image inspectée", inspection.image_id],
        ["Empreinte du plan liée à l’inspection", inspection.spec_hash],
      ]) {
        const value = modal().locator(".key-value")
          .filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
        assert.deepEqual(await value.allTextContents(), [expected ?? "Inconnu"], label);
      }
      assert.equal(await modal().getByText("FIXTURE_EXCLUDED_FIELD", { exact: false }).count(), 0);
      await page.keyboard.press("Escape");
    }
    assert.equal(requests.filter(({ path, tenant }) => path === `/api/v1/workers/${ids[1]}` && tenant === "a").length, 2);
  } else if (scenario === "agent-registry-reopen") {
    await restart();
    assert.equal(workerReads().length, 0, "Le détail Agent part d’un registre encore non lu");
    workerMode = "outage";
    await hold("/api/v1/federation/workers?");
    await openAgent();
    await held();
    await links().getByRole("status").getByText("Chargement…", { exact: true }).waitFor();
    assert.equal(await links().getByText(/Aucune correspondance/).count(), 0);
    await page.keyboard.press("Escape");
    workerMode = "single";
    await openAgent();
    const activeLink = links().getByRole("button", { name: ids[1], exact: true });
    try {
      await activeLink.waitFor();
      assert.equal(workerReads().length, 2, "Le nouveau détail lit le registre rétabli sans attendre le détail fermé");
    } finally {
      await release();
    }
    assert.equal(await activeLink.isVisible(), true, "Le 503 tardif n’efface pas le lien du nouveau détail");
    assert.equal(await links().getByRole("alert").count(), 0, "Le 503 tardif ne rend pas le nouveau détail périmé");
    await page.keyboard.press("Escape");
    await openAgent();
    await activeLink.waitFor();
    assert.equal(workerReads().length, 2, "Un détail suivant réutilise la page autorisée encore fraîche");
    workerMode = "forbidden";
    emit("control.worker.transition");
    await page.getByLabel("Jeton de lecture").waitFor();
    assert.equal(await page.locator(".app-shell, dialog").count(), 0, "Un refus courant retire le détail et les données");
    entityMode = "unidentified";
    await login("a");
    const beforeUnidentified = workerReads().length;
    await openAgent();
    await links().getByText("Identifiant de session ou de runtime inconnu.", { exact: true }).waitFor();
    assert.equal(await links().getByText(/Aucune correspondance/).count(), 0);
    assert.equal(workerReads().length, beforeUnidentified, "Sans identifiant de relation, aucun registre n’est nécessaire");
  } else if (scenario === "usage-links") {
    for (const destination of ["Agent A", "sandbox-a"]) {
      await restart(); await openUsageWorker();
      await modal().getByRole("button", { name: destination, exact: true }).click();
      await links().getByRole("button", { name: ids[1], exact: true }).waitFor();
      const before = requests.filter((item) => item.path.startsWith("/api/v1/federation/workers?")).length;
      workerMode = "updated";
      const reread = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/federation/workers", { timeout: 4000 });
      emit("control.worker.transition");
      try { await reread; await links().getByRole("button", { name: ids[2], exact: true }).waitFor(); }
      catch { findings.push({ destination, defect: "Le registre ouvert depuis Allocations n’a pas été relu ou son lien n’a pas changé",
        before, after: requests.filter((item) => item.path.startsWith("/api/v1/federation/workers?")).length,
        text: await links().innerText() }); }
    }
  } else if (["retry-focus", "removed-link-focus"].includes(scenario)) {
    for (const origin of ["agent", "worker"]) {
      await restart();
      if (origin === "agent") await openAgent(); else await openUsageWorker();
      await source().getByRole("button", { name: "sandbox-a", exact: true }).waitFor();
      if (scenario === "retry-focus") {
        entityMode = "outage"; emit("test.source.refresh");
        await source().getByText("Liens périmés · dernière lecture conservée", { exact: true }).waitFor();
        entityMode = "data"; await hold("/api/sandboxes");
        await source().getByRole("button", { name: "Réessayer la source du journal", exact: true }).focus();
        await page.keyboard.press("Enter"); await held();
        const during = await focusEvidence();
        if (!during.connected || !during.inside || !during.usable) findings.push({ origin, phase: "retry pending", focus: during });
        await release(); await source().getByText("Lecture des liens réussie", { exact: true }).waitFor();
        const after = await focusEvidence();
        if (!after.connected || !after.inside || !after.usable) findings.push({ origin, phase: "retry completed", focus: after });
      } else {
        await source().getByRole("button", { name: "sandbox-a", exact: true }).focus();
        entityMode = "missing"; emit("test.source.refresh");
        await source().getByText(/Aucune correspondance dans les données reçues/).waitFor();
        const after = await focusEvidence();
        if (!after.connected || !after.inside || !after.usable) findings.push({ origin, phase: "focused link removed", focus: after });
      }
    }
  } else if (scenario === "summary-notice") {
    await restart(); await navigate("Sandboxes");
    await page.getByRole("button", { name: "Voir les détails", exact: true }).click();
    const agentSource = () => modal().getByRole("region", { name: "Agents correspondants", exact: true });
    entityMode = "invalid"; emit("test.source.refresh");
    const contractError = "Contrat de liste d’identités incomplet ou incompatible.";
    await agentSource().getByText(contractError, { exact: true }).waitFor();
    await page.locator(".notice:not([hidden])").getByText(contractError, { exact: true }).waitFor();
    entityMode = "data";
    await agentSource().getByRole("button", { name: "Réessayer la source du journal", exact: true }).click();
    await agentSource().getByText("Lecture des liens réussie", { exact: true }).waitFor();
    const recoveredNotice = await page.locator(".notice").evaluate((node) => ({ hidden: node.hidden, text: node.textContent }));
    if (!recoveredNotice.hidden && recoveredNotice.text.includes(contractError))
      findings.push({ phase: "summary recovered", defect: "Le bandeau d’erreur du résumé persiste après le retry réussi", notice: recoveredNotice });
    // An independent operation can fail after the summary read starts but before it completes.
    await hold("/api/agents"); emit("test.source.refresh"); await held();
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Exporter JSONL", exact: true }).click();
    const exportError = "Le collecteur a répondu avec une erreur HTTP 502.";
    await page.locator(".notice:not([hidden])").getByText(exportError, { exact: true }).waitFor();
    await release();
    const independentNotice = await page.locator(".notice").evaluate((node) => ({ hidden: node.hidden, text: node.textContent }));
    if (independentNotice.hidden || !independentNotice.text.includes(exportError))
      findings.push({ phase: "independent export failure", defect: "Une lecture de résumé réussie a effacé une erreur indépendante", notice: independentNotice });
  } else throw new Error(`Scénario inconnu : ${scenario}`);
  assert.deepEqual(findings, [], `Régression ciblée ${scenario}`);
  assert.deepEqual(errors, []);
  process.stdout.write(`Régression ciblée ${scenario} PASS. Fixtures uniquement, aucun moteur qualifié.\n`);
};
try {
  if (scenario !== "full") {
    await runRegression();
  } else {
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await login("a");
  assert.equal(workerReads().length, 0);
  await openAgent();
  await links().getByText("Données indisponibles", { exact: true }).waitFor();
  assert.equal(await links().getByText(/Aucune correspondance/).count(), 0, "Une erreur initiale ne signifie pas absence de worker");
  workerMode = "data";
  await links().getByRole("button", { name: "Réessayer le registre" }).click();
  await links().getByText(/Aucune correspondance sur cette page/).waitFor();
  assert.ok((await links().innerText()).includes("lecture partielle"));
  assert.equal(await links().getByRole("button", { name: ids[0], exact: true }).count(), 0, "Un nom ressemblant ne crée aucun lien");
  await links().getByRole("button", { name: "Page suivante", exact: true }).click();
  await links().getByRole("button", { name: ids[1], exact: true }).waitFor();
  await links().getByText("Identifiant déclaré", { exact: true }).waitFor();
  assert.equal(await links().evaluate((node) => node.contains(document.activeElement)), true, "Le focus reste dans le détail après pagination");
  await links().getByRole("button", { name: ids[1], exact: true }).click();
  await modal().getByRole("button", { name: "sandbox-a", exact: true }).waitFor();
  await modal().getByRole("button", { name: "Agent A", exact: true }).waitFor();
  await modal().locator(".worker-related-resources").getByText("Déclaré", { exact: true }).waitFor();
  // A second successful read wins even when the first response is released later.
  agentName = "Agent ancien";
  await hold("/api/agents");
  await modal().getByRole("button", { name: "Actualiser les sources du journal", exact: true }).click();
  await held();
  agentName = "Agent récent";
  await modal().getByRole("button", { name: "Actualisation des sources…", exact: true }).click();
  await modal().getByRole("button", { name: "Agent récent", exact: true }).waitFor();
  await release();
  await page.waitForFunction(() => !document.body.textContent.includes("Agent ancien"));
  assert.equal(await modal().getByRole("button", { name: "Agent récent", exact: true }).count(), 1);
  entityMode = "outage";
  await modal().getByRole("button", { name: "Actualiser les sources du journal", exact: true }).click();
  await modal().getByText("Liens périmés · dernière lecture conservée", { exact: true }).first().waitFor();
  assert.equal(await modal().getByRole("button", { name: "Agent récent", exact: true }).count(), 1);
  entityMode = "invalid";
  await modal().getByRole("button", { name: "Réessayer la source du journal", exact: true }).first().click();
  await modal().getByText("Contrat de liste d’identités incomplet ou incompatible.", { exact: true }).waitFor();
  assert.equal(await modal().getByText(/Aucune correspondance dans les données/).count(), 0, "Un DTO invalide garde les données périmées");
  entityMode = "data";
  await modal().getByRole("button", { name: "Actualiser les sources du journal", exact: true }).click();
  await modal().getByText("Lecture des liens réussie", { exact: true }).first().waitFor();
  await modal().getByRole("button", { name: "sandbox-a", exact: true }).click();
  await modal().getByRole("button", { name: "Agent récent", exact: true }).waitFor();
  await links().getByRole("button", { name: ids[1], exact: true }).waitFor();
  await mkdir(artifacts, { recursive: true });
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: width === 1440 ? 1050 : 844 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((value) => { document.documentElement.dataset.theme = value; }, theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `Aucun débordement ${width}px ${theme}`);
      assert.equal(await modal().evaluate((node) => node.scrollWidth <= node.clientWidth), true, `Dialogue sans débordement ${width}px ${theme}`);
      await page.screenshot({ path: join(artifacts, `unified-sandbox-${width}-${theme}.png`) });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  workerMode = "outage";
  for (const stream of streams) stream.write(`id: 10\nevent: audit\ndata: ${JSON.stringify({ seq: 10, id: "worker-change", kind: "control.worker.transition", timestamp: "2026-11-01T12:00:00Z" })}\n\n`);
  await links().getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
  assert.equal(await links().getByRole("button", { name: ids[1], exact: true }).count(), 1);
  workerMode = "data";
  await hold("/api/v1/federation/workers?");
  await links().getByRole("button", { name: "Réessayer le registre" }).click();
  await held();
  await page.keyboard.press("Escape");
  await release();
  assert.equal(await page.locator("dialog[open]").count(), 0, "La lecture tardive ne rouvre pas un détail fermé");
  assert.equal(await page.locator("#main-content h1").evaluate((node) => node === document.activeElement), true,
    "Échap après navigation entre détails rejoint le titre de la vue lorsque le déclencheur a disparu");
  // A transport can finish before logout while its JavaScript continuation arrives after login B.
  await openAgent();
  await links().getByRole("status").getByText("Lecture réussie", { exact: true }).waitFor();
  await hold("/api/v1/federation/workers?");
  await links().getByRole("button", { name: "Première page", exact: true }).click();
  await held();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Se déconnecter", exact: true }).click();
  await login("b");
  await release();
  await openAgent();
  await links().getByRole("button", { name: ids[2], exact: true }).waitFor();
  assert.ok(!(await page.locator("body").innerText()).includes("Agent récent"));
  assert.equal(await links().getByRole("button", { name: ids[1], exact: true }).count(), 0, "Aucun worker A après changement d’identité");
  assert.ok(requests.every((item) => item.method === "GET" && !item.path.includes("tenant_id=")));
  assert.deepEqual(errors, []);
  process.stdout.write("Supervision liée : chargement initial sans visite Workers, panne/retry, pagination bornée, correspondances/provenances exactes, liens bidirectionnels, sources périmées/invalides, réponse de résumé dépassée, détail fermé et changement A→B ; captures 1440/390/320 clair/sombre. Fixtures uniquement, aucun moteur qualifié.\n");
  }
} catch (error) {
  process.stderr.write(JSON.stringify({ errors, requests, page: await page.locator("body").innerText() }, null, 2) + "\n");
  throw error;
} finally {
  await browser.close(); server.closeAllConnections(); await new Promise((resolve) => server.close(resolve));
}
