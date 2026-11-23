// Real owned Rust gateway/dashboard + SQLite. Test-only; no fake HTTP server,
// response substitution, fake clock, worker, provider or external destination.
// Compile/register the ignored fixture first in Root's admitted QA. This file
// has not itself been executed merely by being committed.
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, createReadStream, constants, fsyncSync, ftruncateSync, openSync, writeSync } from "node:fs";
import { access, chmod, lstat, open, readdir, realpath, rename, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { isAbsolute, join, relative, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { saveBrowserFailure } from "./browser_failure.mjs";

process.umask(0o077);
const fixtureMode = process.env.AI_GUARD_OWNED_BROWSER_FIXTURE;
const egressMode = fixtureMode === "run-egress-v1";
const workersMode = fixtureMode === "run-workers-v1";
const usageMode = fixtureMode === "run-usage-v1";
const membershipMode = fixtureMode === "run-membership-v1";
const grantMode = fixtureMode === "run-grants-v1";
const FQN = egressMode
  ? "api::human_sources::tests::owned_browser_fixture::egress::serve_owned_egress_browser_fixture"
  : "api::human_sources::tests::owned_browser_fixture::serve_owned_browser_fixture";
const PRIVATE_CONTENT = "owned-browser-private-content-must-not-appear-in-metadata";
const workersPath = "/api/v1/human/workers";
const workerReceiptKind = "control.identity.workers.access";
const summaryPath = "/api/v1/human/summary";
const quotaPath = "/api/v1/human/admin/quota";
const usagePath = "/api/v1/human/admin/usage";
const usagePagePath = `${usagePath}?limit=50`;
const membershipsPath = "/api/v1/human/admin/memberships";
const membershipPagePath = `${membershipsPath}?limit=50`;
const membershipReceiptFields = ["version", "operation_id", "membership_id", "membership_revision", "authorization_revision", "audit_seq"];
const grantsPath = "/api/v1/human/admin/grants";
const grantResourcePath = (resource) => `${grantsPath}?${new URLSearchParams({ resource_kind: resource.kind, resource_id: resource.resource_id })}`;
const grantReceiptFields = ["version", "operation_id", "resource", "grant_revision", "authorization_revision", "audit_seq"];
const QUIET_MS = 33000; // Actual time, covers the existing30s dashboard cycle.
const must = (condition, code) => { if (!condition) throw new Error(code); };
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const closed = (value, keys, code) => must(object(value) && same(Object.keys(value).sort(), [...keys].sort()), code);
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const shaFile = async (path) => { const hash = createHash("sha256"); for await (const bytes of createReadStream(path)) hash.update(bytes); return hash.digest("hex"); };
const exists = async (path) => access(path).then(() => true, (error) => { if (error.code === "ENOENT") return false; throw error; });
const below = (root, path) => { const part = relative(root, path); return part !== "" && part !== ".." && !part.startsWith("../") && !isAbsolute(part); };
const uuid = (value) => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
const privateValues = [PRIVATE_CONTENT];
if (egressMode) privateValues.push("owned-egress-fixture.invalid");
const hasPrivate = (bytes) => privateValues.some((value) => bytes.includes(Buffer.from(value)));
let stage = "preconditions", child, childDone, browser, context, page, ready, bearer = "", interrupted = false;
let authenticatedDashboard = false;
let interruptionSignal = null, terminalResult = null, terminalResultFd;
let sequence = 0, fixtureStarted = false, fixtureExit = null, interactionDeadline = 0, artifactsAdmitted = false;
const commands = [], requests = [], wires = [], pendingWires = [], captures = [], pageErrors = [], external = [];
const observed = {}, output = { stdout: { bytes: 0, hash: createHash("sha256") }, stderr: { bytes: 0, hash: createHash("sha256") } };
let artifacts, control, libtest, dist, seconds, failure = null, cleanupFailure = null, closedFixture = null, receipts = null;
let firstMembershipPost, replayedMembershipReceipt, firstGrantPost, replayedGrantReceipt;
const remaining = (cap = 45000) => {
  must(!interrupted, "INTERRUPTED"); const left = interactionDeadline - Date.now(); must(left > 0, "FIXTURE_INTERACTION_BUDGET"); return Math.min(cap, left);
};
async function bounded(promise, milliseconds, code) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(code)), milliseconds); })]); }
  finally { clearTimeout(timer); }
}
const redact = (text) => {
  let value = String(text).replace(/aigh1_[0-9a-f]+/gi, "[fixture bearer redacted]").replace(/Bearer\s+\S+/gi, "Bearer [redacted]");
  for (const secret of privateValues) value = value.split(secret).join("[fixture data redacted]");
  return value.slice(0, 4096);
};
async function normalizedDirectory(path, empty) {
  must(typeof path === "string" && isAbsolute(path) && path === resolve(path), "DIRECTORY_ABSOLUTE_NORMALIZED");
  const facts = await lstat(path); must(facts.isDirectory() && !facts.isSymbolicLink() && facts.uid === process.getuid() && (facts.mode & 0o777) === 0o700, "DIRECTORY_PRIVATE_0700");
  must(await realpath(path) === path, "DIRECTORY_ALIAS_REFUSED");
  if (empty) must((await readdir(path)).length === 0, "DIRECTORY_NOT_FRESH");
  return { dev: facts.dev, ino: facts.ino };
}
async function privateJson(path, cap, allowPartial = false) {
  let file;
  try {
    file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const facts = await file.stat();
    must(facts.isFile() && facts.uid === process.getuid() && facts.nlink === 1 && (facts.mode & 0o777) === 0o600 && facts.size <= cap, "PRIVATE_JSON_METADATA");
    // The Rust publisher creates with O_EXCL before writing/fsync. A private
    // file may therefore exist while empty/partial; existence is not an ACK.
    const buffer = Buffer.alloc(cap + 1), read = await file.read(buffer, 0, buffer.length, 0), after = await file.stat();
    must(read.bytesRead <= cap && after.size <= cap, "PRIVATE_JSON_SIZE");
    const bytes = buffer.subarray(0, read.bytesRead);
    if (bytes.length === 0 || bytes.length !== facts.size || after.size !== facts.size || after.mtimeMs !== facts.mtimeMs) throw new Error(allowPartial ? "PRIVATE_JSON_PARTIAL" : "PRIVATE_JSON_CHANGED");
    let value; try { value = JSON.parse(bytes.toString("utf8")); } catch { throw new Error(allowPartial ? "PRIVATE_JSON_PARTIAL" : "PRIVATE_JSON_INVALID"); }
    return { value, bytes, sha256: digest(bytes), identity: `${facts.dev}:${facts.ino}` };
  } finally { await file?.close(); }
}
async function waitJson(path, cap, deadline, allowInterrupted = false) {
  let previous = null;
  while (Date.now() < deadline) {
    if (!allowInterrupted) must(!interrupted, "INTERRUPTED");
    let candidate = null;
    try { candidate = await privateJson(path, cap, true); }
    catch (error) { if (error.code !== "ENOENT" && error.message !== "PRIVATE_JSON_PARTIAL") throw error; }
    // Two complete identical reads, separated by the bounded polling interval.
    const stable = candidate ? `${candidate.identity}:${candidate.sha256}` : null;
    if (candidate && stable === previous) return candidate;
    previous = stable;
    if (fixtureExit !== null && candidate === null) throw new Error("FIXTURE_EXITED_BEFORE_COMPLETE_CONTROL_FILE");
    await delay(100);
  }
  throw new Error("CONTROL_FILE_DEADLINE");
}
async function save(name, value) {
  const bytes = Buffer.from(JSON.stringify(value, null, 2) + "\n");
  must(!hasPrivate(bytes), "SECRET_IN_ARTIFACT_REFUSED");
  await writeFile(join(artifacts, name), bytes, { mode: 0o600, flag: "wx" });
}
function publishTerminalResult() {
  terminalResult.status = interrupted ? "FAIL_NOT_QUALIFIED" : !fixtureStarted ? "BLOCKED_NOT_RUN"
    : failure || cleanupFailure ? "FAIL_NOT_QUALIFIED"
    : grantMode ? "PASS_OWNED_GRANT_RECOVERY"
    : membershipMode ? "PASS_OWNED_MEMBERSHIP_RECOVERY"
    : usageMode ? "PASS_OWNED_TENANT_USAGE"
    : workersMode ? "PASS_OWNED_WORKER_REGISTRY"
    : egressMode ? "PASS_OWNED_EGRESS_JOURNAL" : "PASS_THREE_BOUNDED_OWNED_SCENARIOS";
  terminalResult.interrupted = interrupted;
  terminalResult.interruption = interrupted ? { code: "INTERRUPTED", signal: interruptionSignal } : null;
  const bytes = Buffer.from(JSON.stringify(terminalResult, null, 2) + "\n");
  must(!hasPrivate(bytes), "SECRET_IN_ARTIFACT_REFUSED");
  // Retain only this exclusively created result FD. A signal handled during or
  // after publication rewrites it to failure without reopening a mutable path.
  terminalResultFd ??= openSync(join(artifacts, "RESULT.json"), constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  ftruncateSync(terminalResultFd, 0);
  for (let offset = 0; offset < bytes.length;) {
    const written = writeSync(terminalResultFd, bytes, offset, bytes.length - offset, offset);
    must(written > 0, "RESULT_WRITE_INCOMPLETE"); offset += written;
  }
  fsyncSync(terminalResultFd);
}
process.once("exit", () => { if (terminalResultFd !== undefined) { try { closeSync(terminalResultFd); } catch {} } });
async function finishWireCaptures() {
  const settled = await bounded(Promise.allSettled(pendingWires), 10000, "WIRE_CAPTURE_DEADLINE");
  must(settled.every((item) => item.status === "fulfilled"), "WIRE_CAPTURE_FAILED");
}
async function command(action, cleanup = false) {
  sequence++; must(sequence <= 32, "COMMAND_LIMIT");
  const name = String(sequence).padStart(4, "0") + ".json";
  const directory = join(control, "commands"), temporary = join(directory, "." + name + ".part"), target = join(directory, name);
  await normalizedDirectory(directory, false);
  must(!await exists(target), "COMMAND_OVERWRITE_REFUSED");
  const bytes = Buffer.from(JSON.stringify({ version: 1, sequence, action }) + "\n"); must(bytes.length <= 512, "COMMAND_SIZE");
  const file = await open(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { await file.writeFile(bytes); await file.sync(); } finally { await file.close(); }
  await rename(temporary, target);
  const parent = await open(directory, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { await parent.sync(); } finally { await parent.close(); }
  const deadline = cleanup ? Date.now() + 15000 : Date.now() + remaining(30000);
  const ackFile = await waitJson(join(control, "acks", name), 4096, deadline, cleanup);
  const ack = ackFile.value;
  must(object(ack), "ACK_OBJECT");
  const fields = ["version", "sequence", "action", "command_sha256", "status"];
  if (egressMode) {
    if (action === "revoke_session_policy") fields.push("receipt");
  } else {
    fields.push("own_sqlite_locked");
    if (Object.hasOwn(ack, "code")) fields.push("code");
  }
  closed(ack, fields, "ACK_CLOSED");
  must(ack.version === 1 && ack.sequence === sequence && ack.action === action && ack.command_sha256 === digest(bytes), "ACK_CORRELATION");
  const status = { lock_own_sqlite: "locked", unlock_own_sqlite: "unlocked", revoke_session_policy: "revocation_requested", stop: "stopping" }[action];
  must(ack.status === status, "ACK_REJECTED_OR_WRONG_STATE");
  if (egressMode && action === "revoke_session_policy") policyReceipt(ack.receipt, "revoke_requested");
  if (!egressMode) must(ack.own_sqlite_locked === (action === "lock_own_sqlite") && !Object.hasOwn(ack, "code"), "ACK_SQLITE_STATE");
  commands.push({ command: { version: 1, sequence, action }, command_sha256: digest(bytes), ack, ack_sha256: ackFile.sha256 });
  return ack;
}
function policyReceipt(value, state) {
  closed(value, ["operation_id", "event_id", "event_seq", "event_hash", "received_at", "revision", "before_epoch", "after_epoch", "state"], "POLICY_RECEIPT_CLOSED");
  must(uuid(value.operation_id) && value.event_id === value.operation_id
    && typeof value.event_hash === "string" && /^[0-9a-f]{64}$/.test(value.event_hash)
    && typeof value.received_at === "string" && Number.isFinite(Date.parse(value.received_at))
    && [value.event_seq, value.revision, value.after_epoch].every((number) => Number.isSafeInteger(number) && number > 0)
    && Number.isSafeInteger(value.before_epoch) && value.before_epoch >= 0
    && value.after_epoch === value.before_epoch + 1 && value.state === state, "POLICY_RECEIPT_FIELDS");
  return value;
}
function validateEgressReady(value) {
  closed(value, ["version", "mode", "dashboard_url", "tenant_id", "resource", "owner", "reader", "tenant_policy", "session_policy", "budget_seconds", "commands", "acks", "command_limit"], "READY_CLOSED");
  must(value.mode === "egress_journal" && value.tenant_id === "A", "READY_EGRESS_MODE");
  closed(value.resource, ["kind", "resource_id"], "READY_RESOURCE");
  must(value.resource.kind === "session" && typeof value.resource.resource_id === "string"
    && /^[A-Za-z0-9_-]{1,128}$/.test(value.resource.resource_id), "READY_RESOURCE_SCOPE");
  for (const [account, name] of [[value.owner, "Synthetic E1 Journal owner"], [value.reader, "Synthetic E1 Journal reader"]]) {
    closed(account, ["display_name", "bearer"], "READY_ACCOUNT");
    must(account.display_name === name && typeof account.bearer === "string" && /^aigh1_[0-9a-f]{64}$/.test(account.bearer), "READY_FIXTURE_BEARER");
  }
  must(value.owner.bearer !== value.reader.bearer, "READY_DISTINCT_BEARERS");
  policyReceipt(value.tenant_policy, "active");
  policyReceipt(value.session_policy, "active");
  must(value.tenant_policy.event_seq < value.session_policy.event_seq && value.tenant_policy.event_id !== value.session_policy.event_id, "READY_POLICY_ORDER");
  return value;
}
function validateReady(value) {
  must(object(value) && value.version === 1 && value.budget_seconds === seconds &&
    value.commands === "commands" && value.acks === "acks" && value.command_limit === 32, "READY_PROFILE");
  const url = new URL(value.dashboard_url);
  must(url.protocol === "http:" && url.hostname === "127.0.0.1" && Number(url.port) > 0 && !url.username && !url.password && url.pathname === "/" && !url.search && !url.hash, "READY_LOOPBACK_URL");
  if (egressMode) return validateEgressReady(value);
  const fields = ["version", "dashboard_url", "bearer_b", "tenant_id", "budget_seconds", "commands", "acks", "command_limit"];
  const scenarioFields = grantMode ? ["mode", "grant"] : membershipMode ? ["mode", "membership"] : usageMode ? ["mode", "usage"]
    : workersMode ? ["mode", "worker"] : ["resource", "event_ids"];
  closed(value, [...fields, ...scenarioFields], "READY_CLOSED");
  must(value.tenant_id === "B", "READY_PROFILE");
  must(typeof value.bearer_b === "string" && /^aigh1_[0-9a-f]{64}$/.test(value.bearer_b), "READY_FIXTURE_BEARER");
  if (grantMode) {
    must(value.mode === "grant", "READY_GRANT_MODE");
    closed(value.grant, ["resource", "revision", "member_display_name", "member_principal"], "READY_GRANT");
    closed(value.grant.resource, ["kind", "resource_id"], "READY_GRANT_RESOURCE");
    must(value.grant.resource.kind === "session" && value.grant.resource.resource_id === "owned-browser-session-b" &&
      value.grant.revision === "1" && value.grant.member_display_name === "Synthetic browser member" &&
      object(value.grant.member_principal) && value.grant.member_principal.kind === "human_local" &&
      uuid(value.grant.member_principal.installation_id) && uuid(value.grant.member_principal.account_id), "READY_EXISTING_GRANT");
    return value;
  }
  if (membershipMode) {
    must(value.mode === "membership", "READY_MEMBERSHIP_MODE");
    closed(value.membership, ["membership_id", "display_name", "revision"], "READY_MEMBERSHIP");
    must(uuid(value.membership.membership_id) && value.membership.display_name === "Synthetic browser member" &&
      value.membership.revision === "1", "READY_SYNTHETIC_MEMBERSHIP");
    return value;
  }
  if (usageMode) {
    must(value.mode === "usage", "READY_USAGE_MODE");
    closed(value.usage, ["allocated", "unmeasured"], "READY_USAGE_ATTEMPTS");
    const { allocated, unmeasured } = value.usage;
    closed(allocated, ["worker_id", "attempt_id", "terminal_observation_id"], "READY_ALLOCATED_ATTEMPT");
    closed(unmeasured, ["worker_id", "attempt_id"], "READY_UNMEASURED_ATTEMPT");
    must(Object.values(allocated).every(uuid) && Object.values(unmeasured).every(uuid) &&
      allocated.worker_id !== unmeasured.worker_id && allocated.attempt_id !== unmeasured.attempt_id, "READY_DISTINCT_USAGE_ATTEMPTS");
    return value;
  }
  if (workersMode) {
    must(value.mode === "worker_registry", "READY_WORKER_MODE");
    closed(value.worker, ["worker_id", "attempt_id", "session_id"], "READY_RESERVED_WORKER");
    must(Object.values(value.worker).every(uuid), "READY_RESERVED_WORKER_IDS");
    return value;
  }
  closed(value.resource, ["kind", "resource_id"], "READY_RESOURCE");
  must(value.resource.kind === "session" && value.resource.resource_id === "owned-browser-session-b", "READY_RESOURCE_SCOPE");
  must(Array.isArray(value.event_ids) && value.event_ids.length === 2 && new Set(value.event_ids).size === 2 && value.event_ids.every((id) => typeof id === "string" && /^[0-9a-f-]{36}$/.test(id)), "READY_EVENTS");
  return value;
}
const recentPath = (path) => new URL(path, "http://fixture.invalid").pathname === "/api/v1/human/events/recent";
const eventDetailPrefix = "/api/v1/human/events/";
function pageAction(path) {
  const pathname = new URL(path, "http://fixture.invalid").pathname;
  if (pathname.startsWith(eventDetailPrefix) && uuid(pathname.slice(eventDetailPrefix.length))) return "event";
  if (pathname === workersPath) return workerReceiptKind;
  if (pathname === quotaPath) return "quota";
  if (pathname === usagePath) return "usage";
  if (membershipMode && pathname === membershipsPath) return "memberships";
  if (grantMode && pathname === membershipsPath) return "memberships";
  if (grantMode && pathname === grantsPath) return "grants";
  return ({ "/api/v1/human/resources": "resources", "/api/v1/human/events": "events", "/api/v1/human/events/recent": "events" })[pathname] ?? null;
}
function requiredReceipt(body) {
  must(object(body), "PAGE_BODY_REQUIRED");
  const receipt = body.read_receipt;
  closed(receipt, ["request_id", "audit_event_id", "received_at"], "PAGE_RECEIPT_REQUIRED");
  must(uuid(receipt.request_id) && uuid(receipt.audit_event_id) && typeof receipt.received_at === "string" && /^\d{4}-\d\d-\d\dT.+(?:Z|[+-]\d\d:\d\d)$/.test(receipt.received_at) && Number.isFinite(Date.parse(receipt.received_at)), "PAGE_RECEIPT_TYPES");
  return receipt;
}
const countRecent = (from) => requests.slice(from).filter((row) => recentPath(row.path)).length;
const streamRequests = (from) => requests.slice(from).filter((row) => row.path.startsWith("/api/v1/human/stream"));
const button = (name) => page.getByRole("button", { name, exact: true });
const heading = (name) => page.getByRole("heading", { name, exact: true });
const journal = () => page.locator(".sv-journal");
const journalRows = () => journal().locator("tbody tr");
async function settle() { await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))); }
async function noSecret() {
  const text = await page.locator("body").innerText();
  must(!hasPrivate(Buffer.from(text)), "PRIVATE_MATERIAL_RENDERED");
}
async function captureResponse(response) {
  const path = new URL(response.url()).pathname;
  if (!path.startsWith("/api/v1/human/") || path === "/api/v1/human/stream") return;
  const method = response.request().method();
  const bytes = await response.body(); must(bytes.length <= 262144, "WIRE_RESPONSE_SIZE");
  must(!hasPrivate(bytes), "PRIVATE_MATERIAL_IN_WIRE");
  let body; try { body = JSON.parse(bytes.toString("utf8")); } catch { throw new Error("WIRE_RESPONSE_NOT_JSON"); }
  wires.push({ method, path: new URL(response.url()).pathname + new URL(response.url()).search,
    status: response.status(), bytes: bytes.length, sha256: digest(bytes), body });
  if (method === "GET" && response.status() === 200 && pageAction(path) !== null) requiredReceipt(body);
}
async function waitForRead(path, status) {
  const actual = await page.waitForResponse((item) => {
    const { pathname, search } = new URL(item.url());
    return item.request().method() === "GET" && (pathname === path || pathname + search === path);
  }, { timeout: remaining() });
  must(actual.status() === status, "UNEXPECTED_REAL_HTTP_STATUS");
  const bytes = await actual.body(); must(bytes.length <= 262144 && !hasPrivate(bytes), "ACTION_WIRE_PRIVACY_OR_SIZE");
  const body = JSON.parse(bytes.toString("utf8"));
  if (status === 200 && pageAction(path) !== null) requiredReceipt(body);
  return body;
}
async function readByAction(path, status, action) {
  const response = waitForRead(path, status);
  response.catch(() => {}); // An action failure must not leave an unhandled waiter.
  await action();
  return response;
}
async function resourcesVisible(body) {
  const receipt = requiredReceipt(body), panel = page.locator(".human-resource-panel");
  await heading("Ressources autorisées.").waitFor({ timeout: remaining() });
  await panel.locator(".control-status").getByText("Lecture réussie", { exact: true }).waitFor({ timeout: remaining() });
  await panel.getByText(ready.resource.resource_id, { exact: true }).waitFor({ timeout: remaining() });
  const stamped = await page.evaluate((date) => new Intl.DateTimeFormat("fr-FR", { dateStyle: "medium", timeStyle: "medium" }).format(new Date(date)), receipt.received_at);
  await panel.getByText(`Reçu du contrôle : ${stamped}`, { exact: true }).waitFor({ timeout: remaining() });
}
async function snapshotVisible(body) {
  must(body.version === 1 && body.view === "metadata" && body.order === "newest_first" && body.coverage === "authorized_selection" && body.stream_mode === "snapshot_only" && body.live_cursor === null, "REAL_SNAPSHOT_PROFILE");
  must(Array.isArray(body.items) && body.items.every((item) => item.content === null) && same(body.items.map((item) => item.metadata.id), [...ready.event_ids].reverse()), "REAL_SNAPSHOT_SELECTION_ORDER");
  must(body.has_more === false && body.next_cursor === null && object(body.read_receipt), "REAL_SNAPSHOT_RECEIPT_OR_BOUND");
  await journal().locator(".sv-source", { hasText: "Journal" }).getByText("Lecture réussie", { exact: true }).waitFor({ timeout: remaining() });
  await journal().locator(".sv-source", { hasText: "Journal" }).getByText(body.read_receipt.audit_event_id, { exact: false }).waitFor({ timeout: remaining() });
  await journal().locator(".sv-source", { hasText: "Direct" }).getByText("Direct indisponible pour ce profil", { exact: true }).waitFor({ timeout: remaining() });
  must(same(await journalRows().evaluateAll((rows) => rows.map((row) => row.dataset.eventId)), [...ready.event_ids].reverse()), "DOM_REAL_EVENT_IDS");
  const expectedTime = await page.evaluate((date) => new Intl.DateTimeFormat("fr-FR", { dateStyle: "medium", timeStyle: "medium" }).format(new Date(date)), body.read_receipt.received_at);
  const source = await journal().locator(".sv-source", { hasText: "Journal" }).innerText();
  must(source.includes(body.read_receipt.audit_event_id) && source.includes(expectedTime) && source.includes("Dernière page lue le"), "DOM_REAL_RECEIPT_AND_TIME");
  must((await page.locator(".stream-pill").innerText()).includes("direct indisponible pour ce profil"), "HOST_SNAPSHOT_STATUS");
  await noSecret();
}
async function refresh(status) {
  const from = requests.length;
  await button("Actualiser").focus();
  const body = await readByAction("/api/v1/human/events/recent", status, () => button("Actualiser").press("Enter"));
  await settle();
  must(countRecent(from) === 1, "REFRESH_MUST_BE_ONE_REAL_RECENT_GET");
  must(await page.evaluate(() => document.activeElement?.getAttribute("data-focus-key")) === "human-refresh", "REFRESH_FOCUS_LOST");
  return body;
}

