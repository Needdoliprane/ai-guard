// Read-only check of a durable local-model decision using the viewer dashboard.
import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { saveBrowserFailure } from "./browser_failure.mjs";
import { closeBrowserResources, installBrowserGuard } from "./browser_live.mjs";

const require = createRequire(import.meta.url);
class Blocked extends Error {}
let stage = "configuration";
let viewer;
let browser;
let context;
let guard;
let page;

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Blocked(`${name} est requis.`);
  return value;
}

function dashboardOrigin() {
  const value = required("AI_GUARD_TEST_DASHBOARD");
  try {
    const url = new URL(value);
    if (["http:", "https:"].includes(url.protocol) &&
      ["127.0.0.1", "[::1]", "localhost"].includes(url.hostname) &&
      !url.username && !url.password && !url.search && !url.hash && url.pathname === "/")
      return url.origin;
  } catch { /* Report the same error for all invalid origins. */ }
  throw new Blocked("AI_GUARD_TEST_DASHBOARD doit être une origine loopback sans chemin ni credentials.");
}

async function readJson(origin, path) {
  const response = await fetch(`${origin}${path}`, {
    headers: { Authorization: `Bearer ${viewer}`, Accept: "application/json" },
    credentials: "omit", cache: "no-store", redirect: "error", signal: AbortSignal.timeout(15000),
  });
  if (response.status !== 200) {
    await response.body?.cancel();
    assert.fail(`Lecture viewer refusée ou indisponible (HTTP ${response.status}).`);
  }
  return response.json();
}

