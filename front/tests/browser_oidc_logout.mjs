// Deterministic frontend regression with test-only HTTP fixtures and the real bundle.
// This does not qualify an IdP, the Rust gateway, or a real SSO deployment.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createConnection } from "node:net";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { identity, uuid } from "./human-fixtures.mjs";

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require("playwright")); }
catch {
  if (process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES) {
    try { ({ chromium } = require(join(process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES, "playwright"))); } catch {}
  }
}
const root = fileURLToPath(new URL("..", import.meta.url));
const oidcBearer = "fixture-oidc-logout-old";
const localBearer = "fixture-oidc-logout-local";
const viewerBearer = "fixture-oidc-logout-viewer";
let barrier;
const cases = new Set(["suite", "viewer", "human"]);
const phases = new Set(["listen", "launch", "connect", "context", "init", "route", "page", "oidc", "logout", "replacement", "drain", "assert", "cleanup-context", "cleanup-browser", "cleanup-process", "cleanup-kill", "cleanup-exit", "cleanup-child-close", "cleanup-ws", "cleanup-server"]);
const preparationPhases = new Set(["listen", "launch", "connect", "context", "init", "route", "page"]);
const mark = (access, phase, state, elapsed_ms) => {
  assert.ok(cases.has(access) && phases.has(phase) && ["start", "done", "timeout", "failed"].includes(state));
  process.stdout.write(JSON.stringify({ case: access, phase, state, ...(elapsed_ms === undefined ? {} : { elapsed_ms }) }) + "\n");
};
class HarnessTimeout extends Error {}
async function bounded(access, phase, call, milliseconds = preparationPhases.has(phase) ? 30000 : 5000) {
  const started = performance.now();
  mark(access, phase, "start"); let timer;
  try {
    const value = await Promise.race([Promise.resolve().then(call), new Promise((_, reject) => {
      timer = setTimeout(() => reject(new HarnessTimeout()), milliseconds);
    })]);
    mark(access, phase, "done", Math.round(performance.now() - started)); return value;
  } catch (error) { mark(access, phase, error instanceof HarnessTimeout ? "timeout" : "failed", Math.round(performance.now() - started)); throw error; }
  finally { clearTimeout(timer); }
}
const failure = (access, phase, error) => ({ case: access, phase,
  reason: error instanceof HarnessTimeout ? "harness_timeout" : error?.name === "TimeoutError" ? "playwright_timeout" :
    error instanceof assert.AssertionError ? (error.message.startsWith("Late OIDC logout must preserve") ? "replacement_dashboard_assertion" : "assertion") : "operation_failed" });
const sockets = new Set();
const json = (response, value, status = 200) => {
  response.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" }); response.end(JSON.stringify(value));
};
const humanIdentity = (oidc) => ({ ...identity(), contexts: [], display_name: oidc ? "Ancien compte OIDC" : "Nouveau compte local",
  ...(oidc ? { principal: { kind: "human_oidc", issuer: "https://id.example.test/realms/fixture", subject: "old-oidc-subject" },
    authentication: { method: "oidc_access_token", credential_id: uuid, expires_at: new Date(Date.now() + 120000).toISOString() } } : {}) });