async function openEgressAccount(account, allowed) {
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(account.bearer);
  await button("Ouvrir mon espace").click();
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  const resources = await readByAction("/api/v1/human/resources", 200, () => button("Ouvrir personnel · A").click());
  await heading("Ressources autorisées.").waitFor({ timeout: remaining() });
  must(Array.isArray(resources.items), "EGRESS_RESOURCE_PAGE");
  const grant = resources.items.find((item) => item.resource.resource_id === ready.resource.resource_id);
  if (allowed) {
    must(grant?.resource.kind === "session" && grant.permissions.metadata === true
      && grant.permissions.content === false && grant.permissions.export === false, "EGRESS_METADATA_GRANT");
    await resourcesVisible(resources);
  } else {
    must(!grant, "EGRESS_PRIVATE_SESSION_SHARED");
  }
  authenticatedDashboard = true;
}

async function egressSnapshot(body, entries) {
  must(body.version === 1 && body.view === "metadata" && body.order === "newest_first"
    && body.coverage === "authorized_selection" && body.stream_mode === "snapshot_only"
    && body.live_cursor === null && body.next_cursor === null && body.has_more === false, "EGRESS_SNAPSHOT_PROFILE");
  must(Array.isArray(body.items) && body.items.every((item) => object(item.metadata) && item.content === null), "EGRESS_METADATA_ONLY");
  const policies = body.items.map((item) => item.metadata)
    .filter((item) => item.kind.startsWith("control.egress_policy."));
  must(same(policies.map((item) => item.id), entries.map(({ receipt }) => receipt.event_id)), "EGRESS_POLICY_SELECTION_ORDER");
  const pageReceipt = requiredReceipt(body);
  await journal().locator(".sv-source", { hasText: "Journal" }).getByText(pageReceipt.audit_event_id, { exact: false }).waitFor({ timeout: remaining() });
  await journal().locator(".sv-source", { hasText: "Direct" }).getByText("Direct indisponible pour ce profil", { exact: true }).waitFor({ timeout: remaining() });
  must(await journalRows().count() === body.items.length, "EGRESS_DOM_PAGE_COUNT");
  for (const [index, { receipt, kind, label }] of entries.entries()) {
    const meta = policies[index];
    closed(meta, ["id", "timestamp", "received_at", "kind", "source", "provenance", "coverage", "resource", "decision"], "EGRESS_EVENT_METADATA_CLOSED");
    must(meta.kind === kind && meta.source === "gateway" && meta.provenance === "observed"
      && meta.coverage === "native" && meta.decision === null
      && meta.resource?.kind === ready.resource.kind && meta.resource?.resource_id === ready.resource.resource_id
      && Date.parse(meta.received_at) === Date.parse(receipt.received_at), "EGRESS_SQLITE_RECEIPT_TO_WIRE");
    const row = journal().locator(`tbody tr[data-event-id="${receipt.event_id}"]`);
    await row.waitFor({ timeout: remaining() });
    await row.scrollIntoViewIfNeeded({ timeout: remaining() });
    must(await row.locator('[data-label="Type"] strong').innerText() === label
      && await row.locator('[data-label="Type"] code').innerText() === kind, "EGRESS_DOM_POLICY_LABEL");
    await row.locator('[data-label="Décision"]').getByText("Aucune décision de connexion", { exact: true }).waitFor({ timeout: remaining() });
    await row.locator('[data-label="Provenance"]').getByText("observed · couverture native du registre", { exact: true }).waitFor({ timeout: remaining() });
    await row.locator('[data-label="Ressource"]').getByText(ready.resource.resource_id, { exact: true }).waitFor({ timeout: remaining() });
    must((await row.locator('[data-label="Reçu le"] p').first().innerText()).trim().length > 0, "EGRESS_DOM_RECEIVED_TIME");
  }
  await noSecret();
}

