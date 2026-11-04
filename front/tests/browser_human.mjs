// Product UI regressions with isolated fixtures; never imported by the application.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir, mkdtemp } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { identity, page as envelope, resource, event, uuid } from "./human-fixtures.mjs";
import { brokerIdentity, brokerPage, denseBrokerItems } from "./human-fixtures.mjs";

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require("playwright")); }
catch { ({ chromium } = require(join(process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES, "playwright"))); }
const root = fileURLToPath(new URL("..", import.meta.url));
const scenario = process.env.AI_GUARD_HUMAN_SCENARIO || "full";
const bearer = "fixture-human-secret-123456789";
const viewerBearer = "fixture-viewer-secret-123456789";
let revoked = false; let authDenied = false; let detailDenied = false; let resourcesDown = false;
let viewerDenied = null;
let brokersDown = false; let brokerDenied = false; let budgetExhausted = false; let brokerCapRemoved = false;
let brokerDisplay = "standard"; let credentialExpiresAt = null;
const humanIdentity = () => {
  const value = scenario.startsWith("brokers") ? brokerIdentity(scenario === "brokers-mixed") : identity();
  if (brokerCapRemoved) value.contexts[0].capabilities = value.contexts[0].capabilities.filter((cap) => cap !== "read_broker_registry");
  if (scenario === "brokers-lifecycle") value.authentication.expires_at = credentialExpiresAt;
  return value;
};
const requests = []; const streams = new Map();
const json = (response, value, status = 200) => { response.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" }); response.end(JSON.stringify(value)); };
const error = (response, status, code, retry_action) => json(response, { error: { code, message: `Refus ${code}`, retry_action } }, status);
const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  if (url.pathname.startsWith("/api/")) {
    const contextId = request.headers["x-aiguard-context"];
    requests.push({ path: request.url, method: request.method, context: contextId,
      authorized: [bearer, viewerBearer].some((value) => request.headers.authorization === `Bearer ${value}`), lastEventId: request.headers["last-event-id"] });
    if (!url.pathname.startsWith("/api/v1/human/")) {
      if (request.headers.authorization !== `Bearer ${viewerBearer}` || viewerDenied) { response.writeHead(viewerDenied || 401); response.end(); return; }
      if (url.pathname === "/api/v1/identity") return json(response, { tenant_id: "legacy-tenant", role: "viewer" });
      if (url.pathname === "/api/overview") return json(response, { counts: { agents: 0 } });
      if (["/api/agents", "/api/sandboxes"].includes(url.pathname)) return json(response, { items: [] });
      if (url.pathname === "/api/events") return json(response, { items: [], next_cursor: null });
      if (url.pathname === "/api/integrity") return json(response, { valid: true });
      if (url.pathname === "/api/stream") {
        response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": ready\n\n"); return;
      }
      response.writeHead(404); response.end(); return;
    }
    if (request.headers.authorization !== `Bearer ${bearer}` || authDenied) return error(response, 401, "credential_invalid", "authenticate");
    if (url.pathname === "/api/v1/human/identity") {
      const value = humanIdentity(); if (revoked) value.contexts = value.contexts.filter((context) => context.id !== "context-admin");
      return json(response, value);
    }
    if (!String(url.pathname).startsWith("/api/v1/human/")) { response.writeHead(404); response.end(); return; }
    if (!humanIdentity().contexts.some((context) => context.id === contextId)) return error(response, 403, "context_invalid", "select_context");
    if (budgetExhausted) return error(response, 429, "audit_read_budget_exhausted", "contact_operator");
    if (url.pathname === "/api/v1/human/brokers") {
      if (budgetExhausted) return error(response, 429, "audit_read_budget_exhausted", "contact_operator");
      if (brokerDenied || brokerCapRemoved) return error(response, 403, "permission_denied", "review_permissions");
      if (brokersDown) return error(response, 503, "audit_unavailable", "retry");
      if (brokerDisplay === "empty") return json(response, brokerPage([]));
      if (brokerDisplay === "dense") return json(response, brokerPage(denseBrokerItems()));
      return json(response, brokerPage(undefined, url.searchParams.has("cursor") ? {} : { has_more: true, next_cursor: "broker-next-cursor" }));
    }
    if (contextId === "context-admin" && revoked) return error(response, 403, "context_revoked", "select_context");
    const resourceId = contextId === "context-b" ? "session-b" : "session-a";
    const permissions = contextId === "context-a" ? { metadata: true, content: false, export: false } : { metadata: true, content: true, export: true };
    if (url.pathname === "/api/v1/human/resources") {
      if (resourcesDown) return error(response, 503, "audit_unavailable", "retry");
      return json(response, envelope(contextId, [resource(resourceId, permissions)]));
    }
    if (url.pathname === "/api/v1/human/stream") {
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" }); response.write(": ready\n\n");
      streams.set(contextId, response); request.on("close", () => { if (streams.get(contextId) === response) streams.delete(contextId); }); return;
    }
    if (url.pathname === `/api/v1/human/events/${uuid}` && detailDenied) return error(response, 403, "permission_denied", "review_permissions");
    if (["/api/v1/human/events", `/api/v1/human/events/${uuid}`, "/api/v1/human/export"].includes(url.pathname)) {
      const view = url.searchParams.get("view");
      if (view === "content" && !permissions.content || url.pathname.endsWith("/export") && !permissions.export) return error(response, 403, "permission_denied", "review_permissions");
      const exporting = url.pathname.endsWith("/export");
      const item = event(view, resourceId, { message: resourceId === "session-a" ? "AUTHORIZED_CONTENT_A" : "AUTHORIZED_CONTENT_B", echo: bearer });
      const partial = exporting && !url.searchParams.has("cursor");
      return json(response, envelope(contextId, [item], { view, next_cursor: partial ? "export-next-opaque" : null, has_more: partial,
        ...(exporting ? { selection_complete: !partial } : {}) }));
    }
    response.writeHead(404); response.end(); return;
  }
  try {
    const path = url.pathname === "/" ? "index.html" : url.pathname.slice(1); if (path.includes("..")) throw new Error("Invalid path");
    const body = await readFile(join(root, "dist", path));
    response.writeHead(200, { "content-type": { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" }[extname(path)] || "application/octet-stream" }); response.end(body);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined, args: ["--no-sandbox"] });
const context = await browser.newContext({ viewport: { width: 1440, height: 1050 } });
const page = await context.newPage(); page.setDefaultTimeout(7000);
const errors = []; page.on("pageerror", (error) => errors.push(error.message));
let downloadCount = 0; page.on("download", () => downloadCount++);
await page.addInitScript(() => {
  const nativeFetch = window.fetch; window.__humanHold = false; window.__humanHeld = []; window.__humanFetches = [];
  const nativeInterval = window.setInterval;
  const nativeTimeout = window.setTimeout; window.__humanRefreshTimers = []; window.__humanExpiryTimers = [];
  window.setTimeout = (handler, delay, ...args) => {
    if (delay === 1000 && typeof handler === "function") window.__humanRefreshTimers.push(() => handler(...args));
    if (delay >= 90000 && delay <= 150000 && typeof handler === "function") window.__humanExpiryTimers.push(() => handler(...args));
    return nativeTimeout(handler, delay, ...args);
  };
  const nativeNow = Date.now; let advance = 0; Date.now = () => nativeNow() + advance;
  window.__humanAdvance = (milliseconds) => { advance += milliseconds; };
  window.__humanPoll = [];
  window.setInterval = (handler, delay, ...args) => {
    if (delay === 30000 && typeof handler === "function") window.__humanPoll.push(() => handler(...args));
    return nativeInterval(handler, delay, ...args);
  };
  window.__humanRelease = () => { window.__humanHeld.splice(0).forEach((release) => release()); };
  window.fetch = async (...args) => {
    window.__humanFetches.push(String(args[0]));
    const response = await nativeFetch(...args);
    if (window.__humanHold && (String(args[0]).includes("/human/events/") || String(args[0]).includes("/human/brokers"))) {
      window.__humanHold = false; const body = await response.text();
      await new Promise((release) => window.__humanHeld.push(release));
      return new Response(body, { status: response.status, headers: response.headers });
    }
    return response;
  };
});
const artifacts = process.env.AI_GUARD_HUMAN_ARTIFACTS || await mkdtemp(join(tmpdir(), "ai-guard-human-browser-"));
const choose = async (mode, tenant = "tenant-a") => page.getByRole("button", { name: `Ouvrir ${mode} · ${tenant}`, exact: true }).click();
const tab = async (name) => page.getByRole("button", { name, exact: true }).click();
const emit = (contextId, value) => streams.get(contextId)?.write(`event: error\ndata: ${JSON.stringify({ error: value })}\n\n`);
const settle = async () => page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
const loginHuman = async () => {
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer); await tab("Ouvrir mon espace");
  await page.getByRole("heading", { name: "Choisir un contexte." }).waitFor();
};
const captureBrokerState = async (label, widths, target) => {
  await mkdir(artifacts, { recursive: true });
  for (const width of widths) for (const theme of ["light", "dark"]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((value) => { document.documentElement.dataset.theme = value; window.scrollTo(0, 0); }, theme);
    const geometry = await page.evaluate(() => ({
      fits: document.documentElement.scrollWidth <= innerWidth,
      headingTop: document.querySelector(".human-page .page-heading").getBoundingClientRect().top,
      dockBottom: document.querySelector(".human-toolbar").getBoundingClientRect().bottom,
    }));
    assert.equal(geometry.fits, true, `${label} ${width}px ${theme} sans débordement global`);
    assert.ok(geometry.headingTop >= geometry.dockBottom - 1, `${label} ${width}px ${theme} sans contenu sous les commandes fixes`);
    if (target) await target().scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(artifacts, `human-brokers-${label}-${width}-${theme}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; window.scrollTo(0, 0); });
};
const runBrokerStates = async () => {
  brokerDisplay = "empty"; await loginHuman();
  await page.evaluate(() => { window.__humanHold = true; }); await choose("administration");
  await page.waitForFunction(() => window.__humanHeld.length > 0);
  await page.getByRole("heading", { name: "Chargement du registre…", exact: true }).waitFor();
  assert.equal(await page.getByRole("heading", { name: "Aucun broker enregistré à cette lecture", exact: true }).count(), 0, "Une réponse suspendue n’est pas une page vide");
  await page.evaluate(() => window.__humanRelease());
  await page.getByRole("heading", { name: "Aucun broker enregistré à cette lecture", exact: true }).waitFor();
  await page.getByText("Lecture réussie", { exact: true }).waitFor();
  assert.equal(await page.locator(".human-broker-table tbody tr").count(), 0);
  assert.ok(await page.getByText(/inventaire runtime inconnu/i).count(), "Le registre vide ne devient pas un inventaire runtime vide");
  await captureBrokerState("empty", [390, 320], () => page.getByRole("heading", { name: "Aucun broker enregistré à cette lecture", exact: true }));

  await tab("Contextes"); await page.getByRole("button", { name: /Ouvrir administration/ }).waitFor();
  brokersDown = true; await choose("administration");
  await page.getByRole("heading", { name: "Registre indisponible", exact: true }).waitFor();
  await page.getByText("Données indisponibles", { exact: true }).waitFor();
  await page.getByText("Aucune lecture réussie", { exact: true }).waitFor();
  assert.equal(await page.getByRole("heading", { name: "Aucun broker enregistré à cette lecture", exact: true }).count(), 0, "Un 503 initial n’est pas rendu comme un registre vide");
  assert.equal(await page.locator(".human-broker-table").count(), 0);
  await captureBrokerState("initial-unavailable", [390, 320], () => page.getByRole("heading", { name: "Registre indisponible", exact: true }));

  brokersDown = false; brokerDisplay = "dense"; await tab("Réessayer");
  await page.getByText("Lecture réussie", { exact: true }).waitFor();
  assert.equal(await page.locator(".human-broker-table tbody tr").count(), 25, "La page dense affiche les 25 lignes reçues, sans total extrapolé");
  for (const state of ["Heartbeat non reçu", "Bail courant", "Bail périmé", "Autorisation expirée", "Autorisation révoquée", "Horloge incertaine"])
    assert.ok(await page.getByText(state, { exact: true }).count(), `L’état ${state} est explicite`);
  assert.ok(await page.getByText("Identifiant masqué", { exact: true }).count());
  assert.ok(await page.getByText("Heartbeat d’une ancienne incarnation", { exact: true }).count());
  const oldIncarnation = page.locator('[data-focus-key="broker-detail-6"]');
  await oldIncarnation.click(); await page.getByRole("dialog").getByText(/ancienne incarnation/).waitFor();
  await page.keyboard.press("Escape");
  const focus = await page.evaluate(() => {
    const node = document.activeElement; const rect = node.getBoundingClientRect();
    return { key: node.getAttribute("data-focus-key"), connected: node.isConnected,
      visible: rect.top >= document.querySelector(".human-toolbar").getBoundingClientRect().bottom - 1 && rect.bottom <= innerHeight + 1 };
  });
  assert.deepEqual(focus, { key: "broker-detail-6", connected: true, visible: true }, "Fermer le détail dense restitue un focus connecté et visible");
  await page.locator(".human-broker-scroll").evaluate((node) => { node.scrollLeft = 0; });
  await captureBrokerState("dense", [1440, 390, 320], () => page.locator(".human-broker-table tbody tr").first().locator("td").first());
  assert.ok(!requests.some((request) => /\/human\/(resources|events|export|stream)/.test(request.path)), "Ces états broker seul ne déclenchent aucune autre lecture métier");
  await tab("Se déconnecter"); assert.equal(await page.locator(".human-shell, dialog").count(), 0);
};
const runBrokerLifecycle = async () => {
  for (const cause of ["identity-loss", "credential-expiry", "logout"]) {
    authDenied = false; credentialExpiresAt = cause === "credential-expiry" ? new Date(Date.now() + 120000).toISOString() : null;
    await loginHuman(); await choose("administration"); await page.getByText("Lecture réussie", { exact: true }).waitFor();
    await page.evaluate(() => { window.__humanHold = true; }); await tab("Actualiser");
    await page.waitForFunction(() => window.__humanHeld.length > 0);
    await tab("Détails du broker"); await page.getByRole("dialog").waitFor();
    if (cause === "identity-loss") {
      authDenied = true;
      await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
    } else if (cause === "credential-expiry") {
      await page.waitForFunction(() => window.__humanExpiryTimers.length > 0);
      // Invoke the real expiry callback captured from the client, after advancing only the fixture browser clock.
      await page.evaluate(() => { window.__humanAdvance(121000); window.__humanExpiryTimers.forEach((callback) => callback()); });
    } else {
      await page.keyboard.press("Escape"); await tab("Se déconnecter");
    }
    await page.getByLabel("Credential personnel local").waitFor();
    assert.equal(await page.getByLabel("Credential personnel local").inputValue(), "");
    assert.equal(await page.locator(".human-shell, dialog").count(), 0, `${cause} purge la page et le détail avant la réponse tardive`);
    const readsAfterClose = await page.evaluate(() => window.__humanFetches.length);
    await page.evaluate(() => {
      window.__humanRelease(); window.__humanPoll.forEach((callback) => callback());
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await settle();
    assert.equal(await page.evaluate(() => window.__humanHeld.length), 0, "La réponse suspendue a bien été relâchée");
    assert.equal(await page.evaluate(() => window.__humanFetches.length), readsAfterClose, `${cause} ne relance aucune lecture par un ancien callback`);
    assert.equal(await page.locator(".human-shell, .human-broker-table, dialog").count(), 0, `${cause} empêche toute résurrection après réception`);
    assert.equal(await page.getByText(uuid, { exact: true }).count(), 0);
    assert.equal(await page.getByLabel("Credential personnel local").inputValue(), "");
    assert.ok(!(await page.locator("body").innerText()).includes(bearer));
  }
  assert.ok(!requests.some((request) => /\/human\/(resources|events|export|stream)/.test(request.path)));
  assert.equal(downloadCount, 0);
};
const runRegression = async () => {
  if (scenario === "brokers-states") {
    await runBrokerStates();
  } else if (scenario === "brokers-lifecycle") {
    await runBrokerLifecycle();
  } else if (["brokers", "brokers-mixed"].includes(scenario)) {
    await loginHuman(); assert.equal(requests.length, 1, "La connexion ne lit que l’identité");
    await choose("administration");
    if (scenario === "brokers-mixed") {
      await page.getByText("session-a", { exact: true }).waitFor(); await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
      streams.get("context-brokers").write(`id: broker-old-resource-cursor\nevent: human_audit\ndata: ${JSON.stringify(envelope("context-brokers", [], { view: "metadata" }))}\n\n`);
      await page.getByText("Données à actualiser", { exact: true }).waitFor();
      await page.waitForFunction(() => window.__humanRefreshTimers.length > 0); await tab("Brokers");
    }
    await page.getByRole("heading", { name: "Brokers enregistrés du tenant." }).waitFor();
    await page.getByText("Lecture réussie", { exact: true }).waitFor();
    const brokerCalls = () => requests.filter((request) => request.path.includes("/human/brokers")).length;
    const otherCalls = () => requests.filter((request) => /\/human\/(resources|events|export|stream)/.test(request.path)).length;
    const otherBefore = otherCalls();
    await page.evaluate(() => window.__humanRefreshTimers.forEach((callback) => callback())); await settle();
    assert.equal(otherCalls(), otherBefore, "Un ancien callback SSE exécuté après changement de vue reste sans effet");
    await tab("Actualiser"); await page.getByText("Lecture réussie", { exact: true }).waitFor();
    await page.evaluate(() => window.__humanPoll.at(-1)()); await settle();
    await page.evaluate(() => { Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" }); document.dispatchEvent(new Event("visibilitychange")); });
    await page.evaluate(() => { Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" }); document.dispatchEvent(new Event("visibilitychange")); });
    await settle(); assert.equal(otherCalls(), otherBefore, "Brokers ne lit aucune ressource ni flux caché");
    assert.ok(await page.getByText(/inventaire runtime inconnu/i).count());
    await tab("Détails du broker"); await page.getByRole("dialog").getByText(/Heartbeat déclaré/).waitFor(); await page.keyboard.press("Escape");
    assert.equal(await page.evaluate(() => document.activeElement?.dataset.focusKey), "broker-detail-0");
    await tab("Page suivante"); await page.getByText(/page 2/).waitFor(); await tab("Page précédente");
    brokersDown = true; await tab("Actualiser"); await page.getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
    assert.equal(await page.getByRole("button", { name: "Détails du broker", exact: true }).count(), 1);
    brokersDown = false; await tab("Réessayer"); await page.getByText("Lecture réussie", { exact: true }).waitFor();
    await mkdir(artifacts, { recursive: true });
    for (const width of [1440, 390, 320]) for (const theme of ["light", "dark"]) {
      await page.setViewportSize({ width, height: 900 }); await page.evaluate((value) => { document.documentElement.dataset.theme = value; }, theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      await page.screenshot({ path: join(artifacts, `human-brokers-${scenario}-${width}-${theme}.png`) });
    }
    budgetExhausted = true; await tab("Actualiser"); await page.getByText(/Lecture automatique suspendue/).first().waitFor();
    const stopped = requests.length;
    await page.evaluate(() => { window.__humanAdvance(31000); window.__humanPoll.at(-1)(); document.dispatchEvent(new Event("visibilitychange")); });
    await settle(); assert.equal(requests.length, stopped, "Le budget durable arrête poll et retour de visibilité");
    await tab("Contextes"); await settle(); assert.equal(requests.length, stopped, "La navigation ne réarme pas le budget durable");
    budgetExhausted = false; await tab("Relire après intervention de l’opérateur");
    await page.getByRole("button", { name: /Ouvrir administration/ }).waitFor(); await choose("administration");
    if (scenario === "brokers-mixed") { await page.getByText("session-a", { exact: true }).waitFor(); await tab("Brokers"); }
    await page.getByText("Lecture réussie", { exact: true }).waitFor();
    brokerDenied = true; await tab("Actualiser"); await page.getByText("Refus permission_denied", { exact: false }).waitFor();
    assert.equal(await page.getByRole("button", { name: "Détails du broker", exact: true }).count(), 0);
    const deniedReads = brokerCalls(); await page.evaluate(() => { window.__humanAdvance(31000); window.__humanPoll.at(-1)(); }); await settle(); assert.equal(brokerCalls(), deniedReads);
    if (scenario === "brokers-mixed") {
      await tab("Ressources"); await page.getByText("session-a", { exact: true }).waitFor();
      const revalidated = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/identity");
      await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange"))); await revalidated;
      assert.equal(await page.getByRole("heading", { name: "Ressources autorisées." }).count(), 1, "Le refus broker n’empêche pas la revalidation d’une autre vue autorisée");
      budgetExhausted = true; await tab("Actualiser"); await page.getByText(/Lecture automatique suspendue/).first().waitFor();
      await tab("Brokers"); const beforeRecovery = requests.length; budgetExhausted = false;
      await tab("Relire après intervention de l’opérateur"); await page.getByRole("heading", { name: "Choisir un contexte." }).waitFor();
      await page.getByRole("button", { name: /Ouvrir administration/ }).waitFor();
      assert.deepEqual(requests.slice(beforeRecovery).map((request) => request.path), ["/api/v1/human/identity"], "La reprise d’un broker refusé relit seulement l’identité avant une nouvelle sélection explicite");
      await choose("administration"); await page.getByText("session-a", { exact: true }).waitFor(); await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
      for (const view of ["resources", "events"]) {
        if (view === "events") { await tab("Métadonnées"); await page.getByRole("heading", { name: "command.completed", exact: true }).waitFor(); }
        budgetExhausted = true; await tab("Actualiser"); await page.getByText(/Lecture automatique suspendue/).first().waitFor();
        budgetExhausted = false; await tab("Relire après intervention de l’opérateur");
        await page.getByText("Lecture réussie", { exact: true }).waitFor(); await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
        assert.equal(await page.getByText(/Lecture automatique suspendue/).count(), 0, `Le flux ${view} reprend seulement après lecture courante validée`);
      }
      brokerDenied = false; await tab("Brokers"); await page.getByText("Lecture réussie", { exact: true }).waitFor();
      brokerDenied = true; await tab("Actualiser"); await page.getByText("Refus permission_denied", { exact: false }).waitFor();
    }
    brokerDenied = false; await tab("Relire mes accès"); await choose("administration");
    if (scenario === "brokers-mixed") { await page.getByText("session-a", { exact: true }).waitFor(); await tab("Brokers"); }
    await page.getByText("Lecture réussie", { exact: true }).waitFor();
    await page.evaluate(() => { window.__humanHold = true; }); await tab("Actualiser"); await page.waitForFunction(() => window.__humanHeld.length > 0);
    await tab("Contextes"); brokerCapRemoved = true; await tab("Actualiser"); await settle();
    await page.evaluate(() => window.__humanRelease()); await settle();
    assert.equal(await page.getByRole("button", { name: "Brokers", exact: true }).count(), 0);
    assert.equal(await page.getByRole("button", { name: "Détails du broker", exact: true }).count(), 0);
    await tab("Se déconnecter"); assert.equal(await page.locator(".human-shell, dialog").count(), 0);
  } else if (scenario === "focus") {
    await loginHuman(); await choose("personnel"); await page.getByText("session-a", { exact: true }).waitFor();
    await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
    const findings = [];
    const inspect = async (phase) => {
      const focus = await page.evaluate(() => { const node = document.activeElement; return {
        connected: !!node?.isConnected, inside: !!document.querySelector(".human-shell")?.contains(node),
        usable: node instanceof HTMLElement && !node.matches(":disabled") && node.matches("button, a[href], [tabindex]"),
        tag: node?.tagName, key: node?.getAttribute("data-focus-key"),
      }; });
      if (!focus.connected || !focus.inside || !focus.usable) findings.push({ phase, focus });
    };
    const refresh = () => page.locator('[data-focus-key="human-refresh"]');
    await refresh().focus();
    const refreshed = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/resources");
    await page.keyboard.press("Enter"); await refreshed; await settle(); await inspect("keyboard refresh");
    await page.locator('[data-focus-key="human-nav-resources"]').focus();
    streams.get("context-a").write(`id: focus-confirmed-cursor\nevent: human_audit\ndata: ${JSON.stringify(envelope("context-a", [], { view: "metadata" }))}\n\n`);
    await page.getByText("Données à actualiser", { exact: true }).waitFor(); await inspect("SSE render");
    const pollRead = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/resources");
    await refresh().focus(); await page.evaluate(() => window.__humanPoll.at(-1)());
    await pollRead; await settle(); await inspect("poll callback");
    assert.deepEqual(findings, [], "Le remplacement des commandes doit conserver un focus connecté et utilisable");
  } else if (scenario === "access-modes") {
    await loginHuman(); await choose("personnel"); await page.getByText("session-a", { exact: true }).waitFor();
    await tab("Se déconnecter");
    await page.getByLabel("Type d’accès").selectOption("viewer");
    const beforeHuman = requests.filter((request) => request.path.startsWith("/api/v1/human/")).length;
    await page.getByLabel("Jeton de lecture").fill(bearer); await tab("Ouvrir la supervision");
    await page.locator(".login-error:not([hidden])").waitFor();
    assert.equal(await page.getByLabel("Jeton de lecture").inputValue(), "");
    assert.equal(requests.filter((request) => request.path.startsWith("/api/v1/human/")).length, beforeHuman, "Aucun fallback humain depuis une connexion technique refusée");
    const loginViewer = async () => {
      await page.getByLabel("Jeton de lecture").fill(viewerBearer); await tab("Ouvrir la supervision");
      await page.getByText("Client : legacy-tenant", { exact: true }).first().waitFor();
      await page.getByText("Flux connecté", { exact: true }).waitFor();
      assert.equal(await page.getByRole("link", { name: /^(Vue d’ensemble|Agents|Actions|Sécurité|Fichiers|Sandboxes|Workers|Quotas|Allocations)$/ }).count(), 9);
      assert.ok(!(await page.locator("body").innerText()).includes("Alice locale"));
    };
    await loginViewer();
    for (const status of [401, 403]) {
      viewerDenied = status; await tab("Actualiser"); await page.getByLabel("Jeton de lecture").waitFor();
      assert.equal(await page.locator(".app-shell, dialog").count(), 0, `Le refus viewer${status} retire la session`);
      assert.equal(await page.getByLabel("Jeton de lecture").inputValue(), "");
      viewerDenied = null; await loginViewer();
    }
    await tab("Se déconnecter");
  } else throw new Error(`Scénario inconnu : ${scenario}`);
  assert.deepEqual(errors, []);
  process.stdout.write(`Régression humaine ${scenario} PASS ; fixtures uniquement.\n`);
};
try {
  if (scenario !== "full") {
    await runRegression();
  } else {
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  assert.equal(await page.getByLabel("Type d’accès").inputValue(), "viewer", "Le mode technique conserve son parcours explicite");
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer); await tab("Ouvrir mon espace");
  await page.getByRole("heading", { name: "Choisir un contexte." }).waitFor();
  await page.getByText("Alice locale", { exact: false }).first().waitFor();
  assert.equal(await page.getByRole("button", { name: /Ouvrir administration/ }).count(), 1);
  await choose("personnel"); await page.getByText("session-a", { exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "Contenu", exact: true }).isDisabled(), true);
  await tab("Métadonnées"); await page.getByRole("heading", { name: "command.completed", exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "Exporter les métadonnées" }).isDisabled(), true);
  await tab("Détails"); await page.getByRole("dialog").getByText(/Métadonnées uniquement/).waitFor();
  assert.ok(!(await page.getByRole("dialog").innerText()).includes('"command"'));
  await page.keyboard.press("Escape"); await tab("Contextes"); await choose("administration");
  await page.getByText("session-a", { exact: true }).waitFor(); await tab("Contenu");
  await page.getByRole("heading", { name: "command.completed", exact: true }).waitFor();
  await tab("Détails"); await page.getByRole("dialog").getByText(/AUTHORIZED_CONTENT_A/).waitFor();
  assert.ok(!(await page.locator("body").innerText()).includes(bearer)); await page.keyboard.press("Escape");
  const downloaded = page.waitForEvent("download"); await tab("Exporter le contenu");
  const file = await downloaded; const path = await file.path(); const data = await readFile(path, "utf8");
  assert.ok(!data.includes(bearer)); const exported = JSON.parse(data);
  assert.equal(exported.selection_complete, true); assert.equal(exported.pages.length, 2); assert.equal(exported.selection.view, "content");
  assert.ok(requests.some((request) => request.path.includes("cursor=export-next-opaque") && request.context === "context-admin"));
  await mkdir(artifacts, { recursive: true });
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: width === 1440 ? 1050 : 844 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((value) => { document.documentElement.dataset.theme = value; }, theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${width}px ${theme} sans débordement`);
      await page.screenshot({ path: join(artifacts, `human-events-${width}-${theme}.png`) });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  // A closed detail's continuation arrives under a different context, after both transport and body were received.
  await page.evaluate(() => { window.__humanHold = true; }); await tab("Détails");
  await page.waitForFunction(() => window.__humanHeld.length > 0); await page.keyboard.press("Escape");
  await tab("Contextes"); await choose("personnel", "tenant-b");
  await page.getByText("session-b", { exact: true }).waitFor();
  await page.evaluate(async () => { window.__humanRelease(); await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))); });
  assert.equal(await page.locator("dialog[open]").count(), 0); assert.ok(!(await page.locator("body").innerText()).includes("AUTHORIZED_CONTENT_A"));
  resourcesDown = true; await tab("Actualiser"); await page.getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
  assert.equal(await page.getByText("session-b", { exact: true }).count(), 1, "Une panne garde les données avec leur état périmé");
  resourcesDown = false; await tab("Réessayer"); await page.getByText("Lecture réussie", { exact: true }).waitFor();
  await tab("Contenu"); await page.getByRole("heading", { name: "command.completed", exact: true }).waitFor();
  detailDenied = true; await tab("Détails"); await page.getByText("Refus permission_denied", { exact: true }).waitFor();
  assert.equal(await page.locator("dialog[open]").count(), 0); assert.equal(await page.getByRole("button", { name: "Se déconnecter", exact: true }).count(), 1);
  detailDenied = false;
  await tab("Contextes"); await choose("administration"); await page.getByText("session-a", { exact: true }).waitFor();
  await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
  revoked = true; emit("context-admin", { code: "context_revoked", message: "Contexte révoqué", retry_action: "select_context" });
  await page.getByRole("heading", { name: "Choisir un contexte." }).waitFor();
  assert.equal(await page.getByText("session-a", { exact: true }).count(), 0, "Une révocation retire les données du contexte");
  assert.equal(await page.getByRole("button", { name: /Ouvrir administration/ }).count(), 0);
  await choose("personnel"); await page.getByText("session-a", { exact: true }).waitFor();
  authDenied = true; await tab("Actualiser"); await page.getByLabel("Credential personnel local").waitFor();
  assert.equal(await page.getByLabel("Credential personnel local").inputValue(), "");
  assert.equal(await page.locator(".human-shell, dialog").count(), 0); assert.equal(downloadCount, 1);
  assert.equal(await page.evaluate(() => localStorage.length + sessionStorage.length), 0);
  assert.ok(requests.every((request) => request.authorized && request.method === "GET" && request.path.startsWith("/api/v1/human/") && !request.path.includes("tenant_id=") && !request.lastEventId));
  assert.deepEqual(errors, []);
  process.stdout.write(`Parcours humain fixture PASS : identité, contextes serveur, droits metadata/content/export distincts, export complet borné, détail tardif ignoré, indisponibilité, refus ressource, révocation de contexte,401 et token mémoire. Captures : ${artifacts}. Aucun backend/runtime qualifié.\n`);
  }
} catch (error) {
  process.stderr.write(JSON.stringify({ errors, requests, page: await page.locator("body").innerText() }, null, 2) + "\n"); throw error;
} finally { await browser.close(); server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); }
