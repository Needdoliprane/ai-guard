// Fixtures exclusivement destinées à la vérification : jamais incluses dans le bundle.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { extname, join } from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { federationPage } from "./federation-fixture.mjs";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch {
  const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
  if (!modules)
    throw new Error(
      "Playwright est nécessaire pour ce contrôle facultatif. Installez-le dans un environnement de test ou définissez CODEX_PRIMARY_RUNTIME_NODE_MODULES.",
    );
  ({ chromium } = require(join(modules, "playwright")));
}
const root = fileURLToPath(new URL("..", import.meta.url));
const scenario = process.env.AI_GUARD_BROWSER_SCENARIO;
const eventFocusOnly = scenario === "event-focus";
const summaryFreshnessOnly = scenario === "summary-freshness";
const journalOlderOnly = scenario === "journal-older-boundaries";
const focusedScenario = eventFocusOnly || summaryFreshnessOnly || journalOlderOnly;
const journalEvent = (seq, kind = "command.completed") => ({
  seq, id: `journal-${seq}`, kind, timestamp: "2026-10-29T12:10:00Z", provenance: "observed",
});
const malicious = '<img src=x onerror="window.__xss=true">';
const events = eventFocusOnly ? [journalEvent(1)] : [
  {
    seq: 14,
    id: "evt-14",
    timestamp: "2026-10-29T12:10:00Z",
    kind: "command.completed",
    agent_id: "agent-main",
    project_id: "projet-test",
    sandbox_id: "sandbox-01",
    provenance: "observed",
    coverage: "observed",
    command: { argv: ["printf", malicious], cwd: "/workspace" },
    decision: {
      outcome: "allow",
      engine: "local",
      rule: "lecture",
      reason: "Commande autorisée",
    },
    result: { exit_code: 0, duration_ms: 14, stdout: malicious, stderr: "" },
    hash: "abc123",
    prev_hash: "def456",
  },
  {
    seq: 13,
    id: "evt-13",
    timestamp: "2026-10-29T12:09:00Z",
    kind: "policy.decision",
    agent_id: "agent-child",
    provenance: "declared",
    coverage: "generic",
    command: { argv: ["rm", "-rf", "/workspace"] },
    decision: {
      outcome: "deny",
      engine: "local",
      reason: "Commande refusée par la politique",
    },
  },
  {
    seq: 12,
    id: "evt-12",
    timestamp: "2026-10-29T12:08:00Z",
    kind: "workspace.diff",
    sandbox_id: "sandbox-01",
    provenance: "observed",
    data: { path: "src/main.rs", added: ["src/main.rs"] },
  },
];
const agents = [
  {
    id: "agent-main",
    session_id: "00000000-0000-4000-8000-000000000001",
    name: "Agent de développement",
    provider: "Codex",
    status: "running",
    model: "Modèle communiqué",
    project_id: "projet-test",
    sandbox_id: "sandbox-01",
    compatibility: "observed",
    provenance: "observed",
    task: "Vérification de l’interface",
    field_provenance: {
      model: "declared",
      task: "declared",
      provider: "observed",
    },
  },
  {
    id: "agent-child",
    name: "Relecture",
    parent_id: "agent-main",
    provider: "Claude",
    status: "completed",
    compatibility: "generic",
    provenance: "declared",
  },
];
let mode = "data";
const failingRoutes = new Map();
let integrityReadBarrier = null;
let limitNextJournalRead = false;
let pagedJournal = false;
let olderPageFailures = 0;
let journalOlderMode = "forbidden";
let heldOlderPage;
const olderEvent = { seq: 11, id: "evt-11", timestamp: "2026-10-29T12:07:00Z", kind: "workspace.diff", provenance: "observed", data: { path: "older.txt" } };
let controlMode = "data";
let controlReadBarrier = null;
const workerId = "00000000-0000-4000-8000-000000000001";
const secondWorkerId = "00000000-0000-4000-8000-000000000002";
const worker = {
  worker_id: workerId, state: "in_doubt", quota_held: true, revision: 4,
  launch: { version: 1, session_id: workerId, profile: "strict", runtime_kind: "docker",
    authority_id: "qa-federation-authority",
    provider: malicious, cpu_millis: 2000, memory_bytes: 1073741824, pids_limit: 256,
    timeout_seconds: 60, network: "none" },
  attempt: { attempt_id: workerId, stable_name: "worker-known", container_id: null, volume_name: "volume-known" },
  last_observation: null, unknown_reason: "Inspection runtime indisponible", cleanup_removed: null, cleanup_error: null,
  observed_started_at: null, allocation_inconsistent: false,
};
const allocation = {
  attempt_id: workerId, worker_id: workerId, terminal_observation_id: workerId,
  started_at: "2026-10-29T12:00:00Z", finished_at: "2026-10-29T12:00:01Z",
  duration_ns: "1000000000", cpu_millicore_nanoseconds: "9007199254740993000000001",
  memory_byte_nanoseconds: "1073741824000000000", method_version: 1, billable: false,
  source: { authority_id: "qa-authority", runtime_kind: "docker", container_id: "runtime-known", spec_hash: "a".repeat(64), provenance: "observed" },
};
const streams = new Set();
let streamCount = 0;
const requests = [];
const json = (response, value) => {
  if (response.federationRead) value = federationPage({ ...value,
    registry_status: controlMode === "legacy" ? "migration_required" : "available" }, {
    authority_id: "qa-federation-authority",
    broker: { broker_id: "90000000-0000-4000-8000-000000000001",
      current_incarnation_id: "90000000-0000-4000-8000-000000000002", state: "stale",
      grant_revision: "2", heartbeat_revision: "1", last_received_at: "2026-11-06T01:00:00.000Z",
      lease_expires_at: "2026-11-06T01:02:00.000Z" },
  });
  response.writeHead(200, {
    "content-type": "application/json",
    "cache-control": "no-store",
  });
  response.end(JSON.stringify(value));
};
const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  response.federationRead = url.pathname === "/api/v1/federation/workers";
  requests.push({
    path: request.url,
    auth: request.headers.authorization,
    cursor: request.headers["last-event-id"],
    method: request.method,
  });
  if (url.pathname.startsWith("/api/")) {
    if (request.headers.authorization !== "Bearer viewer-test-token") {
      response.writeHead(401);
      response.end();
      return;
    }
    if (url.pathname === "/api/v1/identity") return json(response, { tenant_id: "local", role: "viewer" });
    const failure = failingRoutes.get(url.pathname);
    if (failure) {
      response.writeHead(failure);
      response.end();
      return;
    }
    if (url.pathname.startsWith("/api/v1/")) {
      if (controlReadBarrier && url.pathname === "/api/v1/federation/workers") await controlReadBarrier;
      if (controlMode === "outage" || controlMode === "forbidden") {
        response.writeHead(controlMode === "forbidden" ? 403 : 503);
        response.end();
        return;
      }
      if (url.pathname === "/api/v1/quota") return json(response, {
        policy: { version: 1, max_active_workers: 3, max_cpu_millis: 6000,
          max_memory_bytes: 3221225472, max_timeout_seconds: 3600 },
        held_workers: 1, held_cpu_millis: 2000, held_memory_bytes: 1073741824,
      });
      if (url.pathname === "/api/v1/federation/workers" && controlMode === "large") {
        const offset = url.searchParams.has("cursor") ? 50 : 0;
        const items = Array.from({ length: 50 }, (_, index) => ({ ...worker,
          worker_id: `00000000-0000-4000-8000-${String(offset + index + 1).padStart(12, "0")}`,
        }));
        return json(response, { items, next_cursor: offset ? null : items.at(-1).worker_id });
      }
      if (url.pathname === "/api/v1/federation/workers") return json(response,
        controlMode === "invalid" ? { items: [{ worker_id: workerId }], next_cursor: null } : {
          items: controlMode === "empty" ? [] : [url.searchParams.has("cursor") ?
            { ...worker, worker_id: secondWorkerId, state: "cancelled", quota_held: false } : worker],
          next_cursor: controlMode === "empty" || url.searchParams.has("cursor") ? null : workerId,
        });
      if (url.pathname === `/api/v1/workers/${workerId}`) return json(response, worker);
      if (url.pathname === `/api/v1/workers/${secondWorkerId}`) return json(response, { ...worker, worker_id: secondWorkerId });
      if (url.pathname === "/api/v1/usage") return json(response, { items: controlMode === "empty" ? [] : [allocation], next_cursor: null });
    }
    if (
      mode === "outage" &&
      ["/api/agents", "/api/sandboxes", "/api/events"].includes(url.pathname)
    ) {
      response.writeHead(503);
      response.end();
      return;
    }
    if (url.pathname === "/api/overview")
      return json(response, {
        counts: {
          agents: mode === "empty" ? 0 : 1,
          subagents: 0,
          declared_agents: mode === "empty" ? 0 : 1,
          declared_subagents: mode === "empty" ? 0 : 1,
          sandboxes: mode === "empty" ? 0 : 1,
          commands: mode === "empty" ? 0 : 2,
          allowed: mode === "empty" ? 0 : 1,
          denied: mode === "empty" ? 0 : 1,
          pending: 0,
          security: mode === "empty" ? 0 : 1,
        },
        health: { collector: "ok", engine: "unknown", jev: "disabled" },
        resources: { cpu_percent: null, memory_bytes: null },
        coverage: "observed",
        timestamp: "2026-10-29T12:10:00Z",
      });
    if (url.pathname === "/api/agents")
      return json(response, { items: mode === "empty" ? [] : agents });
    if (url.pathname === "/api/sandboxes")
      return json(response, {
        items:
          mode === "empty"
            ? []
            : [
                {
                  id: "sandbox-01",
                  engine: "docker",
                  image: "agent-sandbox:local",
                  profile: "strict",
                  status: "running",
                  network: "none",
                  resources: null,
                  mounts: [],
                },
              ],
      });
    if (url.pathname === "/api/integrity") {
      const held = integrityReadBarrier;
      if (held) {
        held.started();
        await held.release;
      }
      return json(response, {
        valid: held?.valid ?? true,
        event_count: mode === "empty" ? 0 : 14,
        head_hash: "abc123",
      });
    }
    if (url.pathname === "/api/events") {
      if (limitNextJournalRead) {
        limitNextJournalRead = false;
        response.writeHead(429);
        response.end();
        return;
      }
      if (journalOlderOnly) {
        const before = url.searchParams.get("before");
        if (journalOlderMode === "forbidden") {
          if (before === "12") {
            response.writeHead(403);
            response.end();
            return;
          }
          if (before === null) return json(response, { items: events, next_cursor: 12 });
        } else {
          if (before === null) {
            return json(response, { items: [2002, 2001, 2000].map((seq) => journalEvent(seq)), next_cursor: 2000 });
          }
          if (before === "2000") {
            await heldOlderPage;
            return json(response, { items: [journalEvent(1999)], next_cursor: null });
          }
          if (before === "2001") {
            return json(response, { items: [journalEvent(2000), journalEvent(1999)], next_cursor: null });
          }
        }
        response.writeHead(400);
        response.end();
        return;
      }
      if (pagedJournal && url.searchParams.has("before")) {
        if (olderPageFailures > 0) {
          olderPageFailures--;
          response.writeHead(503);
          response.end();
          return;
        }
        return json(response, { items: [olderEvent], next_cursor: null });
      }
      if (pagedJournal) return json(response, { items: events, next_cursor: 12 });
      const q = url.searchParams.get("q")?.toLowerCase();
      return json(response, {
        items:
          mode === "empty"
            ? []
            : events.filter(
                (event) =>
                  (!q || JSON.stringify(event).toLowerCase().includes(q)) &&
                  (!url.searchParams.get("agent_id") ||
                    event.agent_id === url.searchParams.get("agent_id")),
              ),
        next_cursor: null,
      });
    }
    if (url.pathname === "/api/export") {
      response.writeHead(200, { "content-type": "application/x-ndjson" });
      response.end(events.map(JSON.stringify).join("\n"));
      return;
    }
    if (url.pathname === "/api/stream") {
      streamCount++;
      response.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
      });
      if (!focusedScenario && mode === "data")
        response.write(
          `id: 15\nevent: audit\ndata: ${JSON.stringify({ ...events[0], seq: 15, id: "evt-15", kind: "agent.updated", command: null, data: { message: "Mise à jour reçue en direct" } })}\n\n`,
        );
      if (!focusedScenario && streamCount === 1) response.end();
      else {
        streams.add(response);
        response.write(": maintien\n\n");
        const timer = setInterval(() => response.write(": maintien\n\n"), 1000);
        request.on("close", () => { clearInterval(timer); streams.delete(response); });
      }
      return;
    }
    response.writeHead(404);
    response.end();
    return;
  }
  if (url.pathname === "/healthz") return json(response, { status: "ok" });
  try {
    const path = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
    if (path.includes("..")) throw new Error("Chemin invalide");
    const body = await readFile(join(root, "dist", path));
    response.writeHead(200, {
      "content-type":
        {
          ".html": "text/html",
          ".js": "text/javascript",
          ".css": "text/css",
          ".svg": "image/svg+xml",
        }[extname(path)] || "application/octet-stream",
    });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({
  headless: true,
  chromiumSandbox: focusedScenario,
  executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
  args: focusedScenario ? [] : ["--no-sandbox"],
});
const context = await browser.newContext({
  viewport: { width: 1440, height: 1050 },
  reducedMotion: "reduce",
});
context.setDefaultTimeout(10000);
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const artifacts = join(root, "tests", "artifacts");
await mkdir(artifacts, { recursive: true });
const capture = async (name, fullPage = false, prefix = "editorial") => {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await page.screenshot({ path: join(artifacts, `${prefix}-${name}.png`), fullPage });
};
const captureContent = async (name, selector) => {
  const scroll = await page.evaluate(() => scrollY);
  await page.locator(selector).evaluate((node) => {
    const boundary = document.querySelector(".control-dock")?.getBoundingClientRect().bottom ?? 0;
    window.scrollTo(0, scrollY + node.getBoundingClientRect().top - boundary - 16);
  });
  await capture(name);
  await page.evaluate((position) => window.scrollTo(0, position), scroll);
};
const commandBoxes = async () => Promise.all([".view-previous", ".view-next", ".dock-refresh", ".export-place"]
  .map((selector) => page.locator(selector).boundingBox()));
const assertGeometry = async (label) => {
  const geometry = await page.evaluate(() => {
    const header = document.querySelector(".topbar").getBoundingClientRect();
    const dock = document.querySelector(".control-dock").getBoundingClientRect();
    const title = document.querySelector(".page h1").getBoundingClientRect();
    return { overflow: document.documentElement.scrollWidth > innerWidth,
      headerTop: header.top, headerBottom: header.bottom, dockTop: dock.top, dockBottom: dock.bottom, titleTop: title.top,
      navInert: document.querySelector(".control-dock").inert,
      targets: [...document.querySelectorAll(".control-dock a, .control-dock button:not([hidden]), .topbar button")]
        .map((node) => { const rect = node.getBoundingClientRect(); return { width: rect.width, height: rect.height, top: rect.top, bottom: rect.bottom }; }) };
  });
  assert.equal(geometry.overflow, false, `${label} : aucun débordement global`);
  assert.equal(geometry.headerTop, 0, `${label} : en-tête fixe`);
  assert.equal(geometry.dockTop, geometry.headerBottom, `${label} : dock sous l’en-tête`);
  assert.ok(geometry.titleTop >= geometry.dockBottom, `${label} : titre visible sous les commandes`);
  assert.equal(geometry.navInert, false, `${label} : navigation toujours accessible au clavier`);
  assert.ok(geometry.targets.every((rect) => rect.width >= 44 && rect.height >= 44), `${label} : cibles d’au moins 44 pixels`);
  assert.ok(geometry.targets.every((rect) => rect.top >= 0 && rect.bottom <= geometry.dockBottom), `${label} : commandes dans leur zone réservée`);
};
async function runJournalOlderBoundariesCheck() {
  const token = page.getByLabel("Jeton de lecture");
  const login = async () => {
    await token.fill("viewer-test-token");
    await page.getByRole("button", { name: "Ouvrir la supervision", exact: true }).click();
  };
  const journal = page.locator(".event-slot");
  const row = (seq) => journal.getByRole("button", { name: `Voir l’événement ${seq}`, exact: true });
  const more = page.getByRole("button", { name: "Charger les précédents", exact: true });
  const olderRequest = (before) => (request) => {
    const url = new URL(request.url());
    return url.pathname === "/api/events" && url.searchParams.get("before") === String(before);
  };
  const olderResponse = (before) => page.waitForResponse((response) => olderRequest(before)(response.request()));

  await page.goto(origin);
  await login();
  await row(14).waitFor();
  const refusedPage = olderResponse(12);
  await more.click();
  assert.equal((await refusedPage).status(), 403);
  await token.waitFor();
  assert.equal(await page.locator(".app-shell").count(), 0, "Un refus de pagination retire les données du client courant");

  journalOlderMode = "retention";
  await login();
  await row(2000).waitFor();
  await page.getByRole("link", { name: "Agents", exact: true }).click();
  await page.locator('[data-stream="live"]').waitFor();
  const appendLive = (sequences) => {
    assert.equal(streams.size, 1, "Un seul flux de la session courante");
    const batch = sequences.map((seq) =>
      `id: ${seq}\nevent: audit\ndata: ${JSON.stringify(journalEvent(seq, "agent.updated"))}\n\n`,
    ).join("");
    for (const stream of streams) stream.write(batch);
  };
  const afterBurst = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/overview");
  appendLive(Array.from({ length: 997 }, (_, index) => 2003 + index));
  await afterBurst;
  await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
  await row(2999).waitFor();
  assert.equal(await journal.locator("tbody tr").count(), 1000);

  let releaseOlderPage;
  heldOlderPage = new Promise((resolve) => { releaseOlderPage = resolve; });
  const outdatedPage = olderResponse(2000);
  try {
    const requested = page.waitForRequest(olderRequest(2000));
    await more.click();
    await requested;
    appendLive([3000]);
    await row(3000).waitFor();
    assert.equal(await row(2000).count(), 0, "Le direct déplace la limite pendant la lecture");
  } finally {
    releaseOlderPage();
  }
  assert.equal((await outdatedPage).status(), 200);
  const alert = page.getByRole("alert").filter({ hasText: "Historique incomplet" });
  await alert.waitFor();
  assert.equal(await row(1999).count(), 0, "La réponse obsolète ne crée pas de trou");

  const currentPage = olderResponse(2001);
  await page.getByRole("button", { name: "Réessayer de charger les précédents", exact: true }).click();
  assert.equal((await currentPage).status(), 200);
  await row(2000).waitFor();
  await row(1999).waitFor();
  await alert.waitFor({ state: "hidden" });
  assert.deepEqual(errors, []);
  console.log("PASS Chromium · refus de la page précédente et reprise après déplacement du curseur par SSE");
}

async function runEventFocusCheck() {
  await page.goto(origin);
  await page.getByLabel("Jeton de lecture").fill("viewer-test-token");
  await page.getByRole("button", { name: "Ouvrir la supervision", exact: true }).click();
  await page.getByRole("link", { name: "Actions", exact: true }).click();
  await page.locator('[data-stream="live"]').waitFor();
  await page.setViewportSize({ width: 390, height: 844 });

  const detail = (seq) => page.locator(`.event-slot button[aria-label="Voir l’événement ${seq}"]`);
  const scroll = page.locator(".event-slot .table-scroll");
  const dialog = page.getByRole("dialog");
  const append = (events) => {
    assert.equal(streams.size, 1, "Le flux de la fixture est connecté");
    const updates = events.map((event) =>
      `id: ${event.seq}\nevent: audit\ndata: ${JSON.stringify(event)}\n\n`,
    ).join("");
    for (const stream of streams) stream.write(updates);
  };
  await scroll.evaluate((node) => { node.scrollLeft = node.scrollWidth; });
  await detail(1).focus();
  const offset = await scroll.evaluate((node) => node.scrollLeft);
  assert.ok(offset > 0, "Le journal défile horizontalement");

  append([journalEvent(2)]);
  await detail(2).waitFor();
  assert.equal(await detail(1).evaluate((node) => node === document.activeElement), true,
    "Le flux conserve le focus sur le détail de la même séquence");
  assert.equal(await scroll.evaluate((node) => node.scrollLeft), offset,
    "Le flux conserve le défilement horizontal");

  await page.keyboard.press("Enter");
  await dialog.waitFor();
  append([journalEvent(3)]);
  await detail(3).waitFor({ state: "attached" });
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "Voir l’événement 1");
  assert.equal(await scroll.evaluate((node) => node.scrollLeft), offset,
    "Échap rend le focus au détail sans déplacer le journal");

  await page.keyboard.press("Enter");
  await dialog.waitFor();
  // Fill the live window with events projected out of Actions, so only the original detail is evicted.
  append(Array.from({ length: 998 }, (_, index) => journalEvent(index + 4, "agent.updated")));
  await detail(1).waitFor({ state: "detached" });
  await page.setViewportSize({ width: 390, height: 390 });
  await page.evaluate(() => window.scrollTo({ top: document.documentElement.scrollHeight, behavior: "instant" }));
  const title = page.locator("#main-content h1");
  assert.ok(await title.evaluate((node) => node.getBoundingClientRect().bottom <= 0),
    "Le titre est hors écran avant Échap");
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  await page.waitForFunction(() => document.activeElement?.matches("#main-content h1"));
  assert.ok(await title.evaluate((node) => {
    const bounds = node.getBoundingClientRect();
    const dockBottom = document.querySelector(".control-dock").getBoundingClientRect().bottom;
    return bounds.top >= dockBottom && bounds.bottom <= innerHeight;
  }), "Après Échap, le titre focalisé est visible sous les commandes");
  assert.deepEqual(errors, []);
  process.stdout.write("Journal viewer : défilement, focus SSE et retour par Échap vérifiés.\n");
}