async function runEgressJournal() {
  const replacement = {
    receipt: ready.session_policy, kind: "control.egress_policy.replaced", label: "Politique réseau remplacée",
  };
  stage = "egress_owner_snapshot";
  await openEgressAccount(ready.owner, true);
  const journalStart = requests.length;
  let body = await readByAction("/api/v1/human/events/recent", 200, () => button("Journal").click());
  await egressSnapshot(body, [replacement]);
  must(countRecent(journalStart) === 1 && streamRequests(journalStart).length === 0, "EGRESS_INITIAL_JOURNAL_READ");

  stage = "egress_revoke_and_refresh";
  const { receipt } = await command("revoke_session_policy");
  must(receipt.event_id !== replacement.receipt.event_id && receipt.event_seq > replacement.receipt.event_seq
    && receipt.revision === replacement.receipt.revision && receipt.before_epoch === replacement.receipt.after_epoch, "EGRESS_REVOKE_SAME_POLICY");
  must(await journal().locator(`tr[data-event-id="${receipt.event_id}"]`).count() === 0, "EGRESS_SNAPSHOT_READ_BEFORE_REFRESH");
  body = await refresh(200);
  await egressSnapshot(body, [
    { receipt, kind: "control.egress_policy.revoke_requested", label: "Révocation de politique réseau demandée" },
    replacement,
  ]);
  must(streamRequests(journalStart).length === 0, "EGRESS_UNEXPECTED_STREAM");

  stage = "egress_reader_without_share";
  await button("Se déconnecter").click();
  authenticatedDashboard = false;
  await page.getByLabel("Credential personnel local").waitFor({ timeout: remaining() });
  await openEgressAccount(ready.reader, false);
  const readerStart = requests.length;
  body = await readByAction("/api/v1/human/events/recent", 200, () => button("Journal").click());
  await egressSnapshot(body, []);
  must(!body.items.some((item) => item.metadata.resource.resource_id === ready.resource.resource_id)
    && await journal().locator(`tr[data-event-id="${receipt.event_id}"], tr[data-event-id="${replacement.receipt.event_id}"]`).count() === 0
    && countRecent(readerStart) === 1 && streamRequests(readerStart).length === 0, "EGRESS_PRIVATE_POLICY_VISIBLE");
  observed.policy = { replacement: replacement.receipt, revocation_requested: receipt, reader_access: "absent" };
  must(pageErrors.length === 0 && external.length === 0, "PAGE_ERROR_OR_EXTERNAL_DESTINATION");
}

function opaqueRgb(value) {
  const match = /^(rgb|rgba)\(([^)]+)\)$/.exec(value);
  must(match !== null, "CURRENT_NAVIGATION_COLOR_SYNTAX_UNSUPPORTED");
  const parts = match[2].split(",").map((part) => part.trim());
  must(parts.length === (match[1] === "rgba" ? 4 : 3) && parts.every((part) => /^(?:\d+(?:\.\d+)?|\.\d+)$/.test(part)), "CURRENT_NAVIGATION_COLOR_CHANNELS_UNSUPPORTED");
  const channels = parts.map(Number);
  must(channels.every(Number.isFinite) && channels.slice(0, 3).every((channel) => channel >= 0 && channel <= 255), "CURRENT_NAVIGATION_COLOR_RANGE");
  must(channels.length === 3 || channels[3] === 1, "CURRENT_NAVIGATION_ALPHA_UNSUPPORTED");
  return channels.slice(0, 3);
}
function relativeLuminance(channels) {
  const linear = channels.map((channel) => {
    const srgb = channel / 255;
    return srgb <= 0.04045 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}
async function currentNavigationContrast(width, theme) {
  const current = page.locator('.human-nav .button[aria-current="page"]');
  must(await current.count() === 1 && await current.isEnabled() && await current.getAttribute("aria-disabled") !== "true", "CURRENT_NAVIGATION_MUST_BE_ENABLED");
  await page.mouse.move(width - 1, 999);
  await current.evaluate((element) => { if (document.activeElement === element) element.blur(); });
  for (const state of ["normal", "hover"]) {
    if (state === "hover") await current.hover({ timeout: remaining() });
    const styles = await bounded(current.evaluate(async (element) => {
      const targets = [];
      for (let target = element; target; target = target.parentElement) targets.push(target);
      const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));
      await frame(); getComputedStyle(element).backgroundColor; await frame();
      // Wait for real CSS transitions, including inherited ancestor effects.
      // An unsupported or never-settling animation is a refusal, not a sample.
      for (let attempt = 0; attempt < 4; attempt++) {
        if (!targets.every((target) => typeof target.getAnimations === "function")) throw new Error("CURRENT_NAVIGATION_ANIMATION_API_UNSUPPORTED");
        const animations = targets.flatMap((target) => target.getAnimations());
        if (animations.some((animation) => animation.playState === "paused")) throw new Error("CURRENT_NAVIGATION_PAUSED_ANIMATION_UNSUPPORTED");
        const active = animations.filter((animation) => animation.pending || animation.playState === "running");
        if (active.length) { await Promise.allSettled(active.map((animation) => animation.finished)); await frame(); continue; }
        const computed = getComputedStyle(element);
        return { color: computed.color, background: computed.backgroundColor, hovered: element.matches(":hover"),
          enabled: element.matches(":enabled") && element.getAttribute("aria-disabled") !== "true",
          opacities: targets.map((target) => getComputedStyle(target).opacity) };
      }
      throw new Error("CURRENT_NAVIGATION_TRANSITIONS_NOT_SETTLED");
    }), remaining(10000), "CURRENT_NAVIGATION_TRANSITION_DEADLINE");
    must(styles.enabled && styles.hovered === (state === "hover"), "CURRENT_NAVIGATION_STATE_NOT_ESTABLISHED");
    must(styles.opacities.every((opacity) => Number.isFinite(Number(opacity)) && Number(opacity) === 1), "CURRENT_NAVIGATION_OPACITY_UNSUPPORTED");
    const foreground = relativeLuminance(opaqueRgb(styles.color)), background = relativeLuminance(opaqueRgb(styles.background));
    const ratio = (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05);
    observed.current_navigation_contrast ??= [];
    observed.current_navigation_contrast.push({ width, theme, state, ...styles, ratio, minimum: 4.5 });
    must(Number.isFinite(ratio) && ratio >= 4.5, "CURRENT_NAVIGATION_TEXT_CONTRAST_BELOW_4_5");
  }
}
async function screenshot(width, theme) {
  await page.setViewportSize({ width, height: 1000 });
  if (await page.locator("html").getAttribute("data-theme") !== theme) await button(theme === "dark" ? "Activer le thème sombre" : "Activer le thème clair").click();
  await page.evaluate(() => window.scrollTo(0, 0)); await settle(); await noSecret();
  await currentNavigationContrast(width, theme);
  const geometry = await page.evaluate(() => {
    const dock = document.querySelector(".human-toolbar").getBoundingClientRect();
    const row = document.querySelector(".human-nav"); const nav = row.getBoundingClientRect();
    const current = row.querySelector('[aria-current="page"]').getBoundingClientRect();
    return { viewport: innerWidth, document: document.documentElement.scrollWidth, body: document.body.scrollWidth,
      headingTop: document.querySelector(".page-heading").getBoundingClientRect().top, dockBottom: dock.bottom,
      currentVisible: current.left >= nav.left - 1 && current.right <= nav.right + 1,
      tablesFit: [...document.querySelectorAll(".sv-table")].every((table) => table.scrollWidth <= table.parentElement.clientWidth + 1) };
  });
  must(geometry.document <= width + 1 && geometry.body <= width + 1 && geometry.headingTop >= geometry.dockBottom - 1 && geometry.currentVisible && geometry.tablesFit, "OWNED_VIEW_GEOMETRY");
  const name = `owned-snapshot-${width}-${theme}.png`;
  await page.screenshot({ path: join(artifacts, name), timeout: remaining() }); await chmod(join(artifacts, name), 0o600);
  captures.push({ name, width, theme, geometry, sha256: await shaFile(join(artifacts, name)) });
}

