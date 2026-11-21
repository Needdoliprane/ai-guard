// Test child of runtime_user_journey.rs. The same Rust gateway supplies every response.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import { closeBrowserResources, installBrowserGuard } from "./browser_live.mjs";
import { saveBrowserFailure } from "./browser_failure.mjs";

const uuid = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
const sha = /^[a-f0-9]{64}$/;
const tenants = { A1: "tenant-a", A2: "tenant-a", B1: "tenant-b" };
const closed = (value, keys) => {
  assert.ok(value && typeof value === "object" && !Array.isArray(value));
  assert.deepEqual(Object.keys(value).sort(), [...keys].sort());
};

async function readInput() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    assert.ok(size <= 16 * 1024);
    chunks.push(chunk);
  }
  const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  const explicitRuntime = input !== null && Object.hasOwn(input, "runtime_kind");
  closed(input, ["version", "dashboard", "viewers", "workers", ...(explicitRuntime ? ["runtime_kind"] : [])]);
  assert.equal(input.version, 1);
  // Only the historical Docker proof omitted the runtime. Its records are
  // still checked against Docker at every source used below.
  const runtime = explicitRuntime ? input.runtime_kind : "docker";
  assert.ok(runtime === "docker" || runtime === "podman");
  const origin = new URL(input.dashboard);
  assert.ok(origin.protocol === "http:" && origin.hostname === "127.0.0.1" && origin.port && origin.origin === input.dashboard);
  closed(input.viewers, ["tenant-a", "tenant-b"]);
  const secrets = Object.values(input.viewers);
  assert.ok(secrets.every((value) => typeof value === "string" && value.length >= 32 && value.length <= 4096 && !/\s/.test(value)));
  assert.equal(new Set(secrets).size, 2);
  assert.deepEqual(input.workers.map((worker) => worker.label).sort(), ["A1", "A2", "B1"]);
  for (const worker of input.workers) {
    closed(worker, ["label", "tenant", "session_id", "worker_id", "attempt_id", "container_id", "volume", "spec_hash", "authority_id", "image_id", "usage", "completion", "cleanup", "export_matching_source", "terminal_audit", "export_audit"]);
    assert.equal(worker.tenant, tenants[worker.label]);
    assert.ok([worker.session_id, worker.worker_id, worker.attempt_id, worker.authority_id].every((value) => typeof value === "string" && uuid.test(value)));
    assert.ok([worker.container_id, worker.spec_hash].every((value) => typeof value === "string" && sha.test(value)));
    assert.match(worker.image_id, /^sha256:[a-f0-9]{64}$/);
    assert.equal(worker.completion, "normal");
    assert.equal(worker.cleanup, "worker_removed");
    assert.equal(worker.export_matching_source, true);
    assert.equal(worker.usage?.source?.runtime_kind, runtime);
    for (const event of [worker.terminal_audit, worker.export_audit]) {
      closed(event, ["seq", "id", "hash"]);
      assert.ok(Number.isSafeInteger(event.seq) && event.seq > 0 && uuid.test(event.id) && sha.test(event.hash));
    }
    assert.ok(worker.terminal_audit.seq < worker.export_audit.seq);
  }
  for (const key of ["worker_id", "session_id", "attempt_id", "container_id", "volume"])
    assert.equal(new Set(input.workers.map((worker) => worker[key])).size, 3);
  return { ...input, runtime_kind: runtime, secrets };
}

const link = (page, name) => page.getByRole("link", { name, exact: true });
const button = (page, name) => page.getByRole("button", { name, exact: true });
async function readByAction(page, origin, pathname, action, query) {
  const cursor = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return response.request().method() === "GET" && url.origin === origin && url.pathname === pathname && (!query || query(url.searchParams));
  });
  const [response] = await Promise.all([cursor, action()]);
  assert.equal(response.status(), 200);
  return response.json();
}

async function checkField(page, dialog, label, expected) {
  const value = dialog.locator(".key-value").filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
  await value.waitFor();
  assert.equal(await value.textContent(), String(expected));
}