async function runSummaryFreshnessCheck() {
  await page.goto(origin);
  const token = page.getByLabel("Jeton de lecture");
  const login = page.getByRole("button", { name: "Ouvrir la supervision", exact: true });
  const refresh = page.getByRole("button", { name: "Actualiser", exact: true });
  const exportJournal = page.getByRole("button", { name: "Exporter JSONL", exact: true });
  const summaryAlert = page.getByRole("alert").filter({ hasText: "erreur HTTP 503" });
  const exportConfirmation = page.getByText("Le journal complet a été exporté au format JSONL, toutes pages incluses.", { exact: true });
  const exportRefusal = page.getByRole("alert").filter({ hasText: "erreur HTTP 429" });
  const chainValid = page.getByText("Chaîne valide", { exact: true });
  const holdIntegrityRead = (valid = true) => {
    const started = Promise.withResolvers();
    const release = Promise.withResolvers();
    const barrier = { started: started.resolve, release: release.promise, valid };
    integrityReadBarrier = barrier;
    const deadline = setTimeout(() => started.reject(new Error("La lecture d’intégrité n’a pas commencé")), 10000);
    return { started: started.promise, release: () => {
      clearTimeout(deadline);
      if (integrityReadBarrier === barrier) integrityReadBarrier = null;
      release.resolve();
    } };
  };

  await token.fill("viewer-test-token");
  await login.click();
  await chainValid.waitFor();
  failingRoutes.set("/api/agents", 503);
  failingRoutes.set("/api/integrity", 503);
  await refresh.click();
  await summaryAlert.waitFor();

  const download = page.waitForEvent("download");
  await exportJournal.click();
  assert.equal(await (await download).failure(), null);
  await exportConfirmation.waitFor();
  assert.equal(await summaryAlert.isVisible(), true, "L’export réussi conserve l’avertissement de synthèse");
  await summaryAlert.getByText(/Agents/).waitFor();
  await summaryAlert.getByText(/Intégrité de l’audit/).waitFor();

  await page.getByRole("link", { name: "Actions", exact: true }).click();
  await page.getByRole("heading", { name: "Actions", exact: true }).waitFor();
  assert.equal(await summaryAlert.count(), 0, "La vue Actions ne présente pas l’avertissement de synthèse");
  await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
  await summaryAlert.getByText(/Intégrité de l’audit/).waitFor();

  failingRoutes.delete("/api/agents");
  await refresh.click();
  await summaryAlert.getByText(/Agents/).waitFor({ state: "hidden" });
  assert.equal(await summaryAlert.getByText(/Intégrité de l’audit/).isVisible(), true);
  assert.equal(await exportConfirmation.isVisible(), true, "La relecture conserve la confirmation d’export");

  failingRoutes.set("/api/export", 429);
  await exportJournal.click();
  await exportRefusal.waitFor();
  assert.equal(await summaryAlert.isVisible(), true, "Le refus d’export conserve l’avertissement de synthèse");

  failingRoutes.delete("/api/integrity");
  const olderRead = holdIntegrityRead(false);
  const integrityResponse = () => page.waitForResponse((response) => new URL(response.url()).pathname === "/api/integrity");
  let recovery, nextRead;
  try {
    await Promise.all([refresh.click(), olderRead.started]);
    recovery = holdIntegrityRead();
    await Promise.all([refresh.click(), recovery.started]);
    nextRead = holdIntegrityRead();
    await Promise.all([refresh.click(), nextRead.started]);
    const recoveredReply = integrityResponse();
    recovery.release();
    assert.equal((await (await recoveredReply).json()).valid, true);
    await chainValid.waitFor({ timeout: 3000 }).catch((error) => {
      if (error.name !== "TimeoutError") throw error;
      throw new Error("Une lecture réussie doit rétablir la synthèse pendant la lecture suivante", { cause: error });
    });
    await summaryAlert.waitFor({ state: "hidden" });
    assert.equal(await exportRefusal.isVisible(), true, "La synthèse rétablie conserve le refus d’export");

    const olderReply = integrityResponse();
    olderRead.release();
    assert.equal((await (await olderReply).json()).valid, false);
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(await chainValid.isVisible(), true, "Une lecture tardive plus ancienne ne remplace pas la synthèse rétablie");
  } finally {
    olderRead.release();
    recovery?.release();
    nextRead?.release();
  }

  failingRoutes.set("/api/agents", 503);
  await refresh.click();
  await summaryAlert.waitFor();
  failingRoutes.set("/api/export", 403);
  await exportJournal.click();
  await token.waitFor();
  assert.equal(await summaryAlert.count(), 0, "Le refus de session retire la synthèse du client");
  failingRoutes.clear();
  await token.fill("viewer-test-token");
  const firstRead = holdIntegrityRead();
  try {
    await Promise.all([login.click(), firstRead.started]);
    await page.getByText("Non vérifiée", { exact: true }).waitFor();
    assert.equal(await summaryAlert.count(), 0, "La nouvelle session ne reprend pas l’alerte avant la fin de sa première lecture");
  } finally { firstRead.release(); }
  await chainValid.waitFor();
  assert.equal(await summaryAlert.count(), 0, "La nouvelle session ne reprend pas l’alerte précédente");
  assert.deepEqual(errors, []);
  process.stdout.write("Synthèse viewer : alertes par source, exports indépendants et reprise vérifiés.\n");
}