async function runOwnedSnapshot() {
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer); await button("Ouvrir mon espace").click();
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  const initialResources = await readByAction("/api/v1/human/resources", 200, () => button("Ouvrir personnel · B").click());
  await resourcesVisible(initialResources);
  const grant = initialResources.items.find((item) => item.resource.kind === ready.resource.kind && item.resource.resource_id === ready.resource.resource_id);
  must(grant?.permissions.metadata === true && grant.permissions.content === true && grant.permissions.export === true, "REAL_RESOURCE_GRANTS_REQUIRED");
  observed.initial_unknown_profile_stream_attempts = streamRequests(0).length;
  const journalStart = requests.length;
  let body = await readByAction("/api/v1/human/events/recent", 200, () => button("Journal").click()); await snapshotVisible(body);
  must(countRecent(journalStart) === 1 && streamRequests(journalStart).length === 0, "JOURNAL_START_READ_OR_STREAM");
  const quietStart = requests.length, quietAt = Date.now(); must(remaining(QUIET_MS + 1000) > QUIET_MS, "QUIET_WINDOW_BUDGET");
  await page.waitForTimeout(QUIET_MS); await settle();
  observed.quiet = { elapsed_ms: Date.now() - quietAt, human_requests: requests.length - quietStart, recent_requests: countRecent(quietStart), streams: streamRequests(quietStart).length };
  must(observed.quiet.human_requests === 0, "AUTOMATIC_JOURNAL_READ_OR_STREAM_DURING_REAL_WINDOW");
  body = await refresh(200); await snapshotVisible(body);
  for (const [width, theme] of [[1440, "light"], [1440, "dark"], [320, "light"], [390, "dark"]]) await screenshot(width, theme);
  observed.scenario1 = "real_login_metadata_receipt_refresh_and_no_automatic_journal_traffic";

  stage = "navigation_and_coverage";
  const navigationStart = requests.length;
  await button("Ressources").focus();
  const resources = await readByAction("/api/v1/human/resources", 200, () => button("Ressources").press("Enter"));
  await resourcesVisible(resources);
  const actualGrant = resources.items.find((item) => item.resource.resource_id === ready.resource.resource_id);
  must(actualGrant?.permissions.content === true && actualGrant.permissions.export === true, "RESOURCE_RIGHTS_MUST_NOT_BE_MUTATED");
  must(await page.locator('[data-focus-key="resource-content-0"]').isDisabled(), "CONTENT_CONTROL_NOT_DISABLED");
  must((await page.locator(".human-resource-panel").innerText()).includes("Contenu et export indisponibles pour ce profil"), "PROFILE_COVERAGE_REASON_MISSING");
  const events = await readByAction("/api/v1/human/events", 200, () => page.locator('[data-focus-key="resource-metadata-0"]').click());
  must(events.view === "metadata" && events.items.length === 2 && events.items.every((item) => item.content === null), "REAL_FORWARD_METADATA_ONLY");
  must(await page.locator('[data-focus-key="human-export"]').isDisabled(), "EXPORT_CONTROL_NOT_DISABLED");
  const details = page.locator('.human-event-row [data-focus-key^="human-event-"]');
  must(await details.count() === 2, "REAL_DETAIL_CONTROLS_REQUIRED");
  const eventId = events.items[0].metadata.id;
  must(ready.event_ids.includes(eventId), "DETAIL_ID_NOT_IN_AUTHORIZED_FIXTURE");
  const detailPath = eventDetailPrefix + eventId;
  const detailButton = details.first();
  must(await detailButton.isEnabled(), "METADATA_DETAIL_CONTROL_NOT_ENABLED");
  await detailButton.focus();
  const detailPage = await readByAction(detailPath, 200, () => detailButton.press("Enter"));
  must(detailPage.view === "metadata" && detailPage.context_id === events.context_id &&
    detailPage.authorization_version === events.authorization_version && detailPage.has_more === false &&
    detailPage.next_cursor === null && detailPage.items.length === 1 && detailPage.items[0].metadata.id === eventId &&
    detailPage.items[0].content === null && detailPage.items[0].metadata.resource.kind === ready.resource.kind &&
    detailPage.items[0].metadata.resource.resource_id === ready.resource.resource_id, "REAL_METADATA_DETAIL_SCOPE");
  const dialog = page.getByRole("dialog");
  await dialog.getByText(/^Métadonnées uniquement · Reçu du contrôle le /u).waitFor({ timeout: remaining() });
  const renderedDetail = JSON.parse(await dialog.locator("pre.code-block").textContent());
  must(renderedDetail.metadata.id === eventId && renderedDetail.content === null, "DOM_REAL_METADATA_DETAIL");
  await dialog.getByRole("button", { name: "Fermer les détails", exact: true }).click();
  await page.waitForFunction(() => document.activeElement?.getAttribute("data-focus-key") === "human-event-0", undefined, { timeout: remaining() });
  await button("Journal").focus(); body = await readByAction("/api/v1/human/events/recent", 200, () => button("Journal").press("Enter")); await snapshotVisible(body);
  must(await heading("Journal des événements autorisés.").evaluate((node) => document.activeElement === node), "NAVIGATION_HEADING_FOCUS_LOST");
  must(streamRequests(navigationStart).length === 0, "KNOWN_PROFILE_NAVIGATION_OPENED_STREAM"); await noSecret();
  observed.scenario2 = "real_metadata_detail_with_grants_unchanged_closed_content_export_and_keyboard_focus";

  stage = "real_sqlite_busy_and_recovery";
  const previousReceipt = body.read_receipt.audit_event_id;
  await command("lock_own_sqlite"); const busyStart = requests.length;
  const refused = await refresh(503);
  must(object(refused.error) && ["identity_unavailable", "audit_unavailable"].includes(refused.error.code) && !Object.hasOwn(refused, "items") && !Object.hasOwn(refused, "read_receipt"), "REAL_BUSY_RESPONSE_MUST_NOT_PUBLISH_DATA");
  await journal().getByText("Événements inconnus", { exact: true }).waitFor({ timeout: remaining() });
  must(await journalRows().count() === 0, "BUSY_SNAPSHOT_ROWS_NOT_WITHDRAWN");
  must(!(await journal().innerText()).includes(previousReceipt), "BUSY_SNAPSHOT_RECEIPT_NOT_WITHDRAWN");
  await journal().locator(".sv-source", { hasText: "Direct" }).getByText("Direct indisponible pour ce profil", { exact: true }).waitFor({ timeout: remaining() });
  must((await page.locator(".stream-pill").innerText()).includes("direct indisponible pour ce profil"), "BUSY_LOST_RESTRICTIVE_HOST_PROFILE");
  const busyAt = Date.now(); must(remaining(3000) > 2500, "BUSY_OBSERVATION_BUDGET");
  await page.waitForTimeout(2500); await settle();
  observed.busy_quiet = { elapsed_ms: Date.now() - busyAt, human_requests: requests.length - busyStart, recent_requests: countRecent(busyStart) };
  must(countRecent(busyStart) === 1 && requests.length - busyStart === 1, "BUSY_TRIGGERED_FALLBACK_OR_RETRY");
  await command("unlock_own_sqlite"); body = await refresh(200); await snapshotVisible(body);
  const detailRequests = requests.slice(journalStart).filter((row) => row.path.startsWith(eventDetailPrefix) && !recentPath(row.path));
  must(same(detailRequests, [{ method: "GET", path: detailPath + "?view=metadata" }]), "EXACT_ONE_METADATA_DETAIL_REQUEST");
  must(streamRequests(journalStart).length === 0 && !requests.slice(journalStart).some((row) => {
    const url = new URL(row.path, "http://fixture.invalid");
    return url.searchParams.get("view") === "content" || url.pathname === "/api/v1/human/export";
  }), "CLOSED_OPERATION_OR_CONTENT_FALLBACK_ATTEMPTED");
  observed.scenario3 = "actual_sqlite_lock_503_withdrawal_restrictive_fact_and_explicit_200_recovery";
  must(pageErrors.length === 0 && external.length === 0, "PAGE_ERROR_OR_EXTERNAL_DESTINATION");
}

