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
const FQN = egressMode
  ? "api::human_sources::tests::owned_browser_fixture::egress::serve_owned_egress_browser_fixture"
  : "api::human_sources::tests::owned_browser_fixture::serve_owned_browser_fixture";
const PRIVATE_CONTENT = "owned-browser-private-content-must-not-appear-in-metadata";
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
  closed(value, ["version", "dashboard_url", "bearer_b", "tenant_id", "resource", "event_ids", "budget_seconds", "commands", "acks", "command_limit"], "READY_CLOSED");
  must(value.tenant_id === "B", "READY_PROFILE");
  must(typeof value.bearer_b === "string" && /^aigh1_[0-9a-f]{64}$/.test(value.bearer_b), "READY_FIXTURE_BEARER");
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
  const bytes = await response.body(); must(bytes.length <= 262144, "WIRE_RESPONSE_SIZE");
  must(!hasPrivate(bytes), "PRIVATE_MATERIAL_IN_WIRE");
  let body; try { body = JSON.parse(bytes.toString("utf8")); } catch { throw new Error("WIRE_RESPONSE_NOT_JSON"); }
  wires.push({ method: response.request().method(), path: new URL(response.url()).pathname + new URL(response.url()).search,
    status: response.status(), bytes: bytes.length, sha256: digest(bytes), body });
  if (response.status() === 200 && pageAction(path) !== null) requiredReceipt(body);
}
async function readByAction(pathname, status, action) {
  const response = page.waitForResponse((item) => new URL(item.url()).pathname === pathname && item.request().method() === "GET", { timeout: remaining() });
  response.catch(() => {}); // An action failure must not leave an unhandled waiter.
  await action(); const actual = await response; must(actual.status() === status, "UNEXPECTED_REAL_HTTP_STATUS");
  const bytes = await actual.body(); must(bytes.length <= 262144 && !hasPrivate(bytes), "ACTION_WIRE_PRIVACY_OR_SIZE");
  const body = JSON.parse(bytes.toString("utf8"));
  if (status === 200 && pageAction(pathname) !== null) requiredReceipt(body);
  return body;
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

try {
  must(fixtureMode === "run-v1" || egressMode, "EXPLICIT_FIXTURE_GUARD_REQUIRED");
  const admittedLinux = process.platform === "linux" && process.arch === "arm64" && process.getuid?.() === 1000 && process.getgid?.() === 1000;
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
  }
  stage = "browser_start";
  browser = await playwright.chromium.launch({ headless: true, chromiumSandbox: true, timeout: remaining(60000),
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
    ...(egressMode ? { args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] } : {}),
    env: { PATH: "/usr/local/bin:/usr/bin:/bin", HOME: process.env.HOME ?? temporary, TMPDIR: temporary } });
  observed.browser_version = browser.version();
  context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, locale: "fr-FR", timezoneId: "UTC", colorScheme: "light",
    ...(egressMode ? { serviceWorkers: "block" } : {}) });
  await context.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (["about:", "data:", "blob:"].includes(url.protocol) || url.origin === new URL(ready.dashboard_url).origin) return route.continue();
    external.push({ protocol: url.protocol, hostname: url.hostname }); await route.abort("blockedbyclient");
  });
  page = await context.newPage(); page.setDefaultTimeout(remaining()); page.setDefaultNavigationTimeout(remaining());
  page.on("pageerror", (error) => pageErrors.push(redact(error.message)));
  page.on("request", (request) => { const url = new URL(request.url()); if (url.pathname.startsWith("/api/v1/human/")) requests.push({ method: request.method(), path: url.pathname + url.search }); });
  page.on("response", (response) => { const capture = captureResponse(response); capture.catch(() => {}); pendingWires.push(capture); });
  stage = "login_and_snapshot";
  await page.goto(ready.dashboard_url);
  if (egressMode) await runEgressJournal();
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
      closed(closedFixture, ["version", "completed", "commands_processed", extraField, "servers_and_owner_drained", "receipts_exported", "stop_reason"], "CLOSED_SHAPE");
      must(closedFixture.version === 1 && closedFixture.completed === true && closedFixture.commands_processed === sequence && closedFixture[extraField] === true && closedFixture.servers_and_owner_drained === true && closedFixture.receipts_exported === true && closedFixture.stop_reason === "requested", "FIXTURE_CLOSURE_NOT_ESTABLISHED");
      const exported = await waitJson(join(control, "RECEIPTS.json"), 524288, Date.now() + 10000, true); receipts = exported.value;
      closed(receipts, ["version", "tenant_id", "complete", "limit", "records"], "RECEIPTS_SHAPE");
      must(receipts.version === 1 && receipts.tenant_id === (egressMode ? "A" : "B") && receipts.complete === true && receipts.limit === 1000 && Array.isArray(receipts.records) && receipts.records.length <= 1000, "RECEIPTS_INCOMPLETE");
      const byId = new Map();
      for (const record of receipts.records) { closed(record, ["audit_event_id", "request_id", "action", "received_at"], "RECEIPT_RECORD"); must(["identity", "resources", "events", "event"].includes(record.action) && typeof record.audit_event_id === "string" && typeof record.request_id === "string" && typeof record.received_at === "string", "RECEIPT_RECORD_TYPES"); byId.set(record.audit_event_id, record); }
      must(byId.size === receipts.records.length, "DUPLICATE_DURABLE_RECEIPT_ID");
      await finishWireCaptures(); let compared = 0;
      // Identity has no page receipt. Every 200 on a page or detail endpoint
      // requires one; malformed/missing receipts are failures, never skipped.
      const successfulPages = wires.filter((wire) => wire.status === 200 && pageAction(wire.path) !== null);
      for (const wire of successfulPages) {
        const receipt = requiredReceipt(wire.body), row = byId.get(receipt.audit_event_id);
        const action = pageAction(wire.path);
        must(row && row.request_id === receipt.request_id && row.received_at === receipt.received_at && row.action === action, "WIRE_RECEIPT_NOT_IN_REAL_SQLITE_EXPORT"); compared++;
      }
      observed.successful_page_responses = successfulPages.length; observed.compared_receipts = compared; observed.receipts_sha256 = exported.sha256; observed.closed_sha256 = ending.sha256;
      await save("RECEIPTS.json", receipts); await save("CLOSED.json", closedFixture);
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
  : egressMode ? "browser_owned_snapshot PASS scenario=egress_journal" : "browser_owned_snapshot PASS scenarios=3");
process.exitCode = interrupted ? 1 : !fixtureStarted ? 2 : failure || cleanupFailure ? 1 : 0;
