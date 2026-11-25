// Contrôle explicite d'une installation QA réelle. Aucun serveur de fixtures.
// Les deux événements injectés sont déclarés QA, jamais des effets runtime.
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { readFile, mkdtemp } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { saveBrowserFailure } from "./browser_failure.mjs";

const require = createRequire(import.meta.url);
const secrets = [];
const checks = [];
let stage = "configuration";
let browser;
let context;
let page;
let captureFailure;
let artifacts;
let networkViolations = [];

class Blocked extends Error {}

function redact(value) {
  let text = String(value);
  for (const secret of secrets)
    for (const form of [secret, encodeURIComponent(secret), JSON.stringify(secret).slice(1, -1)])
      text = text.split(form).join("[REDACTED]");
  return text;
}

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Blocked(`${name} est requis explicitement.`);
  return value;
}

function localEndpoint(name) {
  let url;
  try {
    url = new URL(required(name));
  } catch (error) {
    if (error instanceof Blocked) throw error;
    throw new Blocked(`${name} doit être une URL locale valide.`);
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) ||
    url.username || url.password || url.search || url.hash ||
    url.pathname !== "/"
  )
    throw new Blocked(`${name} doit désigner une origine loopback sans credentials ni chemin.`);
  return url.origin;
}

function hash(value) {
  return createHash("sha256").update(value).digest("hex");
}

// Garde de l'outil de qualification, installé avant toute page. Aucun header,
// chemin ou URL interdite n'est conservé dans le diagnostic.
export async function installBrowserGuard(browserContext, origins, credentials = []) {
  const allowed = new Set(origins);
  const violations = [];
  let guardedPage;
  if (typeof browserContext.routeWebSocket !== "function")
    throw new Blocked("Playwright avec routeWebSocket est requis pour borner le réseau du test.");
  await browserContext.routeWebSocket("**/*", async (socket) => {
    violations.push({ reason: "websocket", method: "WS" });
    await socket.close(); // Aucun connectToServer : aucune connexion réseau.
  });
  await browserContext.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    let reason;
    let requestPage;
    let requestFrame;
    try {
      requestFrame = request.frame();
      requestPage = requestFrame.page();
    } catch { /* Worker sans page : refus. */ }
    if (!guardedPage || requestPage !== guardedPage) reason = "page";
    else if (requestFrame !== guardedPage.mainFrame()) reason = "frame";
    else if (!allowed.has(url.origin)) reason = "origin";
    else if (url.username || url.password) reason = "url-credentials";
    else if (credentials.filter(Boolean).some((token) =>
      [token, encodeURIComponent(token)].some((form) => request.url().includes(form))))
      reason = "token-in-url";
    else if (request.method() !== "GET") reason = "method";
    if (reason) {
      violations.push({ reason, method: request.method() });
      await route.abort("blockedbyclient");
    } else await route.continue();
  });
  return {
    violations,
    async preparePage(candidate) {
      if (guardedPage) throw new Error("Une seule page est autorisée dans le parcours live.");
      let session;
      try {
        session = await browserContext.newCDPSession(candidate);
        // Playwright ne rappelle pas route() pour chaque saut de redirection.
        // Intercepter la réponse avant tout suivi, sans tamponner le corps SSE.
        session.on("Fetch.requestPaused", async (event) => {
          try {
            if (event.responseStatusCode >= 300 && event.responseStatusCode < 400) {
              violations.push({ reason: "redirect", method: event.request.method });
              await session.send("Fetch.failRequest", {
                requestId: event.requestId, errorReason: "BlockedByClient",
              });
            } else await session.send("Fetch.continueRequest", { requestId: event.requestId });
          } catch {
            // Une requête déjà avortée peut avoir perdu son InterceptionId.
            // Elle n'a ni réponse HTTP à suivre ni corps à transmettre.
            if (!event.responseErrorReason && !candidate.isClosed())
              violations.push({ reason: "interception-error", method: "CDP" });
          }
        });
        await session.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Response" }] });
      } catch {
        throw new Blocked("Interception des redirections Chromium indisponible ; parcours refusé.");
      }
      guardedPage = candidate;
    },
  };
}