async function runOwnedWorkers() {
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer);
  const identity = await readByAction("/api/v1/human/identity", 200, () => button("Ouvrir mon espace").click());
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  must(identity.authentication?.method === "local_credential" && Array.isArray(identity.contexts) &&
    identity.read_profiles?.worker_registry === "human_worker_registry_owned_v1" &&
    identity.read_profiles?.federation_registry === "unavailable", "OWNED_WORKER_IDENTITY");
  const admin = identity.contexts.find((item) => item.tenant_id === "B" && item.mode === "administration");
  const offer = admin?.operations?.find((entry) => entry.operation === "workers_read");
  must(offer?.permission === "granted" && offer.scope === "tenant" && offer.availability === "available", "WORKERS_ADMIN_OFFER");

  stage = "owned_worker_registry";
  const result = await readByAction(workersPath, 200, () => button("Ouvrir administration · B").click());
  must(result.context_id === admin.id && result.authorization_version === admin.authorization_version &&
    result.source === "tenant_worker_broker_registry" && result.consistency === "snapshot_per_page" &&
    result.registry_status === "available" && result.runtime_inventory === "unknown" && result.has_more === false &&
    result.next_cursor === null && Array.isArray(result.items) && result.items.length === 1, "REAL_WORKER_PAGE");
  const item = result.items[0];
  for (const [key, value] of Object.entries(ready.worker)) must(item[key] === value, "RESERVED_WORKER_IDENTITY");
  must(item.state === "reserved" && item.quota_held === true && item.session_provenance === "declared" &&
    item.last_observation === null, "RESERVATION_WITHOUT_RUNTIME_OBSERVATION");
  const receipt = requiredReceipt(result);

  await heading("Workers enregistrés du tenant.").waitFor({ timeout: remaining() });
  const row = page.getByRole("row").filter({ has: page.getByText(item.worker_id, { exact: true }) });
  await row.getByText("Quota réservé", { exact: true }).waitFor({ timeout: remaining() });
  await row.getByText("Observation inconnue", { exact: true }).waitFor({ timeout: remaining() });
  await row.getByRole("button", { name: "Détails", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Métadonnées du worker", exact: true });
  await dialog.getByText(`Reçu durable ${receipt.audit_event_id}`, { exact: false }).waitFor({ timeout: remaining() });
  const detail = JSON.parse(await dialog.locator("pre.code-block").textContent());
  must(detail.worker_id === item.worker_id && detail.attempt_id === item.attempt_id && detail.state === item.state, "REAL_WORKER_DETAIL");
  await noSecret();
  observed.scenario1 = "owned_worker_registry_store14_dashboard_and_receipt";
}

async function runOwnedUsage() {
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer);
  const identity = await readByAction("/api/v1/human/identity", 200, () => button("Ouvrir mon espace").click());
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  must(identity.authentication?.method === "local_credential" && Array.isArray(identity.contexts), "OWNED_USAGE_IDENTITY");
  const admin = identity.contexts.find((item) => item.tenant_id === "B" && item.mode === "administration");
  for (const operation of ["quota_read", "usage_read"]) {
    const offer = admin?.operations?.find((entry) => entry.operation === operation);
    must(offer?.permission === "granted" && offer.scope === "tenant" && offer.availability === "available", "OWNED_QUOTA_AND_USAGE_OFFER");
  }

  // The v2 rights marker opens the real overview first. Wait for it before requesting another read of the same Store.
  await readByAction(summaryPath, 200, () => button("Ouvrir administration · B").click());
  await page.locator('.ov[data-state="ready"]').waitFor({ timeout: remaining() });
  stage = "owned_tenant_usage";
  const [quotaResult, usageResult] = await Promise.all([
    waitForRead(quotaPath, 200),
    waitForRead(usagePagePath, 200),
    button("Quotas et usage").click(),
  ]);
  for (const result of [quotaResult, usageResult]) {
    must(result.version === 1 && result.context_id === admin.id && result.authorization_version === admin.authorization_version, "REAL_ACCOUNTING_SCOPE");
  }
  must(quotaResult.source === "tenant_worker_reservations" && quotaResult.max_timeout_seconds === "7200", "REAL_QUOTA_SOURCE");
  const expected = { held_workers: "1", held_cpu_millis: "2000", held_memory_bytes: "1073741824",
    max_active_workers: "5", max_cpu_millis: "7000", max_memory_bytes: "4294967296" };
  closed(quotaResult.quota, Object.keys(expected), "QUOTA_COUNTS_SHAPE");
  must(Object.entries(expected).every(([key, value]) => quotaResult.quota[key] === value), "DURABLE_QUOTA_COUNTERS_AND_CONFIGURED_LIMITS");
  must(usageResult.source === "tenant_terminal_allocations" && usageResult.method_version === 1 && usageResult.billable === false &&
    usageResult.has_more === false && usageResult.next_cursor === null && Array.isArray(usageResult.items) && usageResult.items.length === 1, "REAL_TERMINAL_ALLOCATIONS_PAGE");
  const allocation = usageResult.items[0];
  const { allocated, unmeasured } = ready.usage;
  for (const [key, value] of Object.entries(allocated)) must(allocation[key] === value, "REAL_ALLOCATED_ATTEMPT");
  must(allocation.duration_ns === "250000" && allocation.cpu_millicore_nanoseconds === "500000000" &&
    allocation.memory_byte_nanoseconds === "268435456000000" && allocation.method_version === 1 && allocation.billable === false &&
    allocation.source?.provenance === "observed" && allocation.source.runtime_kind === "docker", "DURABLE_SUBSECOND_NON_BILLABLE_ALLOCATION");
  const quotaReceipt = requiredReceipt(quotaResult), usageReceipt = requiredReceipt(usageResult);
  must(quotaReceipt.audit_event_id !== usageReceipt.audit_event_id && quotaReceipt.request_id !== usageReceipt.request_id, "DISTINCT_ACCOUNTING_RECEIPTS");

  await heading("Quotas et usage du tenant.").waitFor({ timeout: remaining() });
  const view = page.locator('.ov-usage[data-state="ready"]');
  await view.waitFor({ timeout: remaining() });
  const quota = view.locator('section[data-state="ready"]').filter({ has: heading("Quota du tenant") });
  const usage = view.locator('section[data-state="ready"]').filter({ has: heading("Usage observé") });
  const stats = await quota.locator(".ov-stat").evaluateAll((nodes) => nodes.map((node) =>
    [node.querySelector(".ov-num")?.textContent, node.querySelector(".ov-stat-label")?.textContent]));
  must(same(stats, [["1", "workers réservés sur 5"], ["2\u202f000", "milliCPU réservés sur 7\u202f000"],
    ["1,0 Gio", "mémoire réservée sur 4,0 Gio"]]), "DOM_DURABLE_QUOTA_AND_LIMITS");
  must((await quota.innerText()).includes("Durée maximale d’un worker : 7\u202f200 s."), "DOM_CONFIGURED_WORKER_TIMEOUT");
  for (const [panel, result, receipt] of [[quota, quotaResult, quotaReceipt], [usage, usageResult, usageReceipt]]) {
    const evaluatedAt = await page.evaluate((date) =>
      new Intl.DateTimeFormat("fr-FR", { dateStyle: "medium", timeStyle: "medium" }).format(new Date(date)), result.evaluated_at);
    const provenance = panel.locator(".ov-provenance");
    must(await provenance.locator("time").textContent({ timeout: remaining() }) === evaluatedAt &&
      await provenance.locator("span.mono").textContent({ timeout: remaining() }) === receipt.request_id.slice(0, 8), "DOM_ACCOUNTING_DATES_AND_RECEIPTS");
  }
  const rows = usage.locator("tbody tr");
  must(await usage.locator("caption").textContent({ timeout: remaining() }) === "Allocations terminales observées · méthode 1 · billable=false" &&
    await rows.count() === 1 && await rows.getAttribute("data-attempt-id", { timeout: remaining() }) === allocated.attempt_id &&
    await rows.getByText(allocated.worker_id, { exact: true }).count() === 1 &&
    await usage.getByText(unmeasured.worker_id, { exact: true }).count() === 0 &&
    await usage.getByRole("button", { name: "Allocations suivantes", exact: true }).count() === 0, "DOM_ONLY_OBSERVED_ALLOCATION");
  const duration = rows.getByRole("cell", { name: /^Moins d’une seconde, 250\s000 ns\.$/u });
  const shortDuration = duration.locator('span[aria-hidden="true"]');
  must(await duration.getAttribute("title", { timeout: remaining() }) === "250\u202f000 ns" && await shortDuration.isVisible() &&
    await shortDuration.textContent({ timeout: remaining() }) === "< 1 s" &&
    await rows.getByRole("cell", { name: "< 1", exact: true }).count() === 1 &&
    await rows.getByRole("cell", { name: "< 0,1 Gio", exact: true }).count() === 1, "DOM_SUBSECOND_ALLOCATION_IS_NOT_ZERO");
  await noSecret();
  observed.scenario1 = "owned_tenant_usage_store14_dashboard_and_receipts";
}

const membershipChangePath = () => `${membershipsPath}/${ready.membership.membership_id}`;
const membershipEndpoint = () => new URL(membershipChangePath(), ready.dashboard_url).href;
const postedMembershipRequests = () => requests.filter(({ method }) => method === "POST");

function membershipAdministrator(identity) {
  must(identity.authentication?.method === "local_credential" && identity.principal?.kind === "human_local" &&
    Array.isArray(identity.contexts), "OWNED_MEMBERSHIP_LOCAL_IDENTITY");
  const admins = identity.contexts.filter(({ mode, tenant_id }) => mode === "administration" && tenant_id === ready.tenant_id);
  must(admins.length === 1, "EXACT_MEMBERSHIP_ADMIN_CONTEXT");
  const [admin] = admins;
  must(typeof admin.id === "string" && admin.id.length > 0 && typeof admin.authorization_version === "string" &&
    Array.isArray(admin.capabilities) && ["manage_memberships", "read_memberships"].every((right) => admin.capabilities.includes(right)),
    "MEMBERSHIP_ADMIN_RIGHTS");
  const read = admin.operations?.find(({ operation }) => operation === "memberships_read");
  const manage = admin.operations?.find(({ operation }) => operation === "memberships_manage");
  must(read?.permission === "granted" && read.availability === "available" && manage?.permission === "granted" &&
    manage.availability === "available" && manage.scope === "organization", "MEMBERSHIP_CATALOGUE_OFFER");
  return admin;
}

function grantAdministrator(identity) {
  must(identity.authentication?.method === "local_credential" && identity.principal?.kind === "human_local" &&
    Array.isArray(identity.contexts), "OWNED_GRANT_LOCAL_IDENTITY");
  const admins = identity.contexts.filter(({ mode, tenant_id }) => mode === "administration" && tenant_id === ready.tenant_id);
  must(admins.length === 1, "EXACT_GRANT_ADMIN_CONTEXT");
  const [admin] = admins;
  must(typeof admin.id === "string" && admin.id.length > 0 && typeof admin.authorization_version === "string" &&
    Array.isArray(admin.capabilities) && ["manage_grants", "read_memberships", "read_metadata", "read_content"].every((right) => admin.capabilities.includes(right)),
    "GRANT_ADMIN_RIGHTS");
  const manage = admin.operations?.find(({ operation }) => operation === "grants_manage");
  must(manage?.permission === "granted" && manage.availability === "available" && manage.scope === "organization", "GRANT_CATALOGUE_OFFER");
  return admin;
}

const grantEndpoint = () => new URL(grantsPath, ready.dashboard_url).href;
const postedGrantRequests = () => requests.filter(({ method, path }) => method === "POST" && path === grantsPath);
async function grantPost(request) {
  must(request.method() === "POST" && request.url() === grantEndpoint() && request.frame() === page.mainFrame(), "GRANT_POST_TARGET");
  const bytes = request.postDataBuffer();
  must(bytes && bytes.length <= 4096 && !hasPrivate(bytes), "GRANT_POST_SIZE_OR_PRIVACY");
  const body = JSON.parse(bytes.toString("utf8"));
  closed(body, ["operation_id", "resource", "expected_revision", "shares"], "GRANT_POST_SHAPE");
  closed(body.resource, ["kind", "resource_id"], "GRANT_POST_RESOURCE_SHAPE");
  must(uuid(body.operation_id) && same(body.resource, ready.grant.resource) && body.expected_revision === ready.grant.revision &&
    Array.isArray(body.shares) && body.shares.length === 1, "EXACT_GRANT_CHANGE");
  closed(body.shares[0], ["principal", "permissions"], "GRANT_SHARE_SHAPE");
  const principal = body.shares[0].principal;
  const expected = ready.grant.member_principal;
  closed(principal, ["kind", "installation_id", "account_id"], "GRANT_PRINCIPAL_SHAPE");
  must(principal.kind === expected.kind && principal.installation_id === expected.installation_id &&
    principal.account_id === expected.account_id &&
    same([...body.shares[0].permissions].sort(), ["content", "metadata"]), "EXACT_AUTHORIZED_GRANT_CHANGE");
  return { request, bytes, operationId: body.operation_id, contextId: await request.headerValue("X-AiGuard-Context") };
}

function grantReceipt(body, operationId) {
  closed(body, grantReceiptFields, "GRANT_MUTATION_RECEIPT_SHAPE");
  const positive = (value) => typeof value === "string" && /^[1-9][0-9]{0,19}$/.test(value);
  must(body.version === 1 && uuid(body.operation_id) && body.operation_id === operationId &&
    same(body.resource, ready.grant.resource) && body.grant_revision === "2" &&
    positive(body.authorization_revision) && positive(body.audit_seq), "GRANT_RECEIPT_CORRELATION");
  return body;
}