async function checkWorker(page, origin, rows, expected, runtime) {
  const row = rows.filter({ has: page.getByText(expected.worker_id, { exact: true }) });
  assert.equal(await row.count(), 1);
  for (const label of [expected.session_id, "Terminal observé", "Quota libéré"]) await row.getByText(label, { exact: true }).waitFor();
  const detail = await readByAction(page, origin, `/api/v1/workers/${expected.worker_id}`, () => button(row, "Détails").click());
  assert.equal(detail.worker_id, expected.worker_id);
  assert.equal(detail.state, "terminal");
  assert.equal(detail.quota_held, false);
  assert.equal(detail.cleanup_removed, true);
  assert.equal(detail.launch.session_id, expected.session_id);
  assert.equal(detail.launch.runtime_kind, runtime);
  assert.equal(detail.launch.authority_id, expected.authority_id);
  assert.equal(detail.launch.image_id, expected.image_id);
  assert.equal(detail.spec_hash, expected.spec_hash);
  assert.equal(detail.attempt.attempt_id, expected.attempt_id);
  assert.equal(detail.attempt.container_id, expected.container_id);
  assert.equal(detail.last_observation.exit_code, 0);
  const inspection = detail.last_observation.inspection;
  assert.equal(inspection.container_id, expected.container_id);
  assert.equal(inspection.authority_id, expected.authority_id);
  assert.equal(inspection.image_id, expected.image_id);
  assert.equal(inspection.spec_hash, expected.spec_hash);
  assert.equal(inspection.runtime_kind, runtime);
  const dialog = page.getByRole("dialog");
  for (const [label, value] of [
    ["Session", expected.session_id], ["Tentative", expected.attempt_id], ["Runtime", runtime],
    ["Autorité déclarée du lancement", expected.authority_id], ["Conteneur inspecté", expected.container_id],
    ["Empreinte de l’image inspectée", expected.image_id], ["Empreinte du plan liée à l’inspection", expected.spec_hash],
    ["Runtime de l’inspection enregistrée", runtime], ["Code de sortie", 0], ["Retrait constaté", "Oui"],
  ]) await checkField(page, dialog, label, value);
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
}

async function checkAudit(page, origin, expected) {
  await link(page, "Actions").click();
  await page.getByRole("combobox", { name: "Toutes les sandboxes", exact: true }).selectOption(expected.session_id);
  const pageOfEvents = await readByAction(page, origin, "/api/events", () => button(page, "Filtrer").click(),
    (query) => query.get("sandbox_id") === expected.session_id);
  assert.equal(pageOfEvents.next_cursor, null);
  for (const [view, kind, receipt] of [
    ["Actions", "command.completed", expected.terminal_audit],
    ["Fichiers", "workspace.export.completed", expected.export_audit],
  ]) {
    const event = pageOfEvents.items.find((item) => item.seq === receipt.seq);
    assert.ok(event && event.id === receipt.id && event.hash === receipt.hash && event.kind === kind && event.session_id === expected.session_id);
    if (kind === "command.completed") {
      assert.equal(event.data.worker_id, expected.worker_id);
      assert.equal(event.result.exit_code, 0);
    } else assert.equal(event.data.files, 1);
    await link(page, view).click();
    await button(page, `Voir l’événement ${receipt.seq}`).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("heading", { name: kind, exact: true }).waitFor();
    await dialog.getByRole("tab", { name: "Données brutes", exact: true }).click();
    assert.deepEqual(JSON.parse(await dialog.locator("pre").textContent()), event);
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
  }
}

