// Product UI regressions with isolated fixtures; never imported by the application.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir, mkdtemp } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { identity, page as envelope, resource, event, uuid } from "./human-fixtures.mjs";

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
      const value = identity(); if (revoked) value.contexts = value.contexts.filter((context) => context.id !== "context-admin");
      return json(response, value);
    }
    if (!String(url.pathname).startsWith("/api/v1/human/")) { response.writeHead(404); response.end(); return; }
    if (!identity().contexts.some((context) => context.id === contextId)) return error(response, 403, "context_invalid", "select_context");
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
  const nativeFetch = window.fetch; window.__humanHold = false; window.__humanHeld = [];
  const nativeInterval = window.setInterval;
  window.__humanPoll = [];
  window.setInterval = (handler, delay, ...args) => {
    if (delay === 30000 && typeof handler === "function") window.__humanPoll.push(() => handler(...args));
    return nativeInterval(handler, delay, ...args);
  };
  window.__humanRelease = () => { window.__humanHeld.splice(0).forEach((release) => release()); };
  window.fetch = async (...args) => {
    const response = await nativeFetch(...args);
    if (window.__humanHold && String(args[0]).includes("/human/events/")) {
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
const runRegression = async () => {
  if (scenario === "focus") {
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