async function runBrowserChecks() {
  await page.goto(origin);
  const buildFooter = page.locator(".build-identity");
  assert.equal(await buildFooter.locator("details").evaluate((node) => node.open), false,
    "Le diagnostic des versions est initialement fermé");
  const buildAnnouncement = buildFooter.getByRole("status");
  assert.equal(await buildAnnouncement.count(), 1,
    "L’annonce des versions reste accessible lorsque le diagnostic est fermé");
  assert.equal(await buildAnnouncement.evaluate((node) => node.parentElement.classList.contains("build-identity") && node.closest("details") === null), true,
    "La région d’annonce est un enfant direct du footer, hors du volet repliable");
  assert.equal(await page.locator(".login-header .brand-signature").count(), 1,
    "La signature OVPT est visible dans l’en-tête");
  assert.equal(await page.locator(".login-header .brand-signature").textContent(), "OVPT",
    "La signature OVPT accompagne ai-guard sans remplacer le nom du produit");
  assert.equal(await page.locator(".login-header .brand-wordmark").textContent(), "ai-guard");
  assert.equal(await page.getByRole("img", { name: "OVPT · ai-guard · Lecture seule", exact: true }).count(), 1,
    "Le logo typographique expose son nom complet aux technologies d’assistance");
  const faviconPath = await page.locator('link[rel="icon"]').getAttribute("href");
  assert.ok(faviconPath.startsWith("/favicon.svg?v=ovpt-"), "L’icône Chrome possède une nouvelle identité de cache");
  const favicon = await context.request.get(`${origin}${faviconPath}`);
  assert.equal(favicon.ok(), true, "Le favicon est servi par le bundle statique");
  assert.ok((await favicon.text()).includes("OVPT · ai-guard"), "Le favicon conserve le rattachement au produit");
  const iconPage = await context.newPage();
  await iconPage.setViewportSize({ width: 320, height: 192 });
  await iconPage.setContent(`<style>body{margin:0;font:12px Arial}section{height:96px;padding:18px;box-sizing:border-box;display:flex;align-items:center;gap:24px}section+section{background:#111;color:#f2f1e9}figure{margin:0;display:flex;align-items:center;gap:12px}</style><section><figure><img id="icon16" src="${origin}${faviconPath}" width="16" height="16" alt="OVPT 16 pixels">16 px</figure><figure><img id="icon32" src="${origin}${faviconPath}" width="32" height="32" alt="OVPT 32 pixels">32 px</figure></section><section><figure><img src="${origin}${faviconPath}" width="16" height="16" alt="OVPT 16 pixels">16 px</figure><figure><img src="${origin}${faviconPath}" width="32" height="32" alt="OVPT 32 pixels">32 px</figure></section>`);
  await iconPage.waitForFunction(() => [...document.images].every((node) => node.complete && node.naturalWidth > 0));
  await iconPage.locator("#icon16").screenshot({ path: join(artifacts, "ovpt-favicon-16.png") });
  await iconPage.locator("#icon32").screenshot({ path: join(artifacts, "ovpt-favicon-32.png") });
  await iconPage.screenshot({ path: join(artifacts, "ovpt-favicon-light-dark.png") });
  await iconPage.close();
  await capture("login-1440-light");
  await page.setViewportSize({ width: 320, height: 568 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "Connexion utilisable à 320px");
  assert.equal(await page.evaluate(() => document.querySelector("#viewer-token").getBoundingClientRect().top <
    document.querySelector(".login-facts").getBoundingClientRect().top), true,
    "Sur mobile, l’accès authentifié précède les explications détaillées");
  for (const size of [{ width: 320, height: 568 }, { width: 390, height: 844 }, { width: 844, height: 390 }]) {
    await page.setViewportSize(size);
    const entry = await page.locator(".login-submit").boundingBox();
    const tokenField = await page.getByLabel("Jeton de lecture").boundingBox();
    assert.ok(entry.y >= 0 && entry.y + entry.height <= size.height,
      `L’action d’authentification est entièrement visible dès l’arrivée à ${size.width}×${size.height}`);
    assert.ok(tokenField.y >= 0 && tokenField.y + tokenField.height <= size.height,
      `Le champ d’accès est entièrement visible dès l’arrivée à ${size.width}×${size.height}`);
    assert.equal(await page.evaluate(() => scrollY), 0, "La preuve initiale est prise sans défilement");
    await capture(`login-${size.width}-${size.height}-initial-light`, false, "ovpt");
    await page.getByRole("button", { name: "Activer le thème sombre" }).click();
    await capture(`login-${size.width}-${size.height}-initial-dark`, false, "ovpt");
    await page.getByRole("button", { name: "Activer le thème clair" }).click();
    await page.getByLabel("Jeton de lecture").focus();
    await page.keyboard.press("Tab");
    assert.equal(await page.locator(".login-submit").evaluate((node) => node === document.activeElement), true,
      "Depuis le jeton, Tab atteint directement l’action de connexion");
    await capture(`login-${size.width}-${size.height}-first-screen`);
    await page.evaluate(() => document.activeElement?.blur());
  }
  await page.setViewportSize({ width: 320, height: 568 });
  await capture("login-320-light");
  await captureContent("login-form-320-light", ".login-card");
  await page.setViewportSize({ width: 1440, height: 1050 });
  const publicStatusPaths = ["/api/v1/status", "/api/v1/gateway/status"];
  assert.equal(
    requests.some(({ path, method, auth }) =>
      path.startsWith("/api/") &&
      !(method === "GET" && auth === undefined && publicStatusPaths.includes(path)),
    ),
    false,
    "Avant connexion, seules les lectures de version publiques sans Authorization sont autorisées",
  );
  await page.getByLabel("Jeton de lecture").fill("mauvais-jeton");
  await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
  await page
    .getByRole("alert")
    .filter({ hasText: /Jeton refusé/ })
    .waitFor();
  await page.getByLabel("Jeton de lecture").fill("viewer-test-token");
  await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
  await page
    .getByRole("heading", { name: "Vue d’ensemble", exact: true })
    .waitFor();
  assert.equal(await page.locator(".control-dock").count(), 1,
    "La navigation est un dock supérieur permanent");
  assert.equal(await page.locator(".control-dock .nav-link").count(), 9,
    "Les neuf vues réelles restent accessibles");
  assert.equal(await page.locator(".view-position").textContent(), "Vue 01 / 09",
    "La position désigne une vue, sans progression de validation");
  await assertGeometry("1440×1050");
  const firstCommands = await commandBoxes();
  await page.getByRole("button", { name: "Suivant : Agents", exact: true }).click();
  await page.getByRole("heading", { name: "Agents", exact: true }).waitFor();
  assert.deepEqual(await commandBoxes(), firstCommands, "Les commandes gardent leur position entre les vues");
  await page.getByRole("button", { name: "Précédent : Vue d’ensemble", exact: true }).click();
  assert.equal(await page.locator(".view-previous").isDisabled(), true, "Le début du parcours ne fabrique pas une destination");
  await page
    .getByText("Mise à jour reçue en direct", { exact: true })
    .waitFor();
  assert.equal(
    await page
      .getByRole("combobox", { name: "Tous les agents", exact: true })
      .locator('option[value="agent-main"]')
      .count(),
    1,
    "Les filtres sont alimentés après le chargement initial",
  );
  await page.waitForFunction(() =>
    document.querySelector('[data-stream="live"]'),
  );
  await page.waitForTimeout(1400);
  assert.ok(
    requests.some(
      (request) => request.path === "/api/stream" && request.cursor === "15",
    ),
    "Le curseur est envoyé à la reconnexion",
  );
  assert.equal(
    await page.evaluate(
      () =>
        Object.keys(localStorage).length + Object.keys(sessionStorage).length,
    ),
    0,
  );
  assert.equal(
    await page.locator("#app img").count(),
    0,
    "Les balises des données ne deviennent pas des images",
  );
  assert.equal(await page.evaluate(() => window.__xss), undefined);
  await page.getByRole("button", { name: /Voir l’événement 14/ }).click();
  await page.getByRole("dialog").waitFor();
  await page.getByRole("tab", { name: "Commande" }).click();
  assert.ok((await page.getByRole("dialog").innerText()).includes(malicious));
  await page.getByRole("tab", { name: "Données brutes" }).click();
  assert.ok((await page.getByRole("dialog").innerText()).includes("evt-14"));
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: "Agents", exact: true }).click();
  await page.getByRole("button", { name: "Hiérarchie", exact: true }).click();
  assert.equal(
    await page
      .locator('[data-agent-id="agent-main"] [data-agent-id="agent-child"]')
      .count(),
    1,
  );
  // Opening an agent now reads the same paginated worker registry used by its view.
  let releaseControlRead;
  controlReadBarrier = new Promise((resolve) => { releaseControlRead = resolve; });
  await page
    .getByRole("button", { name: "Détails", exact: true })
    .first()
    .click();
  assert.equal(
    await page
      .getByRole("dialog")
      .locator(".key-value")
      .filter({ hasText: "Modèle communiqué" })
      .getByText("Déclaré", { exact: true })
      .count(),
    1,
    "La provenance du modèle est distincte de celle de l’identité",
  );
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: "Workers", exact: true }).click();
  await page.getByText("Chargement des workers…", { exact: true }).waitFor();
  await capture("workers-loading-1440-light");
  await page.getByRole("button", { name: "Activer le thème sombre" }).click();
  await capture("workers-loading-1440-dark", false, "ovpt");
  await page.getByRole("button", { name: "Activer le thème clair" }).click();
  for (const size of [{ width: 320, height: 568 }, { width: 390, height: 844 }]) {
    await page.setViewportSize(size);
    await assertGeometry(`Chargement ${size.width}px`);
    await capture(`workers-loading-${size.width}-light`);
    await captureContent(`workers-loading-content-${size.width}-light`, ".control-view");
    await page.getByRole("button", { name: "Activer le thème sombre" }).click();
    await capture(`workers-loading-${size.width}-dark`, false, "ovpt");
    await page.getByRole("button", { name: "Activer le thème clair" }).click();
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  controlReadBarrier = null;
  releaseControlRead();
  await page.locator(".control-view").getByText("État incertain", { exact: true }).waitFor();
  await page.getByRole("columnheader", { name: "Associations historiques aux brokers", exact: true }).waitFor();
  assert.equal(await page.locator(".control-view").getByText("90000000-0000-4000-8000-000000000001", { exact: true }).count(), 1);
  assert.equal(await page.locator(".control-view").getByText("Bail périmé", { exact: true }).count(), 1);
  assert.equal(await page.locator(".control-view").getByText("Autorité retirée du grant courant", { exact: true }).count(), 1);
  assert.ok((await page.locator(".control-view").innerText()).includes("elle ne prouve pas quel broker les a effectués"));
  await page.screenshot({ path: join(root, "tests", "artifacts", "workers-desktop.png"), fullPage: true });
  assert.ok((await page.locator(".control-view").innerText()).includes("Inconnue · aucune observation"));
  assert.equal(await page.locator("#app img").count(), 0, "Le fournisseur worker reste du texte");
  await page.getByRole("button", { name: "Détails", exact: true }).click();
  await page.getByRole("dialog").getByText("Aucune observation reçue. L’état et la durée runtime restent inconnus.", { exact: true }).waitFor();
  assert.ok((await page.getByRole("dialog").innerText()).includes(workerId));
  await page.keyboard.press("Escape");
  const detailTrigger = page.locator(".control-view").getByRole("button", { name: "Détails", exact: true });
  await detailTrigger.focus();
  const detachedTrigger = await detailTrigger.elementHandle();
  await page.keyboard.press("Enter");
  await page.getByRole("dialog").getByText("Aucune observation reçue. L’état et la durée runtime restent inconnus.", { exact: true }).waitFor();
  const refreshedWhileModal = page.waitForResponse((response) => response.url().endsWith("/api/v1/federation/workers?limit=50"));
  const modalEvent = { seq: 16, id: "worker-modal-refresh", kind: "control.worker.transition", timestamp: "2026-10-29T12:19:00Z" };
  for (const stream of streams) stream.write(`id: ${modalEvent.seq}\nevent: audit\ndata: ${JSON.stringify(modalEvent)}\n\n`);
  await refreshedWhileModal;
  await page.waitForFunction((trigger) => !trigger.isConnected, detachedTrigger);
  await page.waitForFunction(() => document.querySelector(".control-view")?.getAttribute("aria-busy") === "false");
  await page.keyboard.press("Escape");
  await page.waitForFunction((key) => document.activeElement?.getAttribute("data-focus-key") === key,
    `worker-${workerId}`, { timeout: 2000 });
  const focusAfterModalRefresh = await page.evaluate(() => ({
    tag: document.activeElement?.tagName,
    key: document.activeElement?.getAttribute("data-focus-key"),
  }));
  process.stdout.write(`Focus après détail + SSE : ${JSON.stringify(focusAfterModalRefresh)}\n`);
  assert.equal(focusAfterModalRefresh.key, `worker-${workerId}`, "Échap retrouve le bouton worker après remplacement du registre");
  await page.keyboard.press("Enter");
  await page.getByRole("dialog").getByText("Aucune observation reçue. L’état et la durée runtime restent inconnus.", { exact: true }).waitFor();
  controlMode = "empty";
  const disappearedWhileModal = page.waitForResponse((response) => response.url().endsWith("/api/v1/federation/workers?limit=50"));
  const missingEvent = { ...modalEvent, seq: 17, id: "worker-modal-disappeared" };
  for (const stream of streams) stream.write(`id: ${missingEvent.seq}\nevent: audit\ndata: ${JSON.stringify(missingEvent)}\n\n`);
  await disappearedWhileModal;
  await page.locator(".control-view").getByText("Aucun worker sur cette page", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => document.activeElement?.getAttribute("data-focus-key") === "page-summary-workers", null, { timeout: 2000 });
  const focusAfterWorkerDisappeared = await page.evaluate(() => ({
    tag: document.activeElement?.tagName,
    key: document.activeElement?.getAttribute("data-focus-key"),
  }));
  process.stdout.write(`Focus après disparition du worker : ${JSON.stringify(focusAfterWorkerDisappeared)}\n`);
  assert.equal(focusAfterWorkerDisappeared.key, "page-summary-workers", "Échap rejoint le résumé si le worker a disparu");
  controlMode = "data";
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await page.locator(".control-view").getByText("État incertain", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Page suivante", exact: true }).click();
  await page.getByText("Réservation annulée", { exact: true }).waitFor();
  assert.ok(requests.some((item) => item.path === `/api/v1/federation/workers?limit=50&cursor=${workerId}`));
  assert.equal(await page.locator(".control-view").evaluate((node) => node.contains(document.activeElement)), true, "Le focus reste dans le contenu après pagination");
  await page.getByRole("button", { name: "Page précédente", exact: true }).click();
  await page.locator(".control-view").getByText("État incertain", { exact: true }).waitFor();
  controlMode = "outage";
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await page.getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
  await capture("workers-stale-1440-light");
  assert.equal(await page.locator(".control-view").getByText("État incertain", { exact: true }).count(), 1, "Une erreur conserve la dernière lecture et son état périmé");
  controlMode = "invalid";
  await page.getByRole("button", { name: "Réessayer", exact: true }).click();
  await page.getByText(/Contrat de lecture v1 incomplet/).waitFor();
  assert.equal(await page.getByText("Aucun worker sur cette page", { exact: true }).count(), 0, "Une réponse invalide ne devient pas une liste vide");
  controlMode = "legacy";
  await page.getByRole("button", { name: "Réessayer", exact: true }).click();
  await page.getByText("Registre des brokers indisponible · migration explicite requise", { exact: true }).waitFor();
  assert.equal(await page.locator(".control-view").getByText("État incertain", { exact: true }).count(), 1,
    "Les vrais workers restent visibles sans registre broker");
  await page.getByText("Voir la procédure de migration du registre", { exact: true }).click();
  await page.getByText(/Migrations successives 2 → 3 → 4 → 5 → 6/).waitFor();
  assert.equal(await page.locator(".control-view").getByText("Autorité non enregistrée", { exact: true }).count(), 0,
    "Une absence structurelle de registre ne devient pas une autorité non enregistrée");
  controlMode = "data";
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await page.getByText("Lecture réussie", { exact: true }).waitFor();
  controlMode = "large";
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".control-table tbody tr").length === 50);
  const longViewCommands = await commandBoxes();
  await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
  assert.deepEqual(await commandBoxes(), longViewCommands, "Précédent, suivant, actualisation et export restent fixes après 50 workers");
  await capture("workers-dense-1440-light");
  await page.getByRole("button", { name: "Suivant : Quotas", exact: true }).click();
  await assertGeometry("Après une longue vue");
  await page.getByRole("button", { name: "Précédent : Workers", exact: true }).click();
  await page.getByRole("button", { name: "Page suivante", exact: true }).click();
  await page.getByText("Fin des résultats à cette lecture", { exact: true }).waitFor();
  assert.equal(await page.locator(".control-table tbody tr").count(), 50, "100 workers restent consultables en deux pages bornées");
  controlMode = "data";
  await page.getByRole("button", { name: "Première page", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".control-table tbody tr").length === 1);
  const readsBeforeBurst = requests.filter((item) => item.path.startsWith("/api/v1/federation/workers?")).length;
  for (let index = 0; index < 200; index++) {
    const event = { seq: 1000 + index, id: `burst-${index}`, kind: "control.worker.transition", timestamp: "2026-10-29T12:20:00Z" };
    for (const stream of streams) stream.write(`id: ${event.seq}\nevent: audit\ndata: ${JSON.stringify(event)}\n\n`);
  }
  await page.waitForTimeout(2300);
  const burstReads = requests.filter((item) => item.path.startsWith("/api/v1/federation/workers?")).length - readsBeforeBurst;
  assert.ok(burstReads >= 1 && burstReads <= 2, "200 événements worker déclenchent une relecture bornée du registre");
  await page.getByRole("link", { name: "Agents", exact: true }).click();
  await page.getByRole("button", { name: "Détails", exact: true }).first().click();
  assert.equal(await page.getByRole("dialog").getByRole("button", { name: workerId, exact: true }).count(), 1, "L’agent est relié par sa session explicite");
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: "Quotas", exact: true }).click();
  await page.getByText("Workers actifs réservés", { exact: true }).waitFor();
  await page.screenshot({ path: join(root, "tests", "artifacts", "quota-desktop.png"), fullPage: true });
  assert.ok((await page.locator(".quota-list").innerText()).includes("2\u202f000 milliCPU"));
  await page.getByRole("link", { name: "Allocations", exact: true }).click();
  await page.getByText("9\u202f007\u202f199\u202f254\u202f740\u202f993,000000001 milliCPU·s", { exact: true }).waitFor();
  await page.locator(".control-view").getByText("docker · qa-authority", { exact: true }).waitFor();
  assert.ok((await page.locator(".control-view").innerText()).includes("non facturable"));
  assert.equal(await page.getByRole("button", { name: "Exporter JSONL" }).count(), 0, "Aucun export de journal ambigu dans le relevé d’allocation");
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, "Le relevé mobile ne déborde pas la page");
  await page.locator(".control-view").getByRole("button", { name: "Worker", exact: true }).focus();
  const horizontalPosition = await page.locator(".control-view .table-scroll").evaluate((node) => node.scrollLeft);
  await page.keyboard.press("ArrowLeft");
  await page.waitForFunction((position) => document.querySelector(".control-view .table-scroll").scrollLeft < position, horizontalPosition);
  const leftPosition = await page.locator(".control-view .table-scroll").evaluate((node) => node.scrollLeft);
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction((position) => document.querySelector(".control-view .table-scroll").scrollLeft > position, leftPosition);
  await page.getByRole("link", { name: "Allocations", exact: true }).click();
  await page.screenshot({ path: join(root, "tests", "artifacts", "allocations-mobile.png"), fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.getByRole("link", { name: "Actions", exact: true }).click();
  const selectedPeriodRequest = page.waitForRequest(
    (request) =>
      request.url().includes("/api/events?") &&
      new URL(request.url()).searchParams.has("from"),
  );
  await page
    .getByRole("combobox", { name: "Période des événements" })
    .selectOption("hour");
  const periodFrom = new URL(
    (await selectedPeriodRequest).url(),
  ).searchParams.get("from");
  assert.ok(
    Number.isFinite(Date.parse(periodFrom)),
    "La période est sérialisée en date absolue",
  );
  const filteredAgain = page.waitForRequest(
    (request) =>
      request.url().includes("/api/events?") &&
      new URL(request.url()).searchParams.has("from"),
  );
  await page.getByRole("button", { name: "Filtrer", exact: true }).click();
  assert.equal(
    new URL((await filteredAgain).url()).searchParams.get("from"),
    periodFrom,
    "Le filtre ne recalcule pas le début de période",
  );
  const periodDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Exporter JSONL" }).click();
  await periodDownload;
  assert.equal(
    new URL(
      requests.findLast((request) =>
        request.path.startsWith("/api/export?"),
      ).path,
      origin,
    ).searchParams.get("from"),
    periodFrom,
    "L’export conserve le début de période",
  );
  await page
    .getByRole("combobox", { name: "Période des événements" })
    .selectOption("all");
  await page.getByRole("button", { name: /Voir l’événement 14/ }).waitFor();
  await page.getByLabel("Rechercher dans les événements").fill("introuvable");
  await page.getByRole("button", { name: "Filtrer", exact: true }).click();
  await page.getByText("Aucun événement correspondant").waitFor();
  await page.getByRole("button", { name: "Réinitialiser les filtres" }).click();
  await page.getByRole("button", { name: /Voir l’événement 14/ }).waitFor();
  await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
  await mkdir(join(root, "tests", "artifacts"), { recursive: true });
  for (const [index, name] of ["Vue d’ensemble", "Agents", "Actions", "Sécurité", "Fichiers", "Sandboxes", "Workers", "Quotas", "Allocations"].entries()) {
    await page.getByRole("link", { name, exact: true }).click();
    await page.getByRole("heading", { name, exact: true }).waitFor();
    await assertGeometry(`Vue ${index + 1}/9`);
    await capture(`view-${String(index + 1).padStart(2, "0")}-1440-light`);
  }
  await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
  await page.screenshot({
    path: join(root, "tests", "artifacts", "overview-light.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Activer le thème sombre" }).click();
  const darkSurfaces = await page.evaluate(() => [document.body, document.querySelector(".event-panel"), document.querySelector(".event-table thead")]
    .map((node) => getComputedStyle(node).backgroundColor));
  assert.ok(darkSurfaces.every((color) => {
    const [r, g, b] = color.match(/\d+/g).map(Number);
    return r === g && g === b;
  }), "Le thème sombre utilise des surfaces noires neutres");
  assert.equal(new Set(darkSurfaces).size, 3, "Fond, panneau et en-tête de tableau restent distincts dans le thème sombre");
  await page.screenshot({
    path: join(root, "tests", "artifacts", "overview-dark.png"),
    fullPage: true,
  });
  await capture("overview-1440-dark");
  limitNextJournalRead = true;
  const refreshOverview = page.getByRole("button", { name: "Actualiser", exact: true });
  await refreshOverview.click();
  const journalLimit = page.getByRole("alert").getByText(
    "Le collecteur a répondu avec une erreur HTTP 429.", { exact: true },
  );
  const unavailableJournal = page.getByText("Journal indisponible", { exact: true });
  await journalLimit.waitFor();
  await unavailableJournal.waitFor();
  assert.equal(await page.getByText("Aucun événement reçu", { exact: true }).count(), 0);
  await page.getByRole("banner").getByText("Client : local", { exact: true }).waitFor();
  await refreshOverview.click();
  await page.getByRole("button", { name: "Voir l’événement 14", exact: true }).waitFor();
  await unavailableJournal.waitFor({ state: "hidden" });
  await journalLimit.waitFor({ state: "hidden" });

  // A failed "load older" must keep the history already read and offer a retry: never an empty journal.
  pagedJournal = true;
  olderPageFailures = 1;
  await refreshOverview.click();
  await page.getByRole("button", { name: "Voir l’événement 14", exact: true }).waitFor();
  const pageRequestStart = requests.length;
  await page.getByRole("button", { name: "Charger les précédents", exact: true }).focus();
  await page.keyboard.press("Enter");
  const historyAlert = page.getByRole("alert").filter({ hasText: "Historique incomplet" });
  await historyAlert.waitFor();
  assert.equal(await page.getByText("Journal indisponible", { exact: true }).count(), 0);
  await page.getByRole("button", { name: "Voir l’événement 14", exact: true }).waitFor();
  const retryHistory = page.getByRole("button", { name: "Réessayer de charger les précédents", exact: true });
  assert.equal(await retryHistory.evaluate((node) => node === document.activeElement), true,
    "Le focus doit rester sur la reprise après un échec de lecture");
  await page.keyboard.press("Enter");
  await page.getByRole("button", { name: "Voir l’événement 11", exact: true }).waitFor();
  await historyAlert.waitFor({ state: "hidden" });
  await page.getByText("Fin de l’historique chargé", { exact: true }).waitFor();
  assert.equal(await page.locator(".event-slot .table-footer").evaluate((footer) =>
    footer.contains(document.activeElement) && document.activeElement.textContent.includes("chargés")), true,
    "En fin d’historique, le focus doit rejoindre le résumé");
  const cursors = requests.slice(pageRequestStart)
    .map(({ path }) => new URL(path, "http://localhost"))
    .filter((url) => url.pathname === "/api/events" && url.searchParams.has("before"))
    .map((url) => url.searchParams.get("before"));
  assert.deepEqual(cursors, ["12", "12"], "La reprise relit la même limite du journal");
  pagedJournal = false;
  await refreshOverview.click();
  await page.getByRole("button", { name: "Voir l’événement 14", exact: true }).waitFor();

  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Exporter JSONL" }).click();
  assert.ok((await download).suggestedFilename().endsWith(".jsonl"));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: join(root, "tests", "artifacts", "overview-mobile.png"),
    fullPage: true,
  });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
    true,
    "Aucun débordement horizontal de la page mobile",
  );
  for (const size of [{ width: 390, height: 844 }, { width: 320, height: 568 }, { width: 844, height: 390 }, { width: 1280, height: 720 }]) {
    await page.setViewportSize(size);
    await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
    await assertGeometry(`${size.width}×${size.height}`);
    await capture(`overview-${size.width}-${size.height}-dark`);
    await capture(`overview-${size.width}-${size.height}-dark`, false, "ovpt");
    const positions = await commandBoxes();
    await page.getByRole("link", { name: "Fichiers", exact: true }).focus();
    await page.keyboard.press("Enter");
    await page.getByRole("heading", { name: "Fichiers", exact: true }).waitFor();
    await assertGeometry(`Clavier ${size.width}×${size.height}`);
    assert.deepEqual(await commandBoxes(), positions, "Les commandes ne bougent pas en navigation clavier");
    assert.equal(await page.evaluate(() => document.activeElement?.tagName), "H1", "La navigation place le focus sur le titre visible");
    await page.getByRole("button", { name: "Activer le thème clair" }).click();
    await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
    await capture(`overview-${size.width}-${size.height}-light`);
    await page.getByRole("link", { name: "Workers", exact: true }).click();
    await capture(`workers-${size.width}-${size.height}-light`);
    await captureContent(`workers-data-${size.width}-${size.height}-light`, ".control-table");
    await page.getByRole("link", { name: "Allocations", exact: true }).click();
    await capture(`allocations-${size.width}-${size.height}-light`);
    await captureContent(`allocations-data-${size.width}-${size.height}-light`, ".control-table");
    assert.equal(await page.locator(".view-next").isDisabled(), true, "La dernière vue affiche sa borne");
    const disabledBackground = await page.locator(".view-next").evaluate((node) => getComputedStyle(node).backgroundColor);
    const [disabledRed, disabledGreen, disabledBlue] = disabledBackground.match(/\d+/g).map(Number);
    assert.ok(disabledRed === disabledGreen && disabledGreen === disabledBlue,
      "La dernière vue présente Suivant désactivé sur un fond neutre, sans citron olive");
    assert.deepEqual(await commandBoxes(), positions, "Les commandes gardent leur position quand l’export n’est pas applicable");
    await page.getByRole("button", { name: "Activer le thème sombre" }).click();
    await capture(`allocations-${size.width}-${size.height}-dark`, false, "ovpt");
    await captureContent(`allocations-data-${size.width}-${size.height}-dark`, ".control-table");
  }
  assert.equal(await page.locator(".dock-refresh").evaluate((node) => getComputedStyle(node).transitionDuration), "0s", "Le mouvement réduit est respecté");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole("link", { name: "Fichiers", exact: true }).click();
  await page.getByRole("heading", { name: "Fichiers", exact: true }).waitFor();
  await page.getByRole("button", { name: "Se déconnecter" }).click();
  await page.getByLabel("Jeton de lecture").waitFor();
  mode = "empty";
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.getByLabel("Jeton de lecture").fill("viewer-test-token");
  await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
  await page
    .getByRole("heading", { name: "Vue d’ensemble", exact: true })
    .waitFor();
  await page.getByText("Aucun événement reçu").waitFor();
  await capture("overview-empty-1440-dark");
  await page.setViewportSize({ width: 320, height: 568 });
  await capture("overview-empty-320-dark");
  await captureContent("overview-empty-content-320-dark", ".event-panel");
  await page.setViewportSize({ width: 1440, height: 1050 });
  controlMode = "empty";
  await page.getByRole("link", { name: "Workers", exact: true }).click();
  await page.getByText("Aucun worker sur cette page", { exact: true }).waitFor();
  controlMode = "outage";
  await page.getByRole("link", { name: "Allocations", exact: true }).click();
  await page.getByText("Allocations inconnues", { exact: true }).waitFor();
  assert.equal(await page.getByText("Aucune allocation terminale sur cette page", { exact: true }).count(), 0);
  controlMode = "empty";
  await page.getByRole("button", { name: "Réessayer", exact: true }).click();
  await page.getByText("Aucune allocation terminale sur cette page", { exact: true }).waitFor();
  await page.getByRole("link", { name: "Vue d’ensemble", exact: true }).click();
  assert.ok((await page.locator("#app").innerText()).includes("Inconnu"));
  await page.screenshot({
    path: join(root, "tests", "artifacts", "overview-empty.png"),
    fullPage: true,
  });
  mode = "outage";
  await page.getByRole("button", { name: "Actualiser", exact: true }).click();
  await page.getByText("Journal indisponible", { exact: true }).waitFor();
  await capture("overview-error-1440-dark");
  await page.setViewportSize({ width: 390, height: 844 });
  await capture("overview-error-390-dark");
  await captureContent("overview-error-content-390-dark", ".event-panel");
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.getByRole("link", { name: "Agents", exact: true }).click();
  await page
    .getByText("Données des agents indisponibles", { exact: true })
    .waitFor();
  controlMode = "forbidden";
  await page.getByRole("link", { name: "Quotas", exact: true }).click();
  await page.getByLabel("Jeton de lecture").waitFor();
  await page.reload();
  await page.getByLabel("Jeton de lecture").waitFor();
  assert.equal(
    requests.some(
      (request) =>
        request.path.includes("token") || request.path.includes("viewer-test"),
    ),
    false,
  );
  assert.equal(
    requests
      .filter((request) => request.path.startsWith("/api/"))
      .every((request) => request.method === "GET"),
    true,
  );
  assert.deepEqual(
    errors,
    [],
    "Aucune exception dans la console du navigateur",
  );
  process.stdout.write(
    "Contrôle navigateur réussi : connexion, XSS, SSE et curseur, filtres, hiérarchie, export, mémoire seule, thèmes, mobile, vide, workers paginés/focus/100 lignes, quota, allocations exactes, états périmés/invalides, rafale 200 événements et refus 403.\n",
  );
}

try {
  if (focusedScenario) {
    await context.route("**/*", (route) =>
      new URL(route.request().url()).origin === origin ? route.continue() : route.abort(),
    );
  }
  if (eventFocusOnly) await runEventFocusCheck();
  else if (summaryFreshnessOnly) await runSummaryFreshnessCheck();
  else if (journalOlderOnly) await runJournalOlderBoundariesCheck();
  else await runBrowserChecks();
} catch (error) {
  await mkdir(join(root, "tests", "artifacts"), { recursive: true });
  await page.screenshot({
    path: join(root, "tests", "artifacts", "failure.png"),
    fullPage: true,
  });
  process.stderr.write(
    JSON.stringify(
      {
        errors,
        requests: requests.map(({ path, cursor, method }) => ({
          path,
          cursor,
          method,
        })),
        page: await page.locator("body").innerText(),
      },
      null,
      2,
    ) + "\n",
  );
  throw error;
} finally {
  await browser.close();
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}

if (!focusedScenario) await import("./browser_identity.mjs");
