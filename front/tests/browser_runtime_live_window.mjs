// One freshly read Workers page for each tenant at the peak observed by Rust.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { closeBrowserResources, installBrowserGuard } from "./browser_live.mjs";
import { saveBrowserFailure } from "./browser_failure.mjs";

const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
const frames = input[Symbol.asyncIterator]();
async function frame(keys) {
  const { done, value } = await frames.next();
  assert.ok(!done && Buffer.byteLength(value) < 16 * 1024);
  const valueJson = JSON.parse(value);
  assert.deepEqual(Object.keys(valueJson).sort(), [...keys].sort());
  return valueJson;
}

const windowTimeout = 7000;
const button = (page, name) => page.getByRole("button", { name, exact: true });
const link = (page, name) => page.getByRole("link", { name, exact: true });
function responseOf(page, origin, path) {
  return page.waitForResponse((response) => response.request().method() === "GET" && response.url() === origin + path,
    { timeout: windowTimeout });
}
const registryPath = "/api/v1/federation/workers?limit=50";
const maxPeakReads = 64;
const peakReads = [];
const logoutChecks = [];
let origin, browser, secrets = [], status = "FAIL", stage = "setup";
const sessions = [];

async function openViewer(tenant, token) {
  const context = await browser.newContext({ serviceWorkers: "block", viewport: { width: 1440, height: 1050 } });
  const session = { context, streamRequests: 0, activeStreams: new Set() };
  sessions.push(session);
  const guard = await installBrowserGuard(context, [origin], secrets);
  const page = await context.newPage();
  Object.assign(session, { guard, page });
  page.setDefaultTimeout(windowTimeout);
  page.setDefaultNavigationTimeout(windowTimeout);
  session.errors = 0;
  page.on("pageerror", () => { session.errors++; });
  const isStream = (request) => request.method() === "GET" && request.url() === origin + "/api/stream";
  page.on("request", (request) => { if (isStream(request)) session.streamRequests++; });
  page.on("response", (response) => {
    if (isStream(response.request()) && response.status() === 200 &&
        response.headers()["content-type"]?.startsWith("text/event-stream")) {
      session.activeStreams.add(response.request());
    }
  });
  page.on("requestfinished", (request) => { session.activeStreams.delete(request); });
  page.on("requestfailed", (request) => { session.activeStreams.delete(request); });
  await guard.preparePage(page);
  await page.goto(origin);
  await page.getByLabel("Jeton de lecture", { exact: true }).fill(token);
  const [identity] = await Promise.all([responseOf(page, origin, "/api/v1/identity"), button(page, "Ouvrir la supervision").click()]);
  assert.equal(identity.status(), 200);
  assert.deepEqual(await identity.json(), { role: "viewer", tenant_id: tenant });
  await page.locator(".topbar").getByText(`Client : ${tenant}`, { exact: true }).waitFor();
  const [registry] = await Promise.all([responseOf(page, origin, registryPath), link(page, "Workers").click()]);
  assert.equal(registry.status(), 200);
  assert.deepEqual((await registry.json()).items, []);
  await page.getByText("Aucun worker sur cette page", { exact: true }).waitFor();
  // Do not leave Workers mounted while SSE reports startup transitions.
  await link(page, "Vue d’ensemble").click();
  await page.getByRole("heading", { name: "Vue d’ensemble", exact: true }).waitFor();
}

async function logoutViewer(session, workers) {
  const { page, activeStreams, streamRequests } = session;
  if (activeStreams.size !== 1) return "stream_not_open";
  const [stream] = activeStreams;
  const identifiers = workers.flatMap(({ worker_id, session_id }) => [worker_id, session_id]);
  const [cancelled, login, clicked] = await Promise.allSettled([
    page.waitForEvent("requestfailed", {
      predicate: (request) => request === stream && request.failure()?.errorText === "net::ERR_ABORTED",
      timeout: windowTimeout,
    }),
    page.waitForFunction((ids) => {
      const input = document.querySelector(".login-form #viewer-token[type=password]");
      const display = document.querySelector("#app")?.textContent;
      return location.pathname === "/" && !location.hash && !location.search &&
        input?.value === "" && input.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) &&
        !document.querySelector(".authenticated-scope, .control-view, .control-dock, .header-logout, .stream-pill, dialog[open]") &&
        typeof display === "string" && ids.every((id) => !display.includes(id));
    }, identifiers, { timeout: windowTimeout }),
    button(page, "Se déconnecter").click({ timeout: windowTimeout }),
  ]);
  if (clicked.status !== "fulfilled") return "logout_click";
  if (login.status !== "fulfilled") return "login_or_scope";
  if (cancelled.status !== "fulfilled") return "stream_not_aborted";
  if (activeStreams.size !== 0 || session.streamRequests !== streamRequests) return "stream_reopened";
  if (session.guard.violations.length || session.errors) return "browser_guard";
  return "verified";
}