const sameGrantReceipt = (left, right) => grantReceiptFields.every((key) => same(left[key], right[key]));

async function readGrantReceipt(response, operationId) {
  must(response.url() === grantEndpoint() && response.status() === 200, "GRANT_REAL_BFF_SUCCESS");
  const bytes = await response.body();
  must(bytes.length <= 4096 && !hasPrivate(bytes), "GRANT_RECEIPT_SIZE_OR_PRIVACY");
  return grantReceipt(JSON.parse(bytes.toString("utf8")), operationId);
}

async function loseFirstGrantResponse(route) {
  try {
    const sent = await grantPost(route.request());
    const actual = await route.fetch({ maxRedirects: 0, maxRetries: 0, timeout: remaining(12000) });
    try { return { sent, receipt: await readGrantReceipt(actual, sent.operationId) }; }
    finally { await actual.dispose(); }
  } finally { await route.abort("failed"); }
}

async function membershipPost(request) {
  must(request.method() === "POST" && request.url() === membershipEndpoint() && request.frame() === page.mainFrame(), "MEMBERSHIP_POST_TARGET");
  const bytes = request.postDataBuffer();
  must(bytes && bytes.length <= 4096 && !hasPrivate(bytes), "MEMBERSHIP_POST_SIZE_OR_PRIVACY");
  const body = JSON.parse(bytes.toString("utf8"));
  closed(body, ["operation_id", "expected_revision", "capabilities", "active"], "MEMBERSHIP_POST_SHAPE");
  must(uuid(body.operation_id) && body.expected_revision === ready.membership.revision && body.active === true &&
    same(body.capabilities, ["read_metadata"]), "EXACT_FIXTURE_MEMBERSHIP_CHANGE");
  return { request, bytes, operationId: body.operation_id, contextId: await request.headerValue("X-AiGuard-Context") };
}

function membershipReceipt(body, operationId) {
  closed(body, membershipReceiptFields, "MEMBERSHIP_MUTATION_RECEIPT_SHAPE");
  const positive = (value) => typeof value === "string" && /^[1-9][0-9]{0,19}$/.test(value);
  must(body.version === 1 && uuid(body.operation_id) && body.operation_id === operationId &&
    body.membership_id === ready.membership.membership_id && body.membership_revision === "2" &&
    positive(body.authorization_revision) && positive(body.audit_seq), "MEMBERSHIP_RECEIPT_CORRELATION");
  return body;
}

const sameMembershipReceipt = (left, right) => membershipReceiptFields.every((key) => left[key] === right[key]);

async function readMembershipReceipt(response, operationId) {
  must(response.url() === membershipEndpoint() && response.status() === 200, "MEMBERSHIP_REAL_BFF_SUCCESS");
  const bytes = await response.body();
  must(bytes.length <= 4096 && !hasPrivate(bytes), "MEMBERSHIP_RECEIPT_SIZE_OR_PRIVACY");
  return membershipReceipt(JSON.parse(bytes.toString("utf8")), operationId);
}

async function loseFirstMembershipResponse(route) {
  try {
    const sent = await membershipPost(route.request());
    // The original browser POST reaches the real local BFF once. The browser
    // only gets a network failure after the server's successful receipt.
    const actual = await route.fetch({ maxRedirects: 0, maxRetries: 0, timeout: remaining(12000) });
    try { return { sent, receipt: await readMembershipReceipt(actual, sent.operationId) }; }
    finally { await actual.dispose(); }
  } finally { await route.abort("failed"); }
}

async function runOwnedMembership() {
  const identityPath = "/api/v1/human/identity";
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer);
  const identity = await readByAction(identityPath, 200, () => button("Ouvrir mon espace").click());
  const admin = membershipAdministrator(identity);
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  await readByAction(summaryPath, 200, () => button("Ouvrir administration · B").click());
  await page.locator('.ov[data-state="ready"]').waitFor({ timeout: remaining() });

  stage = "owned_membership_prepare";
  const result = await readByAction(membershipPagePath, 200, () => button("Utilisateurs").click());
  must(result.context_id === admin.id && result.authorization_version === admin.authorization_version &&
    result.source === "tenant_identity_memberships" && Array.isArray(result.items), "MEMBERSHIP_PAGE_SCOPE");
  const target = result.items.find(({ membership_id }) => membership_id === ready.membership.membership_id);
  must(target?.display_name === ready.membership.display_name && target.revision === ready.membership.revision &&
    target.organization_scope === admin.organization_scope && target.own === false && target.active === true &&
    target.account_active === true && same([...target.capabilities].sort(), ["read_content", "read_metadata"]), "SYNTHETIC_MEMBERSHIP_BEFORE_CHANGE");
  const view = page.locator(".ov-memberships");
  const row = view.locator(`tr[data-membership-id="${target.membership_id}"]`);
  await row.getByText(target.display_name, { exact: true }).waitFor({ timeout: remaining() });
  await row.getByRole("button", { name: "Modifier", exact: true }).click({ timeout: remaining() });
  const dialog = page.getByRole("dialog", { name: "Modifier l’appartenance", exact: true });
  const content = dialog.getByRole("checkbox", { name: "Contenus", exact: true });
  must(await content.isChecked({ timeout: remaining() }), "MEMBERSHIP_CONTENT_RIGHT_WAS_PRESENT");
  await content.uncheck({ timeout: remaining() });
  await dialog.getByRole("button", { name: "Vérifier le changement", exact: true }).click({ timeout: remaining() });
  must(postedMembershipRequests().length === 0, "MEMBERSHIP_POST_BEFORE_CONFIRMATION");

  stage = "owned_membership_lose_response";
  const endpoint = membershipEndpoint();
  const failed = page.waitForEvent("requestfailed", {
    predicate: (request) => request.method() === "POST" && request.url() === endpoint, timeout: remaining(),
  });
  const revalidated = waitForRead(identityPath, 200);
  failed.catch(() => {});
  revalidated.catch(() => {});
  await dialog.getByRole("button", { name: "Confirmer le changement", exact: true }).click({ timeout: remaining() });
  const failedRequest = await failed;
  must(firstMembershipPost, "MEMBERSHIP_POST_NOT_INTERCEPTED");
  const first = await firstMembershipPost;
  must(first.sent.request === failedRequest && failedRequest.failure()?.errorText === "net::ERR_FAILED" &&
    first.sent.contextId === admin.id, "MEMBERSHIP_LOSS_AND_INITIAL_CONTEXT");
  observed.membership_responses_lost = 1;

  stage = "owned_membership_revalidate";
  const freshIdentity = await revalidated;
  const fresh = membershipAdministrator(freshIdentity);
  must(same(freshIdentity.principal, identity.principal) && fresh.organization_scope === admin.organization_scope &&
    fresh.id !== admin.id && fresh.authorization_version !== admin.authorization_version, "MEMBERSHIP_RECOVERY_USES_FRESH_CONTEXT");
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  const retry = page.getByRole("button", { name: /^Réessayer la même opération(?: ·.*)?$/u });
  await retry.waitFor({ timeout: remaining() });
  const recovery = page.locator("section.panel").filter({ has: retry });
  await recovery.getByText(/Appartenance active/u).waitFor({ timeout: remaining() });
  await recovery.getByText(/Capacités demandées — Métadonnées/u).waitFor({ timeout: remaining() });
  must(await view.count() === 0 && await page.locator("dialog.dg-dialog").count() === 0, "MEMBERSHIP_VIEW_AND_EDITOR_PURGED");
  const post = { method: "POST", path: membershipChangePath() };
  must(same(postedMembershipRequests(), [post]), "MEMBERSHIP_NO_AUTOMATIC_REPLAY");

  stage = "owned_membership_explicit_replay";
  const replay = page.waitForResponse((response) => response.request().method() === "POST" && response.url() === endpoint, { timeout: remaining() });
  const confirmedIdentity = waitForRead(identityPath, 200);
  replay.catch(() => {});
  confirmedIdentity.catch(() => {});
  await retry.click({ timeout: remaining() });
  const response = await replay;
  const resent = await membershipPost(response.request());
  must(resent.contextId === fresh.id && resent.operationId === first.sent.operationId && resent.bytes.equals(first.sent.bytes), "MEMBERSHIP_EXACT_REPLAY_ON_CURRENT_CONTEXT");
  const confirmed = await readMembershipReceipt(response, resent.operationId);
  must(sameMembershipReceipt(first.receipt, confirmed), "MEMBERSHIP_REPLAY_RETURNS_ORIGINAL_RECEIPT");
  const confirmation = page.getByText(/^Changement confirmé par le contrôle\./u);
  await confirmation.waitFor({ timeout: remaining() });
  const notice = await confirmation.textContent({ timeout: remaining() });
  const receiptDetails = new RegExp(`révision de l’appartenance ${confirmed.membership_revision} · position d’audit ${confirmed.audit_seq}\\.$`, "iu");
  must(receiptDetails.test(notice ?? ""), "MEMBERSHIP_DOM_CONFIRMS_DURABLE_RECEIPT");
  const finalAdmin = membershipAdministrator(await confirmedIdentity);
  must(finalAdmin.authorization_version === fresh.authorization_version && await retry.count() === 0, "MEMBERSHIP_REPLAY_KEEPS_REVISION_AND_CLEARS_OFFER");
  must(same(postedMembershipRequests(), [post, post]), "MEMBERSHIP_EXACTLY_TWO_POSTS");
  await noSecret();
  replayedMembershipReceipt = confirmed;
  observed.scenario1 = "owned_membership_response_loss_recovery";
}