async function main() {
  const origin = dashboardOrigin();
  const requestedSeq = required("AI_GUARD_TEST_DECISION_SEQ");
  const seq = Number(requestedSeq);
  if (!/^[1-9]\d*$/u.test(requestedSeq) || !Number.isSafeInteger(seq + 1))
    throw new Blocked("AI_GUARD_TEST_DECISION_SEQ doit contenir un numéro de séquence positif.");
  const config = required("AI_GUARD_TEST_CONFIG");
  try { viewer = (await readFile(join(config, "secrets", "viewer"), "utf8")).trim(); }
  catch { throw new Blocked("Jeton viewer requis dans AI_GUARD_TEST_CONFIG/secrets/viewer."); }
  if (!viewer) throw new Blocked("Le jeton viewer de la recette est vide.");
  const expectedModel = process.env.AI_GUARD_TEST_EXPECTED_MODEL?.trim();

  stage = "journal de décision durable";
  const query = new URLSearchParams({
    kind: "security.decision", after: String(seq - 1), before: String(seq + 1), limit: "1",
  });
  const [overview, audit] = await Promise.all([
    readJson(origin, "/api/overview"), readJson(origin, `/api/events?${query}`),
  ]);
  assert.ok(overview?.health?.decision_engine === "local_openai", "Évaluateur local configuré attendu.");
  const event = audit?.items?.[0];
  assert.ok(audit?.items?.length === 1 && event.seq === seq && event.kind === "security.decision" &&
    typeof event.id === "string" && event.id.length > 0 && /^[a-f0-9]{64}$/u.test(event.hash) &&
    event.provenance === "observed" && event.source === "gateway" &&
    event.decision?.engine === "local_openai" && event.decision?.outcome === "deny",
  "Le journal doit contenir le refus observé du modèle local pour cette séquence.");
  const advice = event.data?.jev;
  const labels = new Map([["allow", "Favorable"], ["deny", "Défavorable"], ["review", "À confirmer"]]);
  assert.ok(advice?.backend === "local_openai" && advice.status === "evaluated" &&
    advice.qualification === "advisory_only" && labels.has(advice.provider_outcome) &&
    typeof advice.requested_model === "string" && advice.requested_model.trim() &&
    typeof advice.model === "string" && advice.model.trim() &&
    (!expectedModel || advice.model === expectedModel), "Évaluation et modèle retourné attendus dans l’audit.");

  stage = "Chromium local";
  let chromium;
  try {
    try { ({ chromium } = require("playwright")); }
    catch { ({ chromium } = require(join(required("CODEX_PRIMARY_RUNTIME_NODE_MODULES"), "playwright"))); }
    browser = await chromium.launch({
      headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
    });
  } catch { throw new Blocked("Playwright et un exécutable Chromium utilisable sont requis."); }
  context = await browser.newContext({ serviceWorkers: "block" });
  context.setDefaultTimeout(20000);
  guard = await installBrowserGuard(context, [origin], [viewer]);
  page = await context.newPage();
  await guard.preparePage(page);
  const pageErrors = [];
  page.on("pageerror", () => pageErrors.push(true));

  stage = "évaluateur configuré dans le dashboard";
  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await page.getByLabel("Jeton de lecture", { exact: true }).fill(viewer);
  await page.getByRole("button", { name: "Ouvrir la supervision", exact: true }).click();
  await page.getByRole("heading", { name: "Vue d’ensemble", exact: true }).waitFor();
  const evaluator = page.locator(".health-row").filter({
    has: page.getByText("Évaluateur de politique configuré", { exact: true }),
  });
  await evaluator.getByText("Modèle local", { exact: true }).waitFor();
  assert.equal(await evaluator.getByText("Disponible", { exact: true }).count(), 0);

  stage = "détail de la même décision";
  await page.getByRole("link", { name: "Sécurité", exact: true }).click();
  await page.getByLabel("Rechercher dans les événements", { exact: true }).fill(event.id);
  const filtering = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.pathname === "/api/events" && url.searchParams.get("q") === event.id;
  });
  await page.getByRole("button", { name: "Filtrer", exact: true }).click();
  assert.equal((await filtering).status(), 200);
  await page.getByRole("button", { name: `Voir l’événement ${seq}`, exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("heading", { name: "security.decision", exact: true }).waitFor();
  await dialog.getByText("Refusé", { exact: true }).waitFor();
  await dialog.getByText("Avis consultatif", { exact: true }).waitFor();
  for (const [label, text] of [
    ["Moteur de la décision", "Modèle local"], ["Évaluateur", "Modèle local"],
    ["État de l’avis", "Évalué"], ["Modèle demandé", advice.requested_model],
    ["Modèle retourné", advice.model], ["Avis du modèle", labels.get(advice.provider_outcome)],
  ]) {
    const field = dialog.locator(".key-value").filter({ has: page.getByText(label, { exact: true }) });
    await field.getByText(text, { exact: true }).waitFor();
  }
  await dialog.getByRole("tab", { name: "Données brutes", exact: true }).click();
  assert.ok(isDeepStrictEqual(JSON.parse(await dialog.locator("pre").textContent()), event),
    "Le détail brut doit restituer exactement l’événement durable.");
  assert.equal(pageErrors.length, 0, "Exception JavaScript dans le parcours.");
  return {
    status: "PASS", scope: "durable_viewer_read", seq, eventId: event.id, auditHash: event.hash,
    engine: advice.backend, returnedModel: advice.model,
    advisoryOutcome: advice.provider_outcome, effectiveOutcome: event.decision.outcome,
  };
}

let result;
try {
  result = await main();
} catch (error) {
  const status = error instanceof Blocked ? "BLOCKED" : "FAIL";
  const message = error instanceof Blocked || error instanceof assert.AssertionError ? error.message :
    error.name === "TimeoutError" ? "Le contrôle de cette étape a expiré." : "Lecture ou contrôle navigateur impossible.";
  result = { status, stage, message };
  process.exitCode = status === "BLOCKED" ? 2 : 1;
  if (page && !page.isClosed()) {
    const directory = await mkdtemp(join(tmpdir(), "ai-guard-browser-judge-")).catch(() => null);
    const capture = await saveBrowserFailure(page, { directory, secrets: [viewer] });
    if (capture) result.artifacts = { ...capture, directory: join(directory, capture.directory) };
  }
} finally {
  const cleanup = await closeBrowserResources(context, browser);
  if (cleanup.length || guard?.violations.length) {
    result = { ...result, status: "FAIL", stage, message: "Échec de la fermeture ou du garde réseau." };
    process.exitCode = 1;
  }
}
let report = JSON.stringify(result);
for (const secret of viewer ? [viewer, encodeURIComponent(viewer), JSON.stringify(viewer).slice(1, -1)] : [])
  report = report.split(secret).join("[REDACTED]");
process.stdout.write(`${report}\n`);