function readPeakDom({ expectedRows, evaluatedAt, allowScroll }) {
  const visible = (element) => {
    if (!element?.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return false;
    const rect = element.getBoundingClientRect();
    const left = Math.max(rect.left, 0), top = Math.max(rect.top, 0);
    const right = Math.min(rect.right, innerWidth), bottom = Math.min(rect.bottom, innerHeight);
    if (right <= left || bottom <= top) return false;
    return element.contains(document.elementFromPoint((left + right) / 2, (top + bottom) / 2));
  };
  const view = document.querySelector('.control-view[aria-busy="false"]');
  if (!view) return "loading";
  const status = view.querySelector(".control-status .badge");
  if (["Données à actualiser", "Données périmées · dernière lecture conservée"].includes(status?.textContent))
    return "stale";
  if (status?.textContent !== "Lecture réussie") return "transient";
  if (!visible(status)) return "status_hidden";
  const stamp = view.querySelector(".control-panel .panel-heading time[datetime]");
  if (stamp?.dateTime !== evaluatedAt) return "evaluation_changed";
  const formattedDate = new Intl.DateTimeFormat("fr-FR", { dateStyle: "medium", timeStyle: "medium" }).format(new Date(evaluatedAt));
  if (stamp.textContent !== formattedDate || !visible(stamp)) return "evaluation_hidden";
  const table = view.querySelector(".control-table");
  if (!visible(table)) return "table_hidden";
  const rows = [...table.querySelectorAll("tbody tr")];
  if (rows.length !== expectedRows.length) return "row_count";
  const displayedRows = rows.map((row) => {
    const badges = [...row.querySelectorAll(".badge")];
    return {
      row,
      worker: row.querySelector("strong.control-id"),
      session: row.querySelector("td:first-child span.muted.control-id"),
      running: badges.find((node) => node.textContent === "En cours"),
      quota: badges.find((node) => node.textContent === "Quota retenu"),
    };
  });
  const identities = displayedRows.map(({ worker, session }) => [worker?.textContent, session?.textContent]).sort();
  if (JSON.stringify(identities) !== JSON.stringify(expectedRows)) return "row_identity";
  if (displayedRows.every((row) => Object.values(row).every(visible))) return "ready";
  const targets = displayedRows.flatMap(({ worker, session, running, quota }) => [worker, session, running, quota]);
  if (allowScroll && targets.every(Boolean)) {
    const lastBottom = Math.max(...targets.map((node) => node.getBoundingClientRect().bottom));
    const pinnedBottom = Math.max(0, ...[...document.querySelectorAll(".topbar, .control-dock")]
      .map((node) => node.getBoundingClientRect().bottom));
    const move = Math.ceil(lastBottom - innerHeight + 16);
    if (move > 0 && status.getBoundingClientRect().top - move >= pinnedBottom + 16) {
      window.scrollBy({ top: move, behavior: "instant" });
      return "scrolled";
    }
  }
  return "row_visibility";
}

async function readPeak(session, workers, runtimeKind) {
  const { page } = session;
  const expectedRows = workers.map(({ worker_id, session_id }) => [worker_id, session_id]).sort();
  const inFlight = new Map();
  const evaluations = new Map();
  let requests = 0;
  let responses = 0;
  let latest;
  let invalidRead;
  let reason = "registry_wait";
  let domState = "unseen";
  let scrolled = false;
  const report = (code, read = latest) => ({
    code, http_status: read?.httpStatus ?? null, dom_state: domState,
    requests, responses,
  });
  const isRead = (request) => request.method() === "GET" && request.url() === origin + registryPath;
  const trackRequest = (request) => {
    if (!isRead(request)) return;
    requests++;
    if (requests > maxPeakReads) return;
    latest = { code: "registry_wait" };
    inFlight.set(request, latest);
  };
  const trackFailure = (request) => {
    const read = inFlight.get(request);
    if (read && read.httpStatus === undefined) inFlight.delete(request);
  };
  const validate = async (response, read) => {
    read.code = "registry_status";
    read.httpStatus = response.status();
    assert.equal(read.httpStatus, 200);
    read.code = "registry_shape";
    const { items, next_cursor, source, evaluated_at, runtime_inventory } = await response.json();
    assert.equal(next_cursor, null);
    assert.equal(source, "tenant_worker_broker_registry");
    assert.equal(runtime_inventory, "unknown");
    assert.ok(typeof evaluated_at === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(evaluated_at) &&
      Number.isFinite(Date.parse(evaluated_at)));
    assert.ok(Array.isArray(items));
    evaluations.set(evaluated_at, (evaluations.get(evaluated_at) ?? 0) + 1);
    read.code = "worker_binding";
    assert.deepEqual(items.map(({ worker }) => [worker.worker_id, worker.launch.session_id]).sort(), expectedRows);
    for (const { worker } of items) {
      const expected = workers.find((entry) => entry.worker_id === worker.worker_id);
      assert.equal(worker.state, "running");
      assert.equal(worker.quota_held, true);
      assert.equal(worker.attempt.attempt_id, expected.attempt_id);
      assert.equal(worker.attempt.stable_name, expected.stable_name);
      assert.equal(worker.attempt.container_id, expected.container_id);
      assert.equal(worker.launch.image_id, expected.image_id);
      assert.equal(worker.launch.runtime_kind, runtimeKind);
      assert.equal(worker.last_observation.running, true);
      assert.equal(worker.last_observation.restart_count, 0);
      assert.equal(worker.last_observation.inspection.container_id, expected.container_id);
      assert.equal(worker.last_observation.inspection.stable_name, expected.stable_name);
      assert.equal(worker.last_observation.inspection.image_id, expected.image_id);
      assert.equal(worker.last_observation.inspection.runtime_kind, runtimeKind);
      assert.equal(worker.last_observation.started_at, expected.started_at);
    }
    read.evaluatedAt = evaluated_at;
  };
  const trackResponse = (response) => {
    const request = response.request();
    const read = inFlight.get(request);
    if (!read) return;
    responses++;
    void validate(response, read).catch(() => {
      if (read.code === "registry_shape" || read.code === "worker_binding") invalidRead ??= read;
    }).finally(() => inFlight.delete(request));
  };
  page.on("request", trackRequest);
  page.on("requestfailed", trackFailure);
  page.on("response", trackResponse);
  try {
    await Promise.all([responseOf(page, origin, registryPath), link(page, "Workers").click()]);
    const deadline = performance.now() + windowTimeout;
    while (performance.now() < deadline) {
      if (requests > maxPeakReads) return { report: report("read_count") };
      if (invalidRead) return { report: report(invalidRead.code, invalidRead) };
      const candidate = latest;
      if (candidate?.evaluatedAt && inFlight.size === 0) {
        const matchingReads = evaluations.get(candidate.evaluatedAt);
        if (matchingReads !== 1) return { report: report("evaluation_ambiguous") };
        reason = "dom_wait";
        const timer = new AbortController();
        let sample;
        try {
          sample = await Promise.race([
            page.evaluate(readPeakDom, { expectedRows, evaluatedAt: candidate.evaluatedAt, allowScroll: !scrolled }),
            sleep(Math.max(0, deadline - performance.now()), null, { signal: timer.signal }),
          ]);
        } finally {
          timer.abort();
        }
        if (sample === null) return { report: report("dom_timeout") };
        scrolled ||= sample === "scrolled";
        domState = sample === "scrolled" ? "row_visibility" : sample;
        if (requests > maxPeakReads) return { report: report("read_count") };
        // A read started while evaluating could already have replaced this page.
        if (latest !== candidate || inFlight.size > 0) {
          reason = "registry_wait";
        } else if (domState === "ready" && performance.now() < deadline) {
          assert.ok(1 <= responses && responses <= requests);
          return {
            report: report("matched"),
            view: { evaluated_at: candidate.evaluatedAt, rendered_at: new Date().toISOString(),
              requests, responses, matching_reads: matchingReads, fresh: true, visible_rows: expectedRows.length },
          };
        }
      } else {
        reason = inFlight.size ? "registry_wait" : candidate?.code ?? "registry_wait";
      }
      const remaining = deadline - performance.now();
      if (remaining > 0) await sleep(Math.min(remaining, 25));
    }
    return { report: report(reason === "dom_wait" ? "dom_timeout" : reason) };
  } catch {
    return { report: report(reason === "dom_wait" ? "dom_error" : reason) };
  } finally {
    page.off("request", trackRequest);
    page.off("requestfailed", trackFailure);
    page.off("response", trackResponse);
  }
}

function captureHtmlOnly(page) {
  // The live SSE page can repaint between HTML sanitization and a screenshot.
  return saveBrowserFailure({
    isClosed: () => page.isClosed(),
    evaluate: async (...args) => {
      const html = await page.evaluate(...args);
      // A late logout can replace the page between the preliminary checks and serialization.
      if (!dashboardCaptureLocation(page) || !html.includes('class="authenticated-scope"') ||
          html.includes('class="login-form"')) return null;
      return html;
    },
    locator: (...args) => page.locator(...args),
    screenshot: async () => { throw new Error("dynamic page: HTML-only capture"); },
  }, { directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR, secrets });
}

function dashboardCaptureLocation(page) {
  const url = page && URL.parse(page.url());
  return url?.origin === origin && url.pathname === "/" && !url.search && ["", "#overview", "#workers"].includes(url.hash);
}

async function captureAuthenticated(page) {
  if (dashboardCaptureLocation(page) && await page.locator(".topbar .authenticated-scope").isVisible().catch(() => false) &&
    await page.getByLabel("Jeton de lecture", { exact: true }).count().catch(() => 1) === 0) {
    return captureHtmlOnly(page);
  }
}

const captures = [];
let cleanupFailed = false;
let networkViolation = false;
try {
  const setup = await frame(["dashboard", "runtime_kind", "version", "viewers"]);
  assert.equal(setup.version, 1);
  const runtimeKind = setup.runtime_kind;
  assert.ok(runtimeKind === "docker" || runtimeKind === "podman");
  const dashboard = new URL(setup.dashboard);
  assert.ok(dashboard.protocol === "http:" && dashboard.hostname === "127.0.0.1" && dashboard.port && dashboard.origin === setup.dashboard);
  origin = dashboard.origin;
  assert.ok(Array.isArray(setup.viewers) && setup.viewers.length === 2 && new Set(setup.viewers).size === 2 &&
    setup.viewers.every((token) => typeof token === "string" && /^\S{32,4096}$/.test(token)));
  secrets = setup.viewers;
  stage = "launch";
  const require = createRequire(import.meta.url);
  const { chromium } = require(join(process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES, "playwright"));
  browser = await chromium.launch({ headless: true, chromiumSandbox: true, timeout: 15_000,
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
    args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] });
  stage = "ready";
  for (const [index, tenant] of ["tenant-a", "tenant-b"].entries()) await openViewer(tenant, secrets[index]);
  process.stdout.write(JSON.stringify({ status: "READY", tenants: 2 }) + "\n");
  stage = "await_native";
  const peak = await frame(["phase", "workers"]);
  if (peak.phase === "readiness_close") {
    assert.deepEqual(peak.workers, []);
    stage = "readiness_close";
  } else {
    stage = "peak";
    assert.equal(peak.phase, "peak");
    assert.ok(Array.isArray(peak.workers) && peak.workers.length === 3);
    const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
    for (const worker of peak.workers) {
      assert.deepEqual(Object.keys(worker).sort(), ["attempt_id", "container_id", "image_id", "session_id", "stable_name", "started_at", "worker_id"]);
      assert.ok([worker.worker_id, worker.session_id, worker.attempt_id].every((id) => typeof id === "string" && uuid.test(id)));
      assert.match(worker.container_id, /^[0-9a-f]{64}$/);
      assert.match(worker.image_id, /^sha256:[0-9a-f]{64}$/);
      assert.ok(typeof worker.started_at === "string" && Number.isFinite(Date.parse(worker.started_at)));
    }
    for (const field of ["worker_id", "session_id", "attempt_id", "container_id"])
      assert.equal(new Set(peak.workers.map((worker) => worker[field])).size, 3);
    const projected = await Promise.allSettled([
      readPeak(sessions[0], peak.workers.slice(0, 2), runtimeKind),
      readPeak(sessions[1], peak.workers.slice(2), runtimeKind),
    ]);
    peakReads.push(...projected.map((result, index) => ({
      tenant: ["A", "B"][index],
      ...(result.status === "fulfilled" ? result.value.report : { code: "unexpected" }),
    })));
    assert.ok(projected.every((result) => result.status === "fulfilled" && result.value.view));
    for (const { guard, errors } of sessions) { assert.equal(guard.violations.length, 0); assert.equal(errors, 0); }
    process.stdout.write(JSON.stringify({ status: "DISPLAYED", rows: [2, 1], views: projected.map(({ value }) => value.view) }) + "\n");
    stage = "await_native";
    // Remain open until the Rust parent has performed its native inspection.
    const { phase } = await frame(["phase"]);
    assert.ok(phase === "logout_close" || phase === "close");
    if (phase === "logout_close") {
      stage = "logout";
      const results = await Promise.allSettled([
        logoutViewer(sessions[0], peak.workers.slice(0, 2)),
        logoutViewer(sessions[1], peak.workers.slice(2)),
      ]);
      logoutChecks.push(...results.map((result) => result.status === "fulfilled" ? result.value : "unexpected"));
      assert.deepEqual(logoutChecks, ["verified", "verified"]);
      process.stdout.write(JSON.stringify({ status: "LOGGED_OUT", tenants: 2 }) + "\n");
    }
    stage = "complete";
    status = phase === "logout_close" ? "PASS" : "CLOSED";
  }
} catch {
  if (stage === "launch") status = "BLOCKED";
  // Never capture a login form or turn a native/shutdown failure into a stale UI artifact.
  for (const { page } of ["ready", "peak"].includes(stage) ? sessions : []) {
    const capture = await captureAuthenticated(page);
    if (capture) captures.push(capture);
  }
  if (stage === "logout" && sessions.every(({ guard }) => guard.violations.length === 0)) {
    const artifacts = await Promise.allSettled(sessions.map(({ page }) => captureAuthenticated(page)));
    for (const artifact of artifacts) if (artifact.status === "fulfilled" && artifact.value) captures.push(artifact.value);
  }
} finally {
  input.close();
  process.stdin.pause();
  const cleanup = [];
  for (const { context } of sessions) cleanup.push(...await closeBrowserResources(context, null));
  cleanup.push(...await closeBrowserResources(null, browser));
  cleanupFailed = cleanup.length > 0 || sessions.some(({ page }) => page && !page.isClosed());
  networkViolation = sessions.some(({ guard }) => guard?.violations.length);
  if (stage === "readiness_close") {
    const pageFailed = sessions.some(({ errors }) => errors !== 0);
    stage = pageFailed ? "views" : "complete";
    status = pageFailed ? "FAIL" : "CLOSED";
  }
  if ((status === "PASS" || status === "CLOSED") && (cleanupFailed || networkViolation)) {
    stage = cleanupFailed ? "cleanup" : "network";
    status = "FAIL";
  }
}
const closed = status === "PASS" || status === "CLOSED";
process.stdout.write(JSON.stringify({ status, stage, ...(closed ? {} : {
  captures, capture_privacy: captures.length ? "sanitized_private_html_only" : "not_captured",
  ...(stage === "peak" && peakReads.length ? { peak_reads: peakReads } : {}),
  ...(stage === "logout" ? { logout_checks: logoutChecks } : {}),
  cleanup_failed: cleanupFailed, network_violation: networkViolation,
}) }) + "\n");
process.exitCode = closed ? 0 : status === "BLOCKED" ? 2 : 1;
