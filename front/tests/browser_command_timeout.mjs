// Durable command results, on a loopback fixture only.
import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";
import { saveBrowserFailure } from "./browser_failure.mjs";

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require("playwright")); }
catch (error) {
  const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
  if (!modules) throw error;
  ({ chromium } = require(join(modules, "playwright")));
}

const deadlineReason = "authorized worker duration elapsed";
const transportReason = "terminal transport closed";
const terminalResult = {
  exit_code: 137,
  timed_out: false,
  control_deadline: { deadline: "original_execution_deadline", stop: "engine_acknowledged" },
};
const cases = [
  { result: { exit_code: 137, timed_out: true, transport_stop_reason: deadlineReason }, timeout: true, reason: deadlineReason },
  { result: { exit_code: 137 }, timeout: false, reason: null },
  { result: { exit_code: 137, timed_out: false, transport_stop_reason: transportReason }, timeout: false, reason: transportReason },
  { result: { exit_code: 137, timed_out: true, transport_stop_reason: null }, timeout: true, reason: null },
  ...[
    { provenance: "declared" },
    { source: "provider" },
    { kind: "command.started" },
    { session_id: undefined },
    { session_id: "invalid" },
    { data: { output_captured: false } },
    { data: { output_captured: false, worker_id: "invalid" } },
    { data: { output_captured: false, worker_id: "00000000-0000-0000-0000-000000000000" } },
  ].map((event) => ({ event, result: terminalResult, timeout: false })),
  {
    result: terminalResult,
    timeout: false,
    deadline: "Durée d’exécution autorisée atteinte",
    controlStop: "Demande acceptée par le moteur",
  },
  {
    result: {
      exit_code: 137,
      timed_out: true,
      transport_stop_reason: transportReason,
      control_deadline: { deadline: "observation_window", stop: "unconfirmed" },
    },
    timeout: true,
    reason: transportReason,
    deadline: "Fenêtre d’observation écoulée",
    controlStop: "Résultat de la demande inconnu",
  },
  {
    event: { kind: "command.completed" },
    result: {
      exit_code: 0,
      timed_out: false,
      transport_stop_reason: null,
      control_deadline: { deadline: "observation_window", stop: "unconfirmed" },
    },
    timeout: false,
    deadline: "Fenêtre d’observation écoulée",
    controlStop: "Résultat de la demande inconnu",
  },
  {
    result: { exit_code: 137, control_deadline: { deadline: "constructor", stop: "engine_acknowledged" } },
    timeout: false,
  },
  {
    result: { exit_code: 137, control_deadline: { deadline: "observation_window", stop: ["unconfirmed"] } },
    timeout: false,
    deadline: "Fenêtre d’observation écoulée",
  },
];
const events = cases.map(({ event, result }, index) => ({
  seq: index + 1, id: "fixture-" + (index + 1), timestamp: "2026-11-18T12:00:00Z", kind: "command.failed",
  source: "runner", provenance: "observed", coverage: "observed",
  session_id: "388c75f1-00e9-4250-b8e1-d078d3c8e761",
  data: { output_captured: false, worker_id: "72c98e1a-b503-4b53-94fb-b4d471a8559e" },
  result,
  ...event,
}));
const api = new Map([
  ["/api/v1/identity", { tenant_id: "fixture", role: "viewer" }],
  ["/api/overview", { counts: { commands: events.length }, health: { collector: "ok" } }],
  ["/api/agents", { items: [] }],
  ["/api/sandboxes", { items: [] }],
  ["/api/integrity", { valid: true }],
  ["/api/events", { items: events, next_cursor: null }],
]);
const contentTypes = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" };
const server = createServer(async (request, response) => {
  const { pathname } = new URL(request.url, "http://127.0.0.1");
  if (api.has(pathname) || pathname === "/api/stream") {
    if (request.headers.authorization !== "Bearer command-timeout-fixture") {
      response.writeHead(401).end();
    } else if (pathname === "/api/stream") {
      response.writeHead(200, { "content-type": "text/event-stream" }).write(": fixture\n\n");
    } else {
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(api.get(pathname)));
    }
    return;
  }
  const path = pathname === "/" ? "index.html" : pathname.slice(1);
  try {
    if (!/^(?:index\.html|favicon\.svg|assets\/[\w.-]+)$/.test(path)) throw new Error("Unknown asset");
    const bytes = await readFile(new URL("../dist/" + path, import.meta.url));
    response.writeHead(200, { "content-type": contentTypes[extname(path)] }).end(bytes);
  } catch {
    response.writeHead(404).end();
  }
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = "http://127.0.0.1:" + server.address().port;
let browser;
let page;
try {
  browser = await chromium.launch({ chromiumSandbox: true, headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined });
  const context = await browser.newContext();
  await context.route("**/*", (route) => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
  page = await context.newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(origin);
  await page.getByLabel("Jeton de lecture").fill("command-timeout-fixture");
  await page.getByRole("button", { name: "Ouvrir la supervision", exact: true }).click();
  await page.getByRole("link", { name: "Actions", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const row = (label) => dialog.locator(".key-value")
    .filter({ has: page.locator("dt").filter({ hasText: label }) });
  const field = (label) => row(label).locator("dd");
  for (const [index, expected] of cases.entries()) {
    const event = events[index];
    await page.getByRole("button", { name: "Voir l’événement " + event.seq, exact: true }).click();
    await dialog.getByRole("tab", { name: "Commande", exact: true }).click();
    assert.deepEqual(await field("Code de sortie").allTextContents(), [String(event.result.exit_code)]);
    assert.deepEqual(await field("État de la commande").allTextContents(), expected.timeout ? ["Délai dépassé"] : [],
      "État de délai de l’événement " + event.seq);
    assert.deepEqual(await field("Motif de l’arrêt").allTextContents(), expected.reason ? [expected.reason] : [],
      "Motif d’arrêt de l’événement " + event.seq);
    assert.deepEqual(await field("Échéance du contrôle").allTextContents(), expected.deadline ? [expected.deadline] : [],
      "Échéance du contrôle de l’événement " + event.seq);
    assert.deepEqual(await field("Observation de la demande d’arrêt").allTextContents(),
      expected.controlStop ? [expected.controlStop] : [], "Observation de l’arrêt de l’événement " + event.seq);
    for (const label of ["Échéance du contrôle", "Observation de la demande d’arrêt"]) {
      if (await row(label).count()) {
        assert.equal(await row(label).getByText("Observé", { exact: true }).count(), 1, label);
      }
    }
    await dialog.getByRole("tab", { name: "Données brutes", exact: true }).click();
    assert.deepEqual(JSON.parse(await dialog.getByRole("tabpanel").locator("pre").textContent()).result, event.result);
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
  }
  assert.deepEqual(errors, []);
  process.stdout.write(`Délai et motif d’arrêt du résultat durable, ${cases.length} cas sur fixture loopback. PASS.\n`);
} catch (error) {
  if (page) {
    const directory = process.env.AI_GUARD_TEST_ARTIFACT_DIR
      || await mkdtemp(join(tmpdir(), "ai-guard-command-timeout-"));
    const artifacts = await saveBrowserFailure(page, { directory, secrets: ["command-timeout-fixture"] });
    if (artifacts) process.stderr.write(`Captures privées dans ${join(directory, artifacts.directory)}\n`);
  }
  throw error;
} finally {
  await browser?.close();
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}