async function checkTenant(page, origin, input, tenant) {
  const expected = input.workers.filter((worker) => worker.tenant === tenant);
  const foreign = input.workers.filter((worker) => worker.tenant !== tenant);
  const token = input.viewers[tenant];
  await page.getByLabel("Jeton de lecture", { exact: true }).fill(token);
  const identity = await readByAction(page, origin, "/api/v1/identity", () => button(page, "Ouvrir la supervision").click());
  assert.deepEqual(identity, { role: "viewer", tenant_id: tenant });
  await page.locator(".topbar").getByText(`Client : ${tenant}`, { exact: true }).waitFor();
  const listing = await readByAction(page, origin, "/api/v1/federation/workers", () => link(page, "Workers").click(), (query) => query.get("limit") === "50");
  assert.equal(listing.source, "tenant_worker_broker_registry");
  assert.equal(listing.runtime_inventory, "unknown");
  assert.equal(listing.next_cursor, null);
  const ids = expected.map((item) => item.worker_id).sort();
  assert.deepEqual(listing.items.map(({ worker }) => worker.worker_id).sort(), ids);
  for (const { worker } of listing.items) assert.equal(worker.launch.runtime_kind, input.runtime_kind);
  const table = page.getByRole("table", { name: "Workers du client authentifié", exact: true });
  await table.waitFor();
  assert.deepEqual((await table.locator("tbody strong.control-id").allTextContents()).sort(), ids);
  for (const worker of expected) await checkWorker(page, origin, table.locator("tbody tr"), worker, input.runtime_kind);
  for (const { worker_id } of foreign) {
    const denied = await page.evaluate(async ({ id, token }) => {
      const response = await fetch(`/api/v1/workers/${id}`, { headers: { Authorization: `Bearer ${token}` }, credentials: "omit", redirect: "error", signal: AbortSignal.timeout(8000) });
      return { status: response.status, body: await response.json() };
    }, { id: worker_id, token });
    assert.deepEqual(denied, { status: 404, body: { error: "Worker ou opération introuvable.", code: "worker_not_found" } });
  }
  const quota = await readByAction(page, origin, "/api/v1/quota", () => link(page, "Quotas").click());
  assert.equal(quota.held_workers, 0);
  assert.equal(quota.policy.max_active_workers, expected.length);
  const held = page.locator(".quota-row").filter({ has: page.getByText("Workers actifs réservés", { exact: true }) }).locator("dd");
  await held.waitFor();
  assert.equal(await held.textContent(), `0 / ${expected.length}`);
  const usage = await readByAction(page, origin, "/api/v1/usage", () => link(page, "Allocations").click());
  assert.equal(usage.next_cursor, null);
  assert.deepEqual(usage.items.map((item) => item.worker_id).sort(), ids);
  const allocations = page.getByRole("table", { name: "Allocations terminales non facturables", exact: true });
  await allocations.waitFor();
  for (const worker of expected) {
    const record = usage.items.find((item) => item.worker_id === worker.worker_id);
    assert.deepEqual(record, worker.usage);
    assert.equal(record.billable, false);
    assert.equal(record.source.provenance, "observed");
    assert.equal(record.source.runtime_kind, input.runtime_kind);
    const row = allocations.locator("tbody tr").filter({ has: page.getByText(worker.worker_id, { exact: true }) });
    await row.getByText(worker.attempt_id, { exact: true }).waitFor();
    await row.getByText("Observé", { exact: true }).waitFor();
    await row.getByText(`${input.runtime_kind} · ${worker.usage.source.authority_id}`, { exact: true }).waitFor();
  }
  for (const worker of expected) await checkAudit(page, origin, worker);
  await button(page, "Se déconnecter").click();
  await page.getByLabel("Jeton de lecture", { exact: true }).waitFor();
  assert.equal(await page.locator(".control-view, dialog[open]").count(), 0);
}

let stage = "input", status = "FAIL", browser, context, guard, page, input, capture;
let deadline;
try {
  await Promise.race([(async () => {
    input = await readInput();
    stage = "launch";
    const require = createRequire(import.meta.url);
    const { chromium } = require(join(process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES, "playwright"));
    browser = await chromium.launch({ headless: true, chromiumSandbox: true, timeout: 15_000,
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
      args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] });
    context = await browser.newContext({ serviceWorkers: "block" });
    guard = await installBrowserGuard(context, [input.dashboard], input.secrets);
    page = await context.newPage();
    page.setDefaultTimeout(7000);
    page.setDefaultNavigationTimeout(7000);
    const errors = [];
    page.on("pageerror", () => errors.push("page"));
    await guard.preparePage(page);
    await page.goto(input.dashboard);
    for (const tenant of ["tenant-a", "tenant-b"]) {
      stage = tenant === "tenant-a" ? "viewer_a" : "viewer_b";
      await checkTenant(page, input.dashboard, input, tenant);
    }
    assert.deepEqual(errors, []);
    assert.deepEqual(guard.violations, []);
  })(), new Promise((_, reject) => { deadline = setTimeout(() => reject(new Error("QA browser deadline")), 90_000); })]);
  stage = "complete";
  status = "PASS";
} catch {
  const failedStage = stage;
  status = stage === "launch" ? "BLOCKED" : "FAIL";
  const url = page && URL.parse(page.url());
  if (url && url.origin === input.dashboard && url.pathname === "/" && !url.search &&
    ["", "#overview", "#workers", "#quota", "#usage", "#actions", "#files"].includes(url.hash) &&
    await page.locator(".topbar .authenticated-scope").isVisible().catch(() => false) &&
    await page.getByLabel("Jeton de lecture", { exact: true }).count().catch(() => 1) === 0) {
    capture = await saveBrowserFailure(page, { directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR, secrets: input.secrets });
  }
  stage = failedStage;
} finally {
  clearTimeout(deadline);
  const cleanup = await closeBrowserResources(context, browser);
  if (cleanup.length) { stage = "cleanup"; status = "FAIL"; }
  if (guard?.violations.length) { stage = "network"; status = "FAIL"; }
}
process.stdout.write(JSON.stringify({ status, stage,
  ...(status === "PASS" ? { workers: 3, tenant_denials: 3, audit_events: 6, runtime_kind: input.runtime_kind } : {}),
  ...(capture ? { capture } : {}),
}) + "\n");
process.exitCode = status === "PASS" ? 0 : status === "BLOCKED" ? 2 : 1;