const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  if (url.pathname === "/auth/oidc/status") return json(response, { version: 1, enabled: false });
  if (url.pathname === "/auth/oidc/tokens" && request.method === "POST") return json(response, {
    version: 1, authentication_id: uuid, generation: 1, token_type: "Bearer", access_token: oidcBearer,
    expires_at: new Date(Date.now() + 120000).toISOString(), expires_in_seconds: 120,
  });
  if (url.pathname === "/auth/oidc/logout" && request.method === "POST") {
    if (!barrier || request.headers.authorization !== `Bearer ${oidcBearer}`) return json(response, {}, 400);
    const currentBarrier = barrier;
    let size = 0; const chunks = [];
    try {
      for await (const chunk of request) {
        size += chunk.length;
        if (size > 1024) return json(response, {}, 413);
        chunks.push(chunk);
      }
    } catch { response.destroy(); return; }
    let body; try { body = JSON.parse(Buffer.concat(chunks).toString("utf8")); } catch { return json(response, {}, 400); }
    if (JSON.stringify(body) !== JSON.stringify({ version: 1, authentication_id: uuid, generation: 1 })) return json(response, {}, 400);
    currentBarrier.response = response; currentBarrier.entered(); return;
  }
  if (url.pathname === "/api/v1/human/identity" && request.method === "GET") {
    if (request.headers.authorization === `Bearer ${oidcBearer}`) return json(response, humanIdentity(true));
    if (request.headers.authorization === `Bearer ${localBearer}`) return json(response, humanIdentity(false));
    return json(response, {}, 401);
  }
  if (url.pathname.startsWith("/api/")) {
    if (request.headers.authorization !== `Bearer ${viewerBearer}` || request.method !== "GET") return json(response, {}, 401);
    if (url.pathname === "/api/v1/identity") return json(response, { tenant_id: "replacement-viewer", role: "viewer" });
    if (url.pathname === "/api/overview") return json(response, { counts: { agents: 0 } });
    if (["/api/agents", "/api/sandboxes"].includes(url.pathname)) return json(response, { items: [] });
    if (url.pathname === "/api/events") return json(response, { items: [], next_cursor: null });
    if (url.pathname === "/api/integrity") return json(response, { valid: true });
    if (url.pathname === "/api/stream") {
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" }); response.write(": ready\n\n"); return;
    }
    return json(response, {}, 404);
  }
  if (url.pathname === "/healthz") return json(response, { status: "ok" });
  try {
    const path = ["/", "/auth/oidc/complete"].includes(url.pathname) ? "index.html" : url.pathname.slice(1);
    if (path.includes("..")) throw new Error("Invalid static path");
    const body = await readFile(join(root, "dist", path));
    response.writeHead(200, { "content-type": { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" }[extname(path)] || "application/octet-stream" });
    response.end(body);
  } catch { response.writeHead(404); response.end(); }
});
server.on("connection", (socket) => { sockets.add(socket); socket.once("close", () => sockets.delete(socket)); });
let browserServer; let browser; let child; let childExit; let childClose; let wsPort; let shuttingDown = false;
const failures = []; const cleanupFailures = [];
async function cleanup(access, phase, call) {
  try { await bounded(access, phase, call, 15000); }
  catch (error) { cleanupFailures.push(failure(access, phase, error)); }
}
const exited = () => !child || child.exitCode !== null || child.signalCode !== null;
async function killOwnedProcess() {
  if (exited()) return;
  await cleanup("suite", "cleanup-kill", () => browserServer.kill());
  if (!exited()) await cleanup("suite", "cleanup-exit", async () => { child.kill("SIGKILL"); await childExit; });
}
let suitePhase = "listen";
try {
  await bounded("suite", "listen", () => new Promise((resolve, reject) => {
    server.once("error", reject); server.listen(0, "127.0.0.1", () => { server.off("error", reject); resolve(); });
  }));
  const origin = `http://127.0.0.1:${server.address().port}`;
  suitePhase = "launch";
  await bounded("suite", "launch", async () => {
    if (!chromium) throw new Error("Playwright unavailable");
    browserServer = await chromium.launchServer({ host: "127.0.0.1", headless: true, timeout: 30000,
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined, args: ["--no-sandbox"] });
    child = browserServer.process();
    childExit = exited() ? Promise.resolve() : new Promise((resolve) => child.once("exit", resolve));
    childClose = new Promise((resolve) => child.once("close", resolve));
    const endpoint = new URL(browserServer.wsEndpoint());
    assert.equal(endpoint.hostname, "127.0.0.1");
    wsPort = Number(endpoint.port);
    assert.ok(Number.isInteger(wsPort) && wsPort > 0 && wsPort <= 65535);
    // The native launch timeout owns failed launches; a late successful acquisition still owns its child.
    if (shuttingDown) await killOwnedProcess();
  });
  suitePhase = "connect";
  browser = await bounded("suite", "connect", () => chromium.connect(browserServer.wsEndpoint(), { timeout: 30000 }));
  for (const access of ["viewer", "human"]) {
    let context; let page; let stage = "context"; let outsideRequests = 0; let pageErrors = 0;
    try {
    await bounded(access, "context", async () => { context = await browser.newContext({ serviceWorkers: "block" }); });
    stage = "init";
    await bounded(access, "init", () => context.addInitScript(() => {
      const parse = JSON.parse;
      window.__oidcLogoutProgress = { parsed: 0, drained: 0 };
      // Observe native JSON consumption without replacing a response or changing a return value.
      JSON.parse = function (...args) {
        const value = Reflect.apply(parse, this, args);
        if (value?.version === 1 && value.local_session === "closed" && value.provider_logout === "accepted" &&
          value.audit === "recorded" && Object.keys(value).length === 4) {
          const observation = ++window.__oidcLogoutProgress.parsed;
          const channel = new MessageChannel();
          // #request -> logout -> mountOidc.then have no further IO after this parse.
          // The next task runs after that entire native promise microtask checkpoint.
          channel.port1.onmessage = () => {
            window.__oidcLogoutProgress.drained = observation; channel.port1.close(); channel.port2.close();
          };
          channel.port2.postMessage(null);
        }
        return value;
      };
    }));
    stage = "route";
    await bounded(access, "route", () => context.route("**/*", (route) => {
      if (new URL(route.request().url()).origin === origin) return route.continue();
      outsideRequests++; return route.abort();
    }));
    stage = "page";
    await bounded(access, "page", async () => { page = await context.newPage(); }); page.setDefaultTimeout(5000);
    page.on("pageerror", () => pageErrors++);
    let entered;
    const logoutEntered = new Promise((resolve) => { entered = resolve; });
    barrier = { entered, response: null };
    stage = "oidc";
    await bounded(access, "oidc", async () => {
      await page.goto(`${origin}/auth/oidc/complete`);
      await page.getByText("Ancien compte OIDC", { exact: true }).waitFor();
    });
    stage = "logout";
    await bounded(access, "logout", async () => {
      await page.getByRole("button", { name: "Se déconnecter", exact: true }).click();
      await logoutEntered;
      assert.ok(barrier.response && !barrier.response.writableEnded, "The real logout HTTP response must remain deferred");
      assert.equal(await page.getByText("Ancien compte OIDC", { exact: true }).count(), 0, "Old OIDC identity must be removed before replacement login");
    });
    const replacement = () => access === "viewer" ? page.getByText("Client : replacement-viewer", { exact: true }).first() : page.getByText("Nouveau compte local", { exact: true });
    stage = "replacement";
    await bounded(access, "replacement", async () => {
      await page.getByLabel("Type d’accès").selectOption(access);
      await page.getByLabel(access === "viewer" ? "Jeton de lecture" : "Credential personnel local").fill(access === "viewer" ? viewerBearer : localBearer);
      await page.getByRole("button", { name: access === "viewer" ? "Ouvrir la supervision" : "Ouvrir mon espace", exact: true }).click();
      await replacement().waitFor();
      assert.ok(!barrier.response.writableEnded, "Replacement authentication must finish before old logout is released");
      assert.deepEqual(await page.evaluate(() => window.__oidcLogoutProgress), { parsed: 0, drained: 0 }, "Logout JSON cannot have been consumed before its HTTP response is released");
    });
    stage = "drain";
    await bounded(access, "drain", async () => {
      const delivered = page.waitForResponse((response) => new URL(response.url()).pathname === "/auth/oidc/logout");
      json(barrier.response, { version: 1, local_session: "closed", provider_logout: "accepted", audit: "recorded" });
      await delivered;
      await page.waitForFunction(() => window.__oidcLogoutProgress.drained === 1);
      assert.deepEqual(await page.evaluate(() => window.__oidcLogoutProgress), { parsed: 1, drained: 1 }, "The real logout JSON and its application continuations must finish before checking the dashboard");
    });
    stage = "assert";
    await bounded(access, "assert", async () => {
      assert.equal(await replacement().count(), 1, `Late OIDC logout must preserve the authenticated ${access} dashboard`);
      assert.equal(await page.getByRole("button", { name: "Se déconnecter", exact: true }).count(), 1);
      assert.equal(await page.locator(".login-page").count(), 0, "A stale logout cannot reopen the login screen");
      assert.equal(pageErrors, 0, "No browser exception"); assert.equal(outsideRequests, 0, "Only the loopback fixture origin may be requested");
    });
    } catch (error) {
      failures.push(failure(access, stage, error));
    } finally {
      barrier?.response?.destroy(); barrier = null;
      if (context) await cleanup(access, "cleanup-context", () => context.close());
    }
    if (cleanupFailures.length) break;
  }
} catch (error) {
  failures.push(failure("suite", suitePhase, error));
} finally {
  shuttingDown = true;
  try {
    if (browserServer) await cleanup("suite", "cleanup-process", () => browserServer.close());
    if (browser) await cleanup("suite", "cleanup-browser", () => browser.close());
    if (!exited()) await killOwnedProcess();
    if (childClose) await cleanup("suite", "cleanup-child-close", () => childClose);
    if (wsPort) await cleanup("suite", "cleanup-ws", () => new Promise((resolve, reject) => {
      // Observe asynchronous shutdown of only the owned listener. No application bytes are sent.
      let socket; let retry; let done = false;
      const finish = (error) => {
        if (done) return; done = true; clearTimeout(deadline); clearTimeout(retry);
        socket?.setTimeout(0); socket?.destroy(); error ? reject(error) : resolve();
      };
      const deadline = setTimeout(() => finish(new HarnessTimeout()), 15000);
      const probe = () => {
        if (done) return;
        const current = createConnection({ host: "127.0.0.1", port: wsPort }); socket = current;
        let completed = false;
        current.setTimeout(5000, () => finish(new HarnessTimeout()));
        current.once("connect", () => {
          completed = true; current.setTimeout(0); current.destroy();
          if (!done) retry = setTimeout(probe, 100);
        });
        current.once("error", (error) => {
          if (completed || done) return;
          finish(error.code === "ECONNREFUSED" ? null : new Error("Owned listener closure is unconfirmed"));
        });
      };
      probe();
    }));
  } finally {
    await cleanup("suite", "cleanup-server", () => new Promise((resolve, reject) => {
      server.closeAllConnections(); for (const socket of sockets) socket.destroy();
      if (!server.listening) { resolve(); return; }
      server.close((error) => error ? reject(error) : resolve());
    }));
  }
}
if (failures.length || cleanupFailures.length) {
  process.stdout.write(JSON.stringify({ result: "FAIL", failures, cleanup_failures: cleanupFailures }) + "\n"); process.exitCode = 1;
} else process.stdout.write("OIDC logout replacement-session regression PASS: viewer and human; frontend fixtures only.\n");