export async function interruptBrowserTransport(browserContext, targetPage) {
  await browserContext.setOffline(true);
  const session = await browserContext.newCDPSession(targetPage);
  try {
    // Le mode offline seul ne termine pas un fetch SSE déjà ouvert sur Chrome.
    await session.send("Page.stopLoading");
  } finally {
    await session.detach();
  }
}

export async function closeBrowserResources(browserContext, browserInstance) {
  const failures = [];
  try { await browserContext?.close(); } catch (error) { failures.push(redact(error.message)); }
  try {
    await browserInstance?.close();
    assert.ok(!browserInstance?.isConnected(), "Le navigateur doit être fermé");
  } catch (error) { failures.push(redact(error.message)); }
  return failures;
}

function securityHeaders(headers) {
  assert.match(headers.get("cache-control") ?? "", /\bno-store\b/i);
  const csp = headers.get("content-security-policy") ?? "";
  for (const directive of [
    "default-src 'self'", "script-src 'self'", "object-src 'none'",
    "frame-ancestors 'none'", "base-uri 'none'",
  ]) assert.ok(csp.includes(directive), `Directive CSP manquante : ${directive}`);
  assert.ok(!/unsafe-inline|unsafe-eval/.test(csp), "CSP sans code inline permissif");
  assert.equal(headers.get("x-frame-options"), "DENY");
  assert.equal(headers.get("x-content-type-options"), "nosniff");
}

async function request(origin, path, { token, method = "GET", event, status = 200 } = {}) {
  const headers = { Accept: "application/json" };
  if (token) headers.Authorization = `Bearer ${token}`;
  if (event) headers["Content-Type"] = "application/json";
  const response = await fetch(`${origin}${path}`, {
    method, headers, body: event ? JSON.stringify(event) : undefined,
    credentials: "omit", cache: "no-store", redirect: "error",
    signal: AbortSignal.timeout(15000),
  });
  try {
    assert.equal(response.status, status, `${method} ${path} : statut attendu`);
    securityHeaders(response.headers);
    return { body: await response.text(), headers: response.headers };
  } finally {
    if (!response.bodyUsed) await response.body?.cancel();
  }
}

async function check(name, action) {
  stage = name;
  assert.deepEqual(networkViolations, [], "Tentative réseau navigateur hors politique");
  await action();
  assert.deepEqual(networkViolations, [], "Tentative réseau navigateur hors politique");
  checks.push({ name, status: "PASS" });
}

async function storageEmpty() {
  const state = await page.evaluate(async () => ({
    local: localStorage.length,
    session: sessionStorage.length,
    databases: typeof indexedDB.databases === "function"
      ? (await indexedDB.databases()).length : null,
    cookies: document.cookie.length,
  }));
  assert.deepEqual(state, { local: 0, session: 0, databases: 0, cookies: 0 });
  assert.equal((await context.cookies()).length, 0);
}