async function runOwnedGrantRecovery() {
  const identityPath = "/api/v1/human/identity";
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(bearer);
  const identity = await readByAction(identityPath, 200, () => button("Ouvrir mon espace").click());
  const admin = grantAdministrator(identity);
  await heading("Choisir un contexte.").waitFor({ timeout: remaining() });
  await readByAction(summaryPath, 200, () => button("Ouvrir administration · B").click());
  await page.locator('.ov[data-state="ready"]').waitFor({ timeout: remaining() });

  stage = "owned_grant_prepare";
  const resourcePage = await readByAction("/api/v1/human/resources", 200, () => button("Ressources").click());
  must(resourcePage.context_id === admin.id && resourcePage.authorization_version === admin.authorization_version &&
    Array.isArray(resourcePage.items), "GRANT_RESOURCE_PAGE_SCOPE");
  const resource = resourcePage.items.find(({ resource: ref }) => same(ref, ready.grant.resource));
  must(resource?.owner?.kind === "human_local" && resource.permissions?.metadata === true &&
    resource.revision === ready.grant.revision, "EXISTING_OWNED_GRANT_RESOURCE");
  const resourceIndex = resourcePage.items.indexOf(resource);
  const edit = page.locator(`[data-focus-key="resource-grants-${resourceIndex}"]`);
  const initialGrantResponse = waitForRead(grantResourcePath(ready.grant.resource), 200);
  initialGrantResponse.catch(() => {});
  await edit.waitFor({ timeout: remaining() });
  await edit.click({ timeout: remaining() });

  stage = "owned_grant_review_existing";
  const dialog = page.getByRole("dialog", { name: "Partages de la ressource", exact: true });
  await dialog.waitFor({ timeout: remaining() });
  const grantRead = await initialGrantResponse;
  must(grantRead.context_id === admin.id && grantRead.authorization_version === admin.authorization_version &&
    same(grantRead.resource, ready.grant.resource) && grantRead.revision === ready.grant.revision &&
    Array.isArray(grantRead.shares) && grantRead.shares.length === 1 &&
    grantRead.shares[0].display_name === ready.grant.member_display_name &&
    same([...grantRead.shares[0].permissions].sort(), ["metadata"]), "INITIAL_GRANT_ALREADY_LINKED");
  const share = dialog.locator(".dg-share").filter({ hasText: ready.grant.member_display_name });
  const content = share.getByRole("checkbox", { name: "Contenu", exact: true });
  must(await content.isChecked({ timeout: remaining() }) === false, "GRANT_CONTENT_NOT_ALREADY_SHARED");
  await content.check({ timeout: remaining() });
  await dialog.getByRole("button", { name: "Vérifier le changement", exact: true }).click({ timeout: remaining() });
  await dialog.getByText(new RegExp(`${ready.grant.member_display_name} : ajout Contenu`, "u")).waitFor({ timeout: remaining() });
  must(postedGrantRequests().length === 0, "GRANT_POST_BEFORE_CONFIRMATION");

  stage = "owned_grant_lose_response";
  const endpoint = grantEndpoint();
  const failed = page.waitForEvent("requestfailed", {
    predicate: (request) => request.method() === "POST" && request.url() === endpoint, timeout: remaining(),
  });
  const revalidated = waitForRead(identityPath, 200);
  failed.catch(() => {}); revalidated.catch(() => {});
  await dialog.getByRole("button", { name: "Confirmer le changement", exact: true }).click({ timeout: remaining() });
  const failedRequest = await failed;
  must(firstGrantPost, "GRANT_POST_NOT_INTERCEPTED");
  const first = await firstGrantPost;
  must(first.sent.request === failedRequest && failedRequest.failure()?.errorText === "net::ERR_FAILED" &&
    first.sent.contextId === admin.id, "GRANT_RESPONSE_LOST_AFTER_INITIAL_CONTEXT_POST");
  observed.grant_responses_lost = 1;

  stage = "owned_grant_revalidate_and_purge";
  const freshIdentity = await revalidated;
  const fresh = grantAdministrator(freshIdentity);
  must(same(freshIdentity.principal, identity.principal) && fresh.id !== admin.id &&
    fresh.authorization_version !== admin.authorization_version, "GRANT_RECOVERY_USES_FRESH_CONTEXT");
  await heading("Reprise du changement de partages").waitFor({ timeout: remaining() });
  const retry = page.getByRole("button", { name: "Réessayer la même opération · tenant B · organisation synthetic-org", exact: true });
  await retry.waitFor({ timeout: remaining() });
  must(await page.locator("dialog.dg-dialog").count() === 0 && await page.locator(".dg-shares").count() === 0 &&
    await page.locator(".human-resource-panel").count() === 0, "GRANT_OLD_DIALOG_AND_RESOURCE_VIEW_PURGED");
  const post = { method: "POST", path: grantsPath };
  must(same(postedGrantRequests(), [post]), "GRANT_NO_AUTOMATIC_REPLAY");

  stage = "owned_grant_explicit_replay";
  const replay = page.waitForResponse((response) => response.request().method() === "POST" && response.url() === endpoint, { timeout: remaining() });
  const confirmedIdentity = waitForRead(identityPath, 200);
  replay.catch(() => {}); confirmedIdentity.catch(() => {});
  await retry.click({ timeout: remaining() });
  const response = await replay;
  const resent = await grantPost(response.request());
  must(resent.contextId === fresh.id && resent.operationId === first.sent.operationId && resent.bytes.equals(first.sent.bytes),
    "GRANT_EXACT_REPLAY_ON_REVALIDATED_CONTEXT");
  const confirmed = await readGrantReceipt(response, resent.operationId);
  must(sameGrantReceipt(first.receipt, confirmed), "GRANT_REPLAY_RETURNS_ORIGINAL_RECEIPT");
  const confirmation = page.getByText(/^Changement confirmé par le contrôle\./u);
  await confirmation.waitFor({ timeout: remaining() });
  const notice = await confirmation.textContent({ timeout: remaining() });
  const receiptDetails = new RegExp(`Révision des partages ${confirmed.grant_revision} · position d’audit ${confirmed.audit_seq}\\.$`, "iu");
  must(receiptDetails.test(notice ?? ""), "GRANT_DOM_CONFIRMS_DURABLE_RECEIPT");
  const finalAdmin = grantAdministrator(await confirmedIdentity);
  must(finalAdmin.authorization_version === fresh.authorization_version && await retry.count() === 0, "GRANT_REPLAY_KEEPS_CURRENT_REVISION_AND_CLEARS_OFFER");
  must(same(postedGrantRequests(), [post, post]), "GRANT_EXACTLY_TWO_POSTS");
  await noSecret();
  replayedGrantReceipt = confirmed;
  observed.scenario1 = "owned_grant_response_loss_recovery";
}

