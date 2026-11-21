// Runs the shipped browser oracle and compiled dashboard against synthetic audit data.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { brokerBatchFixture } from "./browser_broker_provider_fixture.mjs";
import { federationPage } from "./federation-fixture.mjs";

const runFile = promisify(execFile);
const { expected, worker, cleanup, events } = brokerBatchFixture();
const tenant = "broker-browser-fixture";
const viewer = "synthetic-broker-browser-viewer";
const sandbox = { id: expected.session_id, session_id: expected.session_id, engine: "docker", network: "none" };
const sandboxEvents = [...events].reverse();
const types = { ".html": "text/html", ".js": "application/javascript", ".css": "text/css", ".svg": "image/svg+xml" };

function json(response, data) {
  response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(data));
}

function listen(service) {
  const server = createServer(async (request, response) => {
    const { pathname, searchParams } = new URL(request.url, "http://127.0.0.1");
    if (request.method !== "GET") return response.writeHead(405).end();
    if (pathname === "/healthz") return json(response, { service, status: "ok" });
    if (pathname.startsWith("/api/")) {
      if (request.headers.authorization !== `Bearer ${viewer}`) return response.writeHead(401).end();
      switch (pathname) {
        case "/api/v1/identity": return json(response, { tenant_id: tenant, role: "viewer" });
        case "/api/overview": return json(response, { counts: { commands: 1, sandboxes: 1 }, health: { collector: "ok" } });
        case "/api/agents": return json(response, { items: [] });
        case "/api/sandboxes": return json(response, { items: [sandbox] });
        case "/api/integrity": return json(response, { valid: true, event_count: events.length + 1 });
        case "/api/stream":
          response.writeHead(200, { "content-type": "text/event-stream" }).write(": synthetic\n\n");
          return;
        case "/api/v1/events":
        case "/api/events": {
          const after = searchParams.get("after");
          const before = searchParams.get("before");
          const items = after === String(cleanup.seq - 1) && before === String(cleanup.seq + 1)
            ? [cleanup] : sandboxEvents;
          return json(response, { items, next_cursor: null });
        }
        case "/api/export":
          if (searchParams.get("format") !== "jsonl" || searchParams.get("sandbox_id") !== expected.session_id)
            return response.writeHead(400).end();
          response.writeHead(200, { "content-type": "application/x-ndjson", "content-disposition": "attachment; filename=\"fixture.jsonl\"" })
            .end(events.map((event) => JSON.stringify(event)).join("\n") + "\n");
          return;
        case "/api/v1/federation/workers": return json(response, federationPage({ items: [worker], next_cursor: null }));
        case `/api/v1/workers/${worker.worker_id}`: return json(response, worker);
        default: return response.writeHead(404).end();
      }
    }
    const path = pathname === "/" ? "index.html" : pathname.slice(1);
    if (service !== "dashboard" || !/^(?:index\.html|favicon\.svg|assets\/[\w.-]+)$/.test(path))
      return response.writeHead(404).end();
    try {
      const bytes = await readFile(new URL("../dist/" + path, import.meta.url));
      response.writeHead(200, { "content-type": types[extname(path)] }).end(bytes);
    } catch { response.writeHead(404).end(); }
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

const evidence = await mkdtemp(join(tmpdir(), "ai-guard-broker-browser-fixture-"));
const privateOptions = { mode: 0o600, flag: "wx" };
await mkdir(join(evidence, "secrets"), { mode: 0o700 });
await writeFile(join(evidence, "secrets", "viewer"), viewer, privateOptions);
const proof = join(evidence, "broker-browser-proof.json");
await writeFile(proof, JSON.stringify(expected), privateOptions);
const servers = [];
const mode = "synthetic_broker_dashboard";
try {
  for (const service of ["gateway", "dashboard"]) servers.push(await listen(service));
  const [gateway, dashboard] = servers.map((server) => `http://127.0.0.1:${server.address().port}`);
  const env = Object.fromEntries(["PATH", "LANG", "CODEX_PRIMARY_RUNTIME_NODE_MODULES", "PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
    .filter((name) => process.env[name] !== undefined).map((name) => [name, process.env[name]]));
  const { stdout } = await runFile(process.execPath, [fileURLToPath(new URL("./browser_provider_live.mjs", import.meta.url))], {
    timeout: 90_000, maxBuffer: 1024 * 1024,
    env: { ...env, HOME: evidence, AI_GUARD_TEST_GATEWAY: gateway, AI_GUARD_TEST_DASHBOARD: dashboard,
      AI_GUARD_TEST_CONFIG: evidence, AI_GUARD_TEST_BROKER_PROOF: proof, AI_GUARD_TEST_ARTIFACT_DIR: evidence },
  });
  const result = JSON.parse(stdout);
  assert.equal(result.status, "PASS");
  assert.equal(result.mode, "broker_session_trace");
  assert.equal(result.count, events.length);
  console.log(JSON.stringify({ mode, status: "PASS", browser: result.browser, events: result.count }));
} catch (error) {
  let failure;
  try { failure = JSON.parse(error.stdout); } catch { /* No browser report. */ }
  console.error(JSON.stringify({ mode, status: "FAIL", stage: failure?.stage,
    reason: failure?.message ?? "La recette navigateur n’a pas produit la preuve de simulation attendue.",
    evidence, captures: failure?.artifacts }));
  process.exitCode = 1;
} finally {
  await Promise.all(servers.map((server) => new Promise((resolve) => {
    server.close(resolve);
    server.closeAllConnections();
  })));
}