async function main() {
  const gateway = localEndpoint("AI_GUARD_TEST_GATEWAY");
  const dashboard = localEndpoint("AI_GUARD_TEST_DASHBOARD");
  const config = required("AI_GUARD_TEST_CONFIG");
  const expectedWorkerId = required("AI_GUARD_TEST_WORKER_ID");
  assert.match(expectedWorkerId, /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/);
  const dashboardMode = process.env.AI_GUARD_TEST_DASHBOARD_MODE ?? "fixed";
  assert.ok(["fixed", "shared"].includes(dashboardMode), "Mode dashboard fixed ou shared explicite");
  let workerProof;
  let viewer;
  let runner;
  try {
    viewer = (await readFile(join(config, "secrets", "viewer"), "utf8")).trim();
    runner = (await readFile(join(config, "secrets", "runner"), "utf8")).trim();
  } catch {
    throw new Blocked("Fichiers secrets/viewer et secrets/runner requis dans AI_GUARD_TEST_CONFIG.");
  }
  secrets.push(...[viewer, runner].filter(Boolean));
  if (!viewer || !runner || viewer === runner)
    throw new Blocked("Les credentials QA doivent être non vides et distincts.");

  // Aucun jeton runner n'est confié à Playwright ou au navigateur.
  await check("services HTTP réels", async () => {
    for (const [origin, service] of [[gateway, "gateway"], [dashboard, "dashboard"]]) {
      let result;
      try {
        result = await request(origin, "/healthz");
      } catch (error) {
        if (error instanceof TypeError || error.name === "TimeoutError")
          throw new Blocked(`Service ${service} inaccessible sur l'origine explicite.`);
        throw error;
      }
      assert.equal(JSON.parse(result.body).service, service);
    }
  });

  const invalid = `invalid-qa-${randomUUID()}`;
  secrets.push(invalid);
  const project = `qa-browser-${randomUUID()}`;
  const malicious = '<img src=x onerror="window.__aiGuardLiveXss=true">';
  const firstMessage = `${project} SSE ${malicious}`;
  const secondMessage = `${project} reprise SSE déclarée QA`;
  const event = (message) => ({
    id: randomUUID(), timestamp: new Date().toISOString(),
    kind: "qa.browser.live", source: "qa.browser.live",
    provenance: "declared", coverage: "generic",
    project_id: project, session_id: project,
    data: { message, purpose: "Contrôle QA navigateur ; aucun effet runtime ou fournisseur attesté." },
  });
  const first = event(firstMessage);
  const second = event(secondMessage);
  const query = `project_id=${encodeURIComponent(project)}`;

  await check("refus serveur anonyme, invalide, runner et écritures viewer", async () => {
    for (const origin of [gateway, dashboard]) {
      await request(origin, "/api/overview", { status: 401 });
      await request(origin, "/api/overview", { token: invalid, status: 401 });
      await request(origin, "/api/overview", { token: viewer });
    }
    await request(dashboard, "/api/overview", { token: runner, status: dashboardMode === "shared" ? 403 : 401 });
    await request(gateway, "/api/events", { token: viewer, method: "POST", event: first, status: 403 });
    await request(dashboard, "/api/events", { token: viewer, method: "POST", event: first, status: 405 });
    const result = await request(gateway, `/api/events?${query}`, { token: viewer });
    assert.equal(JSON.parse(result.body).items.length, 0, "Aucune écriture après les refus");
  });

  await check("bundle compilé servi par le dashboard", async () => {
    const root = fileURLToPath(new URL("..", import.meta.url));
    const html = await request(dashboard, "/");
    const localHtml = await readFile(join(root, "dist", "index.html"));
    assert.equal(hash(html.body), hash(localHtml), "HTML servi égal au build local attendu");
    const asset = html.body.match(/src="(\/assets\/[^"/]+\.js)"/u)?.[1];
    assert.ok(asset, "Bundle de production référencé");
    const served = await request(dashboard, asset);
    const expected = await readFile(join(root, "dist", asset.slice(1)));
    assert.equal(hash(served.body), hash(expected), "Bundle servi égal au build local attendu");
  });

  let chromium;
  try {
    ({ chromium } = require("playwright"));
  } catch {
    const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
    if (!modules) throw new Blocked("Playwright est requis hors des dépendances du produit.");
    try { ({ chromium } = require(join(modules, "playwright"))); }
    catch { throw new Blocked("Playwright introuvable dans CODEX_PRIMARY_RUNTIME_NODE_MODULES."); }
  }
  stage = "démarrage navigateur";
  artifacts = await mkdtemp(join(tmpdir(), "ai-guard-browser-live-"));
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
    args: ["--no-sandbox"],
  });
  context = await browser.newContext({
    viewport: { width: 1440, height: 1050 }, reducedMotion: "reduce",
    serviceWorkers: "block",
  });
  const guard = await installBrowserGuard(context, [dashboard], [viewer, runner, invalid]);
  networkViolations = guard.violations;
  context.setDefaultTimeout(20000);
  page = await context.newPage();
  await guard.preparePage(page);
  // Only the two credentials used in the dashboard reach the capture helper.
  captureFailure = () => page.url() === `${dashboard}/`
    ? saveBrowserFailure(page, {
        directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR,
        secrets: [viewer, invalid],
      })
    : null;
  const requests = [];
  const errors = [];
  const streamResponses = [];
  page.on("pageerror", (error) => errors.push(redact(error.message)));
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (!url.pathname.startsWith("/api/")) return;
    const auth = request.headers().authorization;
    requests.push({
      path: redact(url.pathname + url.search), method: request.method(),
      auth: !auth ? "none" : auth === `Bearer ${viewer}` ? "viewer"
        : auth === `Bearer ${invalid}` ? "invalid" : "other",
      runner: auth === `Bearer ${runner}`,
      tokenInUrl: [viewer, runner].some((token) =>
        [token, encodeURIComponent(token)].some((form) => request.url().includes(form))),
      cursor: request.headers()["last-event-id"],
    });
  });
  page.on("response", (response) => {
    if (new URL(response.url()).pathname === "/api/stream")
      streamResponses.push(response);
  });

  await check("connexion UI, mémoire seule et proxy authentifié", async () => {
    await page.goto(dashboard, { waitUntil: "networkidle" });
    await page.getByLabel("Jeton de lecture").waitFor();
    assert.equal(requests.length, 0, "Aucune lecture authentifiée avant connexion");
    await page.getByLabel("Jeton de lecture").fill(invalid);
    await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
    await page.getByRole("alert").filter({ hasText: /Jeton refusé/ }).waitFor();
    assert.equal(await page.getByLabel("Jeton de lecture").inputValue(), "");
    await page.getByLabel("Jeton de lecture").fill(viewer);
    await page.getByRole("button", { name: "Ouvrir la supervision" }).click();
    await page.getByRole("heading", { name: "Vue d’ensemble", exact: true }).waitFor();
    await page.locator('[data-stream="live"]').waitFor();
    const filtered = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return url.pathname === "/api/events" && url.searchParams.get("project_id") === project;
    });
    await page.getByLabel("Identifiant du projet").fill(project);
    await page.getByRole("button", { name: "Filtrer", exact: true }).click();
    assert.equal((await filtered).status(), 200);
    await page.getByText("Aucun événement correspondant", { exact: true }).waitFor();
    await storageEmpty();
  });

  let firstStored;
  let secondStored;
  const historyReads = () => requests.filter(({ path }) => path.startsWith("/api/events?")).length;
  const initialReads = historyReads();
  await check("SSE réel et XSS rendu comme texte déclaré", async () => {
    const result = await request(gateway, "/api/events", { token: runner, method: "POST", event: first });
    firstStored = JSON.parse(result.body);
    assert.ok(Number.isSafeInteger(firstStored.seq));
    assert.equal(firstStored.provenance, "declared");
    await page.getByText(firstMessage, { exact: true }).waitFor();
    assert.equal(historyReads(), initialReads, "Événement reçu sans relecture d'historique");
    assert.equal(await page.locator("#app img").count(), 0);
    assert.equal(await page.evaluate(() => window.__aiGuardLiveXss), undefined);
    const row = page.locator("tr").filter({ hasText: firstMessage });
    assert.ok((await row.innerText()).includes("Déclaré"));
  });

  await check("reconnexion SSE avec Last-Event-ID et rattrapage réel", async () => {
    await interruptBrowserTransport(context, page);
    await page.locator('[data-stream="retrying"]').waitFor();
    const result = await request(gateway, "/api/events", { token: runner, method: "POST", event: second });
    secondStored = JSON.parse(result.body);
    const resumed = page.waitForRequest((request) =>
      new URL(request.url()).pathname === "/api/stream" &&
      Number(request.headers()["last-event-id"]) >= firstStored.seq);
    await context.setOffline(false);
    await resumed;
    await page.locator('[data-stream="live"]').waitFor();
    await page.getByText(secondMessage, { exact: true }).waitFor();
    assert.equal(await page.getByText(firstMessage, { exact: true }).count(), 1);
    assert.equal(await page.getByText(secondMessage, { exact: true }).count(), 1);
    assert.equal(historyReads(), initialReads, "Rattrapage effectué par SSE, sans historique");
    assert.ok(streamResponses.length >= 2);
    for (const response of streamResponses) {
      assert.equal(response.status(), 200);
      const headers = new Headers(await response.allHeaders());
      securityHeaders(headers);
      assert.match(headers.get("content-type") ?? "", /^text\/event-stream/);
    }
  });

  let exportDigest;
  await check("téléchargement export cohérent avec la gateway", async () => {
    const download = page.waitForEvent("download");
    await page.getByRole("button", { name: "Exporter JSONL" }).click();
    const completed = await download;
    assert.equal(await completed.failure(), null);
    assert.ok(completed.suggestedFilename().endsWith(".jsonl"));
    const bytes = await readFile(await completed.path());
    assert.ok(![viewer, runner].some((token) =>
      [token, encodeURIComponent(token)].some((form) => bytes.includes(Buffer.from(form)))),
    "Aucun credential brut ou encodé dans l'export avant sa conservation");
    const exported = bytes.toString("utf8").trim().split("\n").filter(Boolean).map(JSON.parse);
    const result = await request(gateway, `/api/events?${query}&limit=100`, { token: viewer });
    const expected = JSON.parse(result.body).items.sort((a, b) => a.seq - b.seq);
    assert.equal(exported.length, 2, "Seuls les deux événements QA du projet sont exportés");
    assert.ok(isDeepStrictEqual(exported, expected),
      "Export identique à la gateway, indépendamment de l'ordre des clés JSON");
    assert.ok(exported.every((item) => item.provenance === "declared" && item.source === "qa.browser.live"));
    exportDigest = hash(bytes);
    await completed.saveAs(join(artifacts, "declared-qa-export.jsonl"));
  });

  await check("workers, quotas et allocations réels dans le dashboard", async () => {
    const direct = JSON.parse((await request(gateway, "/api/v1/workers?limit=50", { token: viewer })).body);
    const proxied = JSON.parse((await request(dashboard, "/api/v1/workers?limit=50", { token: viewer })).body);
    assert.ok(isDeepStrictEqual(proxied, direct), "Registre relayé identique au gateway authentifié");
    const worker = direct.items.find((item) => item.worker_id === expectedWorkerId);
    if (!worker) throw new Blocked("Le worker explicite doit être présent dans la première page du registre : exécuter runtime_smoke sur ce gateway avant le navigateur.");
    assert.equal(worker.state, "terminal");
    assert.ok(worker.attempt.container_id);
    await page.getByRole("link", { name: "Workers", exact: true }).click();
    const row = page.locator(".control-view tbody tr").filter({ hasText: worker.worker_id });
    await row.waitFor();
    await row.getByRole("button", { name: "Détails", exact: true }).click();
    const dialog = page.locator("dialog[open]");
    const container = dialog.locator(".key-value").filter({
      has: page.getByText("Conteneur", { exact: true }),
    });
    await container.getByText(worker.attempt.container_id, { exact: true }).waitFor();
    assert.equal(JSON.parse(await dialog.locator("pre").textContent()).worker_id, worker.worker_id);
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    const quota = JSON.parse((await request(gateway, "/api/v1/quota", { token: viewer })).body);
    await page.getByRole("link", { name: "Quotas", exact: true }).click();
    const reserved = page.locator(".quota-row").filter({ has: page.getByText("Workers actifs réservés", { exact: true }) });
    await reserved.waitFor();
    assert.equal(await reserved.locator("dd strong").textContent(), String(quota.held_workers));
    const allocations = JSON.parse((await request(gateway, "/api/v1/usage?limit=50", { token: viewer })).body);
    const allocation = allocations.items.find((item) => item.worker_id === worker.worker_id);
    assert.ok(allocation, "Allocation de ce worker réellement enregistrée");
    assert.equal(allocation.billable, false);
    assert.equal(allocation.source.provenance, "observed");
    assert.equal(allocation.source.container_id, worker.attempt.container_id);
    await page.getByRole("link", { name: "Allocations", exact: true }).click();
    const usage = page.locator(".control-view tbody tr").filter({ hasText: worker.worker_id });
    await usage.waitFor();
    await usage.getByText(allocation.attempt_id, { exact: true }).waitFor();
    await page.getByText(/Méthode 1 · non facturable/).waitFor();
    workerProof = { workerId: worker.worker_id, attemptId: worker.attempt.attempt_id,
      containerId: worker.attempt.container_id, allocationObservationId: allocation.terminal_observation_id,
      provenance: allocation.source.provenance, billable: allocation.billable };
  });

  await check("déconnexion, actualisation et absence de persistance du jeton", async () => {
    await page.getByRole("button", { name: "Se déconnecter" }).click();
    await page.getByLabel("Jeton de lecture").waitFor();
    await storageEmpty();
    const beforeReload = requests.length;
    await page.reload({ waitUntil: "networkidle" });
    await page.getByLabel("Jeton de lecture").waitFor();
    assert.equal(await page.getByLabel("Jeton de lecture").inputValue(), "");
    assert.equal(requests.length, beforeReload, "Aucune reconnexion authentifiée après actualisation");
    assert.ok(requests.every(({ method, auth, runner, tokenInUrl }) =>
      method === "GET" && auth !== "other" && !runner && !tokenInUrl));
    assert.deepEqual(errors, [], "Aucune exception JavaScript navigateur");
    assert.deepEqual(networkViolations, [], "Aucune tentative réseau interdite");
  });

  return { status: "PASS", node: process.version, browser: browser.version(),
    gateway, dashboard, dashboardMode, checks, artifacts, worker: workerProof,
    qa: { project, eventIds: [first.id, second.id], sequences: [firstStored.seq, secondStored.seq],
      provenance: "declared", source: "qa.browser.live", exportSha256: exportDigest },
    scope: "Dashboard Rust et gateway réels ; injection QA déclarée ; aucun effet runtime/fournisseur attesté." };
}

async function run() {
  let result;
  try {
    result = await main();
  } catch (error) {
    result = { status: error instanceof Blocked ? "BLOCKED" : "FAIL", stage,
      message: redact(error.message), checks, artifacts, networkViolations };
    process.exitCode = error instanceof Blocked ? 2 : 1;
  } finally {
    if (result.status !== "PASS" && networkViolations.length === 0) {
      const failureArtifacts = await captureFailure?.();
      if (failureArtifacts) result = { ...result, failureArtifacts };
    }
    // Même un échec après lancement ou une coupure réseau ferme les flux et Chromium.
    const cleanup = await closeBrowserResources(context, browser);
    if (cleanup.length) {
      result = { ...result, status: "FAIL", cleanup };
      process.exitCode = 1;
    } else if (browser) checks.push({ name: "fermeture contexte et navigateur", status: "PASS" });
    // Inclut les éventuelles tentatives pagehide/unload pendant la fermeture.
    if (networkViolations.length) {
      result = { ...result, status: "FAIL", networkViolations };
      process.exitCode = 1;
    }
  }
  process.stdout.write(redact(JSON.stringify(result, null, 2)) + "\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  await run();