try {
  must(fixtureMode === "run-v1" || egressMode || workersMode || usageMode || membershipMode || grantMode, "EXPLICIT_FIXTURE_GUARD_REQUIRED");
  const admittedLinuxArch = process.arch === "arm64" || (!egressMode && process.arch === "x64");
  const admittedLinux = process.platform === "linux" && admittedLinuxArch && process.getuid?.() === 1000 && process.getgid?.() === 1000;
  const admittedMac = egressMode && process.platform === "darwin" && process.arch === "arm64" && process.geteuid?.() > 0;
  must(admittedLinux || admittedMac, "ADMITTED_OWNED_BROWSER_PLATFORM_REQUIRED");
  libtest = process.env.AI_GUARD_OWNED_BROWSER_LIBTEST; dist = process.env.AI_GUARD_OWNED_BROWSER_DIST;
  control = process.env.AI_GUARD_OWNED_BROWSER_CONTROL; artifacts = process.env.AI_GUARD_OWNED_BROWSER_ARTIFACTS;
  seconds = Number(process.env.AI_GUARD_OWNED_BROWSER_SECONDS ?? "180");
  must(Number.isInteger(seconds) && seconds >= 180 && seconds <= 600, "DRIVER_REQUIRES_180_TO_600_SECONDS");
  const temporary = await realpath(process.env.TMPDIR ?? "/tmp");
  must(isAbsolute(libtest ?? "") && libtest === resolve(libtest), "ADMITTED_LIBTEST_PATH_REQUIRED");
  const executable = await lstat(libtest); must(executable.isFile() && !executable.isSymbolicLink() && (executable.mode & 0o111) !== 0, "LIBTEST_REGULAR_EXECUTABLE");
  must(isAbsolute(dist ?? "") && dist === resolve(dist) && await realpath(dist) === dist, "ADMITTED_DIST_REQUIRED");
  const index = await lstat(join(dist, "index.html")); must(index.isFile() && !index.isSymbolicLink(), "REAL_DIST_INDEX_REQUIRED");
  await normalizedDirectory(control, true); await normalizedDirectory(artifacts, true);
  must(below(temporary, control) && below(temporary, artifacts) && control !== artifacts && !below(control, artifacts) && !below(artifacts, control), "FRESH_CONTROL_AND_ARTIFACTS_UNDER_TMPDIR");
  must(dist !== control && !below(dist, control) && !below(control, dist) && dist !== artifacts && !below(dist, artifacts) && !below(artifacts, dist), "PRIVATE_DIRECTORIES_MUST_NOT_BE_SERVED");
  artifactsAdmitted = true;
  const require = createRequire(import.meta.url); let playwright;
  try { playwright = require("playwright"); }
  catch { const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES; must(isAbsolute(modules ?? ""), "PLAYWRIGHT_NOT_RESOLVABLE"); playwright = require(join(modules, "playwright")); }
  observed.libtest_sha256 = await shaFile(libtest); observed.dist_index_sha256 = await shaFile(join(dist, "index.html")); observed.node = process.version;
  stage = "fixture_start"; const started = Date.now(); interactionDeadline = started + seconds * 1000 - 25000;
  child = spawn(libtest, ["--ignored", "--exact", FQN, "--nocapture", "--test-threads=1"], { stdio: ["ignore", "pipe", "pipe"],
    env: { PATH: "/usr/local/bin:/usr/bin:/bin", HOME: process.env.HOME ?? temporary, TMPDIR: temporary, RUST_BACKTRACE: "0",
      AI_GUARD_OWNED_BROWSER_FIXTURE: fixtureMode, AI_GUARD_OWNED_BROWSER_DIST: dist, AI_GUARD_OWNED_BROWSER_CONTROL: control, AI_GUARD_OWNED_BROWSER_SECONDS: String(seconds) } });
  child.once("spawn", () => { fixtureStarted = true; });
  childDone = new Promise((resolveDone) => {
    child.once("error", () => { fixtureExit = { code: null, signal: null, spawn_error: true }; resolveDone(fixtureExit); });
    child.once("close", (code, signal) => { fixtureExit ??= { code, signal }; resolveDone(fixtureExit); });
  });
  for (const name of ["stdout", "stderr"]) child[name].on("data", (bytes) => {
    output[name].bytes += bytes.length; output[name].hash.update(bytes);
    if (output[name].bytes > 1048576) { cleanupFailure ??= "FIXTURE_OUTPUT_LIMIT"; if (fixtureExit === null) child.kill("SIGTERM"); }
  });
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => {
    interrupted = true; interruptionSignal ??= signal; process.exitCode = 1;
    if (terminalResult) {
      try { publishTerminalResult(); } catch { cleanupFailure ??= "INTERRUPTED_RESULT_PERSISTENCE_FAILED"; }
      console.log("browser_owned_snapshot FAIL INTERRUPTED");
    }
    void page?.close().catch(() => {});
  });
  const readyFile = await waitJson(join(control, "READY.json"), 8192, Date.now() + remaining(90000));
  ready = validateReady(readyFile.value);
  observed.ready_sha256 = readyFile.sha256;
  observed.ready_public = { ...ready };
  if (egressMode) {
    privateValues.push(ready.owner.bearer, ready.reader.bearer);
    observed.ready_public.owner = { display_name: ready.owner.display_name };
    observed.ready_public.reader = { display_name: ready.reader.display_name };
  } else {
    bearer = ready.bearer_b;
    privateValues.push(bearer);
    delete observed.ready_public.bearer_b;
    if (grantMode) {
      observed.ready_public.grant = { ...ready.grant };
      delete observed.ready_public.grant.member_principal;
    }
  }
  stage = "browser_start";
  const restrictedBrowser = egressMode || workersMode || usageMode || membershipMode || grantMode;
  browser = await playwright.chromium.launch({ headless: true, chromiumSandbox: true, timeout: remaining(60000),
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
    ...(restrictedBrowser ? { args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] } : {}),
    env: { PATH: "/usr/local/bin:/usr/bin:/bin", HOME: process.env.HOME ?? temporary, TMPDIR: temporary } });
  observed.browser_version = browser.version();
  context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, locale: "fr-FR", timezoneId: "UTC", colorScheme: "light",
    ...(restrictedBrowser ? { serviceWorkers: "block" } : {}) });
  const dashboardOrigin = new URL(ready.dashboard_url).origin;
  await context.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (["about:", "data:", "blob:"].includes(url.protocol)) return route.continue();
    if (url.origin !== dashboardOrigin) {
      external.push({ protocol: url.protocol, hostname: url.hostname });
      return route.abort("blockedbyclient");
    }
    if (membershipMode && !firstMembershipPost && request.method() === "POST" && request.url() === membershipEndpoint()) {
      firstMembershipPost = loseFirstMembershipResponse(route);
      await firstMembershipPost.catch(() => {}); // The scenario checks the result after the request fails in the browser.
      return;
    }
    if (grantMode && !firstGrantPost && request.method() === "POST" && request.url() === grantEndpoint()) {
      firstGrantPost = loseFirstGrantResponse(route);
      await firstGrantPost.catch(() => {});
      return;
    }
    return route.continue();
  });
  page = await context.newPage(); page.setDefaultTimeout(remaining()); page.setDefaultNavigationTimeout(remaining());
  page.on("pageerror", (error) => pageErrors.push(redact(error.message)));
  page.on("request", (request) => { const url = new URL(request.url()); if (url.pathname.startsWith("/api/v1/human/")) requests.push({ method: request.method(), path: url.pathname + url.search }); });
  page.on("response", (response) => { const capture = captureResponse(response); capture.catch(() => {}); pendingWires.push(capture); });
  stage = grantMode ? "login_and_grants" : membershipMode ? "login_and_membership" : usageMode ? "login_and_usage" : workersMode ? "login_and_workers" : "login_and_snapshot";
  await page.goto(ready.dashboard_url);
  if (egressMode) await runEgressJournal();
  else if (workersMode) await runOwnedWorkers();
  else if (usageMode) await runOwnedUsage();
  else if (membershipMode) await runOwnedMembership();
  else if (grantMode) await runOwnedGrantRecovery();
  else await runOwnedSnapshot();
} catch (error) { failure = { stage, code: redact(error.message), name: error.name }; }
finally {
  // No global process discovery, container command or cleanup by name. Control
  // files are retained for Root; consumed commands and evidence are never erased.
  if (egressMode && failure && authenticatedDashboard) {
    const capture = await saveBrowserFailure(page, { directory: artifacts, secrets: privateValues });
    if (capture) captures.push(capture);
  }
  try { if (browser) await bounded(browser.close({ reason: "owned snapshot test finished" }), 15000, "BROWSER_CLOSE_DEADLINE"); }
  catch { cleanupFailure = "BROWSER_CLOSE_NOT_ESTABLISHED"; }
  if (fixtureStarted && fixtureExit === null) {
    try { await command("stop", true); } catch { cleanupFailure ??= "STOP_ACK_NOT_ESTABLISHED"; }
  }
  if (fixtureStarted) {
    try {
      const ending = await waitJson(join(control, "CLOSED.json"), 4096, Date.now() + 20000, true); closedFixture = ending.value;
      const extraField = egressMode ? "policy_receipts_verified" : "sqlite_lock_released";
      closed(closedFixture, ["version", "completed", "commands_processed", extraField, "servers_and_owner_drained", "receipts_exported", "stop_reason",
        ...(membershipMode ? ["membership_change"] : []), ...(grantMode ? ["grant_change"] : [])], "CLOSED_SHAPE");
      must(closedFixture.version === 1 && closedFixture.completed === true && closedFixture.commands_processed === sequence && closedFixture[extraField] === true && closedFixture.servers_and_owner_drained === true && closedFixture.receipts_exported === true && closedFixture.stop_reason === "requested", "FIXTURE_CLOSURE_NOT_ESTABLISHED");
      const exported = await waitJson(join(control, "RECEIPTS.json"), 524288, Date.now() + 10000, true); receipts = exported.value;
      closed(receipts, ["version", "tenant_id", "complete", "limit", "records"], "RECEIPTS_SHAPE");
      must(receipts.version === 1 && receipts.tenant_id === (egressMode ? "A" : "B") && receipts.complete === true && receipts.limit === 1000 && Array.isArray(receipts.records) && receipts.records.length <= 1000, "RECEIPTS_INCOMPLETE");
      const byId = new Map();
      const receiptActions = ["identity", "resources", "events", "event", "summary", "quota", "usage", workerReceiptKind];
      if (membershipMode) receiptActions.push("memberships");
      if (grantMode) receiptActions.push("memberships", "grants");
      for (const record of receipts.records) { closed(record, ["audit_event_id", "request_id", "action", "received_at"], "RECEIPT_RECORD"); must(receiptActions.includes(record.action) && typeof record.audit_event_id === "string" && typeof record.request_id === "string" && typeof record.received_at === "string", "RECEIPT_RECORD_TYPES"); byId.set(record.audit_event_id, record); }
      must(byId.size === receipts.records.length, "DUPLICATE_DURABLE_RECEIPT_ID");
      await finishWireCaptures(); let compared = 0;
      // Identity has no page receipt. Every 200 on a page or detail endpoint
      // requires one; malformed/missing receipts are failures, never skipped.
      const successfulPages = wires.filter((wire) => wire.method === "GET" && wire.status === 200 && pageAction(wire.path) !== null);
      for (const wire of successfulPages) {
        const receipt = requiredReceipt(wire.body), row = byId.get(receipt.audit_event_id);
        const action = pageAction(wire.path);
        must(row && row.request_id === receipt.request_id && row.received_at === receipt.received_at && row.action === action, "WIRE_RECEIPT_NOT_IN_REAL_SQLITE_EXPORT"); compared++;
      }
      observed.successful_page_responses = successfulPages.length; observed.compared_receipts = compared; observed.receipts_sha256 = exported.sha256; observed.closed_sha256 = ending.sha256;
      await save("RECEIPTS.json", receipts); await save("CLOSED.json", closedFixture);
      if (workersMode && !failure) {
        must(same(requests, [
          { method: "GET", path: "/api/v1/human/identity" },
          { method: "GET", path: `${workersPath}?limit=25` },
        ]), "EXACT_ONE_WORKER_GET_WITHOUT_STREAM_OR_FEDERATION");
        must(compared === 1 && successfulPages[0].path === `${workersPath}?limit=25` &&
          receipts.records.filter((record) => record.action === workerReceiptKind).length === 1, "EXACT_ONE_DURABLE_WORKER_RECEIPT");
      }
      if (usageMode && !failure) {
        must(same(requests, [
          { method: "GET", path: "/api/v1/human/identity" },
          { method: "GET", path: `${summaryPath}?window=24h` },
          { method: "GET", path: quotaPath },
          { method: "GET", path: usagePagePath },
        ]), "EXACT_ACCOUNTING_GETS_IN_ORDER");
        const accountingActions = receipts.records.map(({ action }) => action).filter((action) => action === "quota" || action === "usage");
        must(compared === 2 && same(successfulPages.map(({ path }) => path).sort(), [quotaPath, usagePagePath]) &&
          same(accountingActions, ["quota", "usage"]), "EXACT_DURABLE_ACCOUNTING_RECEIPTS_IN_ORDER");
      }
      if (membershipMode && !failure) {
        const mutation = closedFixture.membership_change;
        closed(mutation, ["mutation_count", "receipt"], "CLOSED_MEMBERSHIP_MUTATION");
        must(mutation.mutation_count === 1 && replayedMembershipReceipt, "EXACTLY_ONE_DURABLE_MEMBERSHIP_MUTATION");
        const durable = membershipReceipt(mutation.receipt, replayedMembershipReceipt.operation_id);
        must(sameMembershipReceipt(durable, replayedMembershipReceipt), "MEMBERSHIP_RECEIPT_MATCHES_SQLITE");
        const post = { method: "POST", path: membershipChangePath() };
        const received = wires.filter(({ method }) => method === "POST");
        must(same(postedMembershipRequests(), [post, post]) && received.length === 1 && received[0].path === post.path &&
          received[0].status === 200 && sameMembershipReceipt(received[0].body, durable), "MEMBERSHIP_ONLY_REPLAY_RECEIPT_DELIVERED");
        must(compared === 1 && successfulPages[0].path === membershipPagePath &&
          receipts.records.filter(({ action }) => action === "memberships").length === 1, "EXACT_DURABLE_MEMBERSHIP_PAGE_RECEIPT");
        observed.membership_posts = 2;
        observed.membership_browser_receipts = 1;
        observed.membership_sqlite_mutations = 1;
      }
      if (grantMode && !failure) {
        const mutation = closedFixture.grant_change;
        closed(mutation, ["mutation_count", "receipt"], "CLOSED_GRANT_MUTATION");
        must(mutation.mutation_count === 1 && replayedGrantReceipt, "EXACTLY_ONE_DURABLE_GRANT_MUTATION");
        const durable = grantReceipt(mutation.receipt, replayedGrantReceipt.operation_id);
        must(sameGrantReceipt(durable, replayedGrantReceipt), "GRANT_RECEIPT_MATCHES_SQLITE");
        const post = { method: "POST", path: grantsPath };
        const received = wires.filter(({ method }) => method === "POST");
        must(same(postedGrantRequests(), [post, post]) && received.length === 1 && received[0].path === post.path &&
          received[0].status === 200 && sameGrantReceipt(received[0].body, durable), "GRANT_ONLY_REPLAY_RECEIPT_DELIVERED");
        const grantReads = successfulPages.filter(({ path }) => new URL(path, "http://fixture.invalid").pathname === grantsPath);
        must(grantReads.length === 1 && compared >= grantReads.length &&
          receipts.records.filter(({ action }) => action === "grants").length === 1, "EXACT_DURABLE_GRANT_READ_RECEIPT");
        observed.grant_posts = 2;
        observed.grant_browser_receipts = 1;
        observed.grant_sqlite_mutations = 1;
        observed.grant_revision = Number(durable.grant_revision);
      }
      const exit = await bounded(childDone, 15000, "LIBTEST_EXIT_DEADLINE"); must(exit?.code === 0 && exit.signal === null, "LIBTEST_EXIT_NOT_ZERO");
    } catch (error) { cleanupFailure ??= redact(error.message); }
    if (fixtureExit === null) {
      child.kill("SIGTERM"); await bounded(childDone, 5000, "FIXTURE_TERM_DEADLINE").catch(() => {});
      if (fixtureExit === null) { child.kill("SIGKILL"); await bounded(childDone, 5000, "FIXTURE_KILL_DEADLINE").catch(() => {}); }
      cleanupFailure ??= "FIXTURE_REQUIRED_FORCED_TERMINATION";
      if (fixtureExit === null) { child.unref(); child.stdout.destroy(); child.stderr.destroy(); }
    }
  }
  if (artifactsAdmitted) {
    try { await finishWireCaptures(); } catch (error) { cleanupFailure ??= redact(error.message); }
    if (pageErrors.length || external.length) cleanupFailure ??= "PAGE_ERROR_OR_EXTERNAL_DESTINATION";
    const childOutput = Object.fromEntries(Object.entries(output).map(([name, value]) => [name, { bytes: value.bytes, sha256: value.hash.copy().digest("hex"), retained_payload: false, stream_closed: !fixtureStarted || fixtureExit !== null }]));
    try {
      await save("WIRE_RESULTS.json", { version: 1, requests, responses: wires });
      terminalResult = { version: 1, failure, cleanup_failure: cleanupFailure,
        observed, captures, commands, page_errors: pageErrors, external_destinations: external, fixture_exit: fixtureExit, child_output: childOutput,
        limits: "Real owned local software/browser scenarios only. No runtime/provider/SSO/cloud/100-worker/production qualification. Root OS/network/source/admission and terminal review remain required." };
      publishTerminalResult();
    } catch { cleanupFailure ??= "PRIVATE_ARTIFACT_PERSISTENCE_FAILED"; }
  }
}
// No source path, bearer, response body or child diagnostic is printed.
console.log(interrupted ? "browser_owned_snapshot FAIL INTERRUPTED" : !fixtureStarted ? "browser_owned_snapshot BLOCKED NOT_RUN"
  : failure || cleanupFailure ? "browser_owned_snapshot FAIL; private result retained"
  : grantMode ? "browser_owned_snapshot PASS scenario=grant_recovery"
  : membershipMode ? "browser_owned_snapshot PASS scenario=membership_recovery"
  : usageMode ? "browser_owned_snapshot PASS scenario=usage"
  : workersMode ? "browser_owned_snapshot PASS scenario=worker_registry"
  : egressMode ? "browser_owned_snapshot PASS scenario=egress_journal" : "browser_owned_snapshot PASS scenarios=3");
process.exitCode = interrupted ? 1 : !fixtureStarted ? 2 : failure || cleanupFailure ? 1 : 0;
