// Vue 360° and administration destinations in Chromium against the real Rust gateway and dashboard routers.
// Same harness as browser_supervision.mjs: the synthetic seed of examples/supervision_stack.rs, real control
// mutations on private test SQLite stores (human rights marker installed), no runtime and no worker execution.
// Nothing here is imported by the application.
//
//   AI_GUARD_ADMIN360_SCENARIO   all (default) | profiles | keyboard | states | broker | rights | revoked
//                                "all" runs them in this order; broker, rights and revoked mutate the seed.
//   AI_GUARD_SUPERVISION_STACK   path of the built supervision_stack example
//   AI_GUARD_SUPERVISION_FRONT_DIR built frontend directory (default: front/dist)
//   AI_GUARD_ADMIN360_ARTIFACTS  screenshot directory (default: front/tests/artifacts/admin360, ignored by Git)
//
// Prerequisite: the playwright package (1.64.x) and its Chromium, as for browser_supervision.mjs. Without them the
// script exits with code 2 and BLOCKED, and the scenario is NOT_RUN. Run one Chromium recipe at a time (shared
// browser slot of the environment); the waits below are generous because the machine may be saturated.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmod, mkdir, mkdtemp, access } from "node:fs/promises";
import { createRequire } from "node:module";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { saveBrowserFailure } from "./browser_failure.mjs";
import { installBrowserGuard } from "./browser_live.mjs";

const scenario = process.env.AI_GUARD_ADMIN360_SCENARIO || "all";
const phases = ["profiles", "keyboard", "states", "broker", "rights", "revoked"];
const blocked = (reason) => { console.error(`BLOCKED: ${reason}. Scenario ${scenario} is NOT_RUN.`); process.exit(2); };
if (scenario !== "all" && !phases.includes(scenario)) blocked(`unknown scenario ${scenario}`);
if (process.env.DEBUG || process.env.PWDEBUG || process.env.NODE_DEBUG) blocked("disable debug logs for the authenticated browser recipe");
const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require("playwright")); }
catch {
  const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
  try { if (!modules) throw new Error("no module directory"); ({ chromium } = require(join(modules, "playwright"))); }
  catch { blocked("the playwright package (1.64.x) and its Chromium are not installed; make them resolvable through NODE_PATH or CODEX_PRIMARY_RUNTIME_NODE_MODULES"); }
}
const root = fileURLToPath(new URL("..", import.meta.url));
const binary = process.env.AI_GUARD_SUPERVISION_STACK || "/workspace/target/debug/examples/supervision_stack";
const front = process.env.AI_GUARD_SUPERVISION_FRONT_DIR || join(root, "dist");
for (const path of [binary, join(front, "index.html")])
  await access(path).catch(() => blocked(`${path} is missing: build the supervision_stack example and the frontend first`));

// Small registry: every section stays within its row budget, so a ledger row is ready, not granted, or a named gap.
const workers = 24; const events = 60;
const WAIT = 60000;
const stateDir = await mkdtemp(join(tmpdir(), "ai-guard-admin360-state-")); await chmod(stateDir, 0o700);
const artifacts = process.env.AI_GUARD_ADMIN360_ARTIFACTS || join(root, "tests", "artifacts", "admin360");
await mkdir(artifacts, { recursive: true, mode: 0o700 }); await chmod(artifacts, 0o700);

// The stack prints its credentials once on stdout; this parent keeps them in memory only.
const stack = spawn(binary, ["--front-dir", front, "--state-dir", stateDir, "--workers", String(workers), "--events", String(events)],
  { stdio: ["pipe", "pipe", "inherit"] });
const lines = createInterface({ input: stack.stdout })[Symbol.asyncIterator]();
const nextLine = async (milliseconds) => {
  let timer;
  const expired = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("the supervision stack did not answer in time")), milliseconds); });
  try { const { value, done } = await Promise.race([lines.next(), expired]); if (done) throw new Error("the supervision stack exited"); return JSON.parse(value); }
  finally { clearTimeout(timer); }
};
const ready = await nextLine(900000).catch((error) => { stack.kill(); blocked(error.message); });
assert.equal(ready.ready, true);
const command = async (value) => {
  stack.stdin.write(`${JSON.stringify(value)}\n`); const reply = await nextLine(120000);
  assert.equal(reply.ok, true, `stack command ${value.command}: ${reply.error ?? ""}`); return reply;
};
const { accounts, ids } = ready; const secrets = Object.values(accounts);
const a = ids.tenant_a; const b = ids.tenant_b;
const brokersA = Object.values(a.brokers);
const idsA = [...a.workers, ...brokersA, ...a.granted_sessions, a.session_without_grant, a.unregistered_authority];
const idsB = [...b.workers, ...b.brokers, b.session];
const full = ["read_metadata", "read_content", "export", "read_tenant_metadata", "read_broker_registry", "read_worker_registry"];
const reduced = ["read_metadata", "read_tenant_metadata", "read_broker_registry"];

const browser = await chromium.launch({ headless: true, chromiumSandbox: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
  args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] })
  .catch(() => { stack.kill("SIGKILL"); blocked("Chromium failed to launch with its sandbox"); });
// A browser that cannot open its page is closed at once, with the stack: no process outlives this script.
const opened = await (async () => { const context = await browser.newContext({ viewport: { width: 1440, height: 1050 }, serviceWorkers: "block" });
  return { context, page: await context.newPage() }; })()
  .catch(async (error) => { await browser.close().catch(() => {}); stack.kill("SIGKILL"); blocked(`Chromium could not open a page (${String(error?.message ?? error).split("\n")[0]})`); });
const { context, page } = opened; page.setDefaultTimeout(WAIT);
const dashboard = new URL(ready.dashboard);
assert.ok(dashboard.protocol === "http:" && dashboard.hostname === "127.0.0.1" && dashboard.port && dashboard.origin === ready.dashboard);
// One origin, GET only, no credential in a URL: the guard records any other request and refuses it.
const guard = await installBrowserGuard(context, [dashboard.origin], secrets);
await guard.preparePage(page);
const errors = []; page.on("pageerror", (error) => errors.push(error.message));
const requests = []; const contexts = [];
page.on("request", (request) => {
  const url = new URL(request.url()); if (!url.pathname.startsWith("/api/")) return;
  requests.push(`${url.pathname}${url.search}`); contexts.push(request.headers()["x-aiguard-context"] ?? null);
});
const identityReads = (since = 0) => requests.slice(since).filter((path) => path === "/api/v1/human/identity").length;
const summaryReads = (since = 0) => requests.slice(since).filter((path) => path.startsWith("/api/v1/human/summary")).length;
// Independent observations: all are reported together at the end of the run.
const softFailures = [];
const soft = (condition, message) => { if (!condition) softFailures.push(message); };

const button = (name) => page.getByRole("button", { name, exact: true });
const heading = (name) => page.getByRole("heading", { name, exact: true });
const settle = () => page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
const bodyText = () => page.locator("body").innerText();
const tabs = () => page.locator(".human-nav .human-tab").allInnerTexts();
const isReply = (answer, path) => answer.request().method() === "GET" && new URL(answer.url()).pathname === path;
/** Signs in, waits for the contexts, and returns the identity the control served. */
const login = async (account) => {
  await page.goto(ready.dashboard);
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(accounts[account]);
  const [answer] = await Promise.all([page.waitForResponse((reply) => isReply(reply, "/api/v1/human/identity")), button("Ouvrir mon espace").click()]);
  assert.equal(answer.status(), 200, `${account}: identity read`);
  await heading("Choisir un contexte.").waitFor();
  return answer.json();
};
const logout = async () => { await button("Se déconnecter").click(); await page.getByLabel("Credential personnel local").waitFor(); };
const noSecret = async (label) => { const text = await bodyText(); for (const secret of secrets) assert.ok(!text.includes(secret), `${label}: aucun credential dans la page`); };
const noneOf = async (label, forbidden) => { const text = await bodyText(); for (const id of forbidden) assert.ok(!text.includes(id), `${label}: aucun identifiant d’un autre tenant (${id.slice(0, 8)}…)`); };

// Vue 360° ledger.
const ledger = () => page.locator(".ov-ledger");
const row = (key) => page.locator(`.ov-ledger tr[data-ledger-row="${key}"]`);
const LEDGER_ROWS = ["workers", "brokers", "resources", "decisions", "quota", "usage"];
const UNAVAILABLE = ["Indisponible pour le profil de sources adopté : rien n’est lu ni déduit.",
  "Indisponible : la base attend une migration explicite de l’opérateur. L’état reste inconnu.",
  "Indisponible : le registre dépasse le budget de lecture de cette synthèse. Ouvrez la vue exacte."];
const summaryReady = async () => {
  await page.locator('.ov[data-state="ready"]').waitFor({ timeout: WAIT });
  await page.locator(".ov-provenance").waitFor();
};
/** Each figure of a ledger row, by its label, from the exact decimal text the view rendered. */
const stats = (key) => row(key).locator(".ov-stat").evaluateAll((nodes) => nodes.map((node) => ({
  label: node.querySelector(".ov-stat-label")?.textContent ?? "", value: Number((node.querySelector(".ov-num")?.textContent ?? "").replace(/\D/g, "")) })));
const total = async (key, except = []) => (await stats(key)).filter((item) => !except.includes(item.label)).reduce((sum, item) => sum + item.value, 0);
/** A ready row shows its figures and its source; a refused one says so; an unavailable one names its reason. */
const expectRow = async (label, key, state) => {
  const node = row(key); await node.waitFor();
  assert.equal(await node.getAttribute("data-source-state"), state, `${label}: ligne ${key}`);
  const cells = await node.locator("td").allInnerTexts();
  if (state === "not_granted") assert.deepEqual(cells, ["Non accordé à ce contexte.", "Aucune lecture émise."], `${label}: ${key} non accordé`);
  else if (state === "unavailable") assert.ok(UNAVAILABLE.includes(cells[0] ?? ""), `${label}: ${key} nomme sa raison (${cells[0]})`);
  else {
    assert.ok(await node.locator(".ov-stat").count() > 0 || /Aucun worker enregistré/.test(cells[0] ?? ""), `${label}: ${key} porte ses valeurs`);
    assert.ok(["Registre complet du contrôle", "Sélection autorisée de ce contexte", "Journal d’audit vérifié"].includes(cells.at(-1) ?? ""), `${label}: ${key} nomme sa source`);
  }
};
const attentionItem = (kind, id) => page.locator(`.ov-attention li[data-incident-kind="${kind}"][data-subject-id="${id}"]`);
const groupRow = (id) => page.locator(`.ov-groups tr[data-broker-id="${id}"]`);
const refreshSummary = async () => {
  const before = summaryReads();
  await button("Actualiser la synthèse").click();
  for (let attempt = 0; attempt < 600 && summaryReads() === before; attempt++) await page.waitForTimeout(100);
  assert.ok(summaryReads() > before, "« Actualiser la synthèse » relit la synthèse");
};
const openContext = async (mode, tenant) => { await button(`Ouvrir ${mode} · ${tenant}`).click(); };

/** Captures a state at every width and theme, with deterministic names, after checking that nothing overflows. */
const shots = async (view, state, widths) => {
  for (const width of widths) for (const theme of ["light", "dark"]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((value) => { document.documentElement.dataset.theme = value; window.scrollTo(0, 0); }, theme); await settle();
    const measured = await page.evaluate(() => {
      const dock = document.querySelector(".human-toolbar")?.getBoundingClientRect();
      const top = document.querySelector(".human-page .page-heading")?.getBoundingClientRect().top ?? 0;
      return { fits: document.documentElement.scrollWidth <= innerWidth, clear: !dock || top >= dock.bottom - 1,
        ledger: [...document.querySelectorAll(".ov-scroll")].every((box) => box.scrollWidth <= box.clientWidth + 1 || innerWidth > 740) };
    });
    assert.equal(measured.fits, true, `${view} ${state} ${width}px ${theme}: aucun débordement global`);
    assert.equal(measured.clear, true, `${view} ${state} ${width}px ${theme}: aucun contenu sous les commandes fixes`);
    soft(measured.ledger, `${view} ${state} ${width}px ${theme}: une table dense défile horizontalement sous 740 px`);
    await page.screenshot({ path: join(artifacts, `admin360-${view}-${state}-${width}-${theme}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; window.scrollTo(0, 0); }); await settle();
};
const errorBody = (code, message, retry_action) => ({ status: code === "permission_denied" ? 403 : 503, contentType: "application/json",
  headers: { "cache-control": "no-store" }, body: JSON.stringify({ error: { code, message, retry_action } }) });

const ADMIN_TABS = ["Contextes", "Vue 360°", "Quotas et usage", "Supervision", "Journal", "Ressources", "Événements", "Brokers", "Workers", "Fédération"];
const PERSONAL_TABS = ["Contextes", "Mon espace", "Supervision", "Journal", "Ressources", "Événements"];
const READY_ALL = Object.fromEntries(LEDGER_ROWS.map((key) => [key, "ready"]));
const PERSONAL_ROWS = { workers: "not_granted", brokers: "not_granted", resources: "ready", decisions: "ready", quota: "not_granted", usage: "not_granted" };
const profiles = [
  { account: "admin_a", mode: "administration", tenant: "tenant-a", heading: "Vue 360° du tenant.", tabs: ADMIN_TABS, rows: READY_ALL, ids: a, foreign: idsB,
    workers: a.workers.length, brokers: brokersA.length, sessions: a.granted_sessions.length },
  { account: "user_a1", mode: "personnel", tenant: "tenant-a", heading: "Mon espace.", tabs: PERSONAL_TABS, rows: PERSONAL_ROWS, foreign: idsB,
    sessions: a.granted_sessions.length - 1 },
  { account: "user_a2", mode: "personnel", tenant: "tenant-a", heading: "Mon espace.", tabs: PERSONAL_TABS, rows: PERSONAL_ROWS, foreign: idsB, sessions: 2 },
  { account: "workers_only_a", mode: "administration", tenant: "tenant-a", heading: "Vue 360° du tenant.",
    tabs: ["Contextes", "Vue 360°", "Quotas et usage", "Supervision", "Ressources", "Événements", "Workers"],
    rows: { workers: "ready", brokers: "not_granted", resources: "not_granted", decisions: "not_granted", quota: "ready", usage: "ready" }, foreign: idsB, workers: a.workers.length },
  { account: "brokers_only_a", mode: "administration", tenant: "tenant-a", heading: "Vue 360° du tenant.",
    tabs: ["Contextes", "Vue 360°", "Supervision", "Ressources", "Événements", "Brokers"],
    rows: { workers: "not_granted", brokers: "ready", resources: "not_granted", decisions: "not_granted", quota: "not_granted", usage: "not_granted" }, foreign: idsB, brokers: brokersA.length },
  { account: "admin_b", mode: "administration", tenant: "tenant-b", heading: "Vue 360° du tenant.", tabs: ADMIN_TABS, rows: READY_ALL, foreign: idsA,
    workers: b.workers.length, brokers: b.brokers.length, sessions: 1 },
];

let stage = "start";
/** Per profile: exact destinations, the state of each ledger row and its values against the seed identifiers. */
const runProfiles = async () => {
  await command({ command: "heartbeat" });
  for (const profile of profiles) {
    stage = `profiles_${profile.account}`;
    const identity = await login(profile.account);
    const selected = identity.contexts.find((item) => item.tenant_id === profile.tenant && item.mode === (profile.mode === "personnel" ? "personal" : "administration"));
    assert.ok(selected, `${profile.account}: le contexte ${profile.mode} de ${profile.tenant} est proposé`);
    const answered = page.waitForResponse((reply) => isReply(reply, "/api/v1/human/summary"));
    await openContext(profile.mode, profile.tenant);
    // A served summary is the landing of the context.
    await heading(profile.heading).waitFor();
    const reply = await answered; assert.equal(reply.status(), 200, `${profile.account}: synthèse lue`);
    const wire = await reply.text();
    for (const id of profile.foreign) assert.ok(!wire.includes(id), `${profile.account}: la synthèse ne contient aucun identifiant d’un autre tenant`);
    await summaryReady();
    assert.deepEqual(await tabs(), profile.tabs, `${profile.account}: destinations exactes`);
    assert.equal(reply.request().headers()["x-aiguard-context"], selected.id, `${profile.account}: la synthèse est lue dans le contexte choisi`);
    for (const key of LEDGER_ROWS) await expectRow(profile.account, key, profile.rows[key]);
    if (profile.workers !== undefined) assert.equal(await total("workers", ["Quota retenu"]), profile.workers, `${profile.account}: total des workers du tenant`);
    if (profile.brokers !== undefined) assert.equal(await total("brokers"), profile.brokers, `${profile.account}: total des brokers du tenant`);
    if (profile.sessions !== undefined) {
      const sessions = (await stats("resources")).find((item) => item.label === "Sessions")?.value;
      soft(sessions === profile.sessions, `${profile.account}: ${sessions} sessions autorisées au lieu de ${profile.sessions}`);
    }
    if (profile.rows.workers === "ready" && profile.rows.quota === "ready") {
      const held = (await stats("workers")).find((item) => item.label === "Quota retenu")?.value;
      const reserved = (await stats("quota")).find((item) => item.label.startsWith("workers réservés sur"))?.value;
      soft(held === reserved, `${profile.account}: quota retenu par les workers (${held}) et réservations du quota (${reserved}) d’un même instantané`);
    }
    const attention = profile.rows.workers === "ready" || profile.rows.brokers === "ready";
    assert.equal(await heading("Points d’attention").count(), attention ? 1 : 0, `${profile.account}: points d’attention selon les registres accordés`);
    if (profile.account === "admin_a" || profile.account === "brokers_only_a") {
      await attentionItem("broker_revoked", a.brokers.revoked).waitFor();
      assert.equal(await attentionItem("broker_revoked", a.brokers.revoked).getByRole("button", { name: "Ouvrir le broker", exact: true }).count(), 1,
        `${profile.account}: le broker révoqué s’ouvre dans le registre accordé`);
    }
    if (profile.account === "workers_only_a")
      assert.equal(await page.getByRole("button", { name: "Ouvrir le broker", exact: true }).count(), 0, "workers seuls : aucun broker ne s’ouvre sans son registre");
    if (profile.rows.workers === "ready") {
      for (const id of await page.locator(".ov-groups tr[data-broker-id]").evaluateAll((nodes) => nodes.map((node) => node.dataset.brokerId).filter(Boolean)))
        assert.ok((profile.tenant === "tenant-a" ? brokersA : b.brokers).includes(id), `${profile.account}: un groupe nomme un broker de son tenant`);
    } else assert.equal(await page.locator(".ov-groups").count(), 0, `${profile.account}: aucune flotte par broker sans registre des workers`);
    await noneOf(profile.account, profile.foreign); await noSecret(profile.account);
    // The administration-only reads stay out of a personal context, and a context without metadata has nothing to land on.
    if (profile.account === "workers_only_a") {
      await button("Contextes").click(); await heading("Choisir un contexte.").waitFor();
      await openContext("personnel", "tenant-a");
      await page.getByText("Ce contexte n’offre aucune opération de consultation disponible dans ce build. Faites vérifier vos accès.", { exact: true }).waitFor();
      assert.deepEqual(await tabs(), ["Contextes", "Ressources", "Événements"], "workers seuls, contexte personnel : aucune destination de lecture");
    }
    await logout();
  }
};

/** The keyboard reaches the summary commands, then a point of attention, and opens that broker's exact detail. */
const runKeyboard = async () => {
  stage = "keyboard";
  await page.emulateMedia({ reducedMotion: "reduce" });
  await login("admin_a"); await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor(); await summaryReady();
  assert.equal(await page.evaluate(() => document.activeElement?.tagName), "H1", "le titre de la vue reçoit le focus à l’ouverture");
  const focus = () => page.evaluate(() => {
    const node = document.activeElement; const rect = node?.getBoundingClientRect();
    return { tag: node?.tagName ?? "", key: node?.getAttribute("data-focus-key") ?? "", text: node?.textContent ?? "",
      inView: !!rect && rect.bottom > 0 && rect.top < innerHeight, inOverview: !!node?.closest(".ov"),
      afterLedger: !!node && !!document.querySelector(".ov-ledger") && !!(document.querySelector(".ov-ledger").compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING) };
  });
  const target = `ov-open-broker-${a.brokers.revoked}-broker_revoked`;
  const order = [];
  for (let count = 0; count < 120; count++) {
    await page.keyboard.press("Tab"); await settle();
    const state = await focus();
    assert.notEqual(state.tag, "BODY", "le focus ne retombe jamais sur le corps de la page");
    if (state.inOverview) { order.push(state.key); assert.ok(state.inView, `la commande ${state.key} focalisée est visible`); }
    if (state.key === target) { assert.ok(state.afterLedger, "le point d’attention suit le registre de synthèse dans l’ordre du clavier"); break; }
  }
  assert.equal(order.at(-1), target, `le clavier atteint « Ouvrir le broker » du broker révoqué (parcours : ${order.join(", ")})`);
  assert.deepEqual(order.slice(0, 3), ["ov-window", "ov-cadence", "ov-refresh"], "période, relecture puis actualisation précèdent les sections");
  await page.keyboard.press("Enter");
  await heading("Brokers enregistrés du tenant.").waitFor();
  const dialog = page.locator(`dialog[open][data-broker-id="${a.brokers.revoked}"]`); await dialog.waitFor();
  await dialog.getByRole("heading", { name: "Détails du broker", exact: true }).waitFor();
  assert.ok(await page.evaluate(() => !!document.activeElement?.closest("dialog[open]")), "le focus est dans le détail ouvert");
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  await page.emulateMedia({ reducedMotion: null });
  await logout();
};

/** Loading, ready, not granted, stale and refused, captured on both themes; Quotas et usage ready as well. */
const runStates = async () => {
  stage = "states_loading";
  await login("admin_a");
  let release; const gate = new Promise((resolve) => { release = resolve; });
  await page.route("**/api/v1/human/summary**", async (route) => { await gate; await route.fallback(); });
  await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor();
  await page.getByText("Lecture de la synthèse…", { exact: true }).waitFor();
  assert.equal(await ledger().count(), 0, "chargement : aucune valeur avant la réponse du contrôle");
  await shots("overview", "loading", [1440, 390, 320]);
  release(); await page.unroute("**/api/v1/human/summary**");
  stage = "states_ready";
  await summaryReady();
  await shots("overview", "ready", [1440, 390, 320]);

  stage = "states_stale";
  const kept = await row("workers").innerText();
  await page.route("**/api/v1/human/summary**", (route) => route.fulfill(errorBody("audit_unavailable", "Audit du tenant momentanément indisponible.", "retry")));
  await refreshSummary();
  await page.locator('.ov[data-state="stale"]').waitFor();
  await page.getByText("Données périmées · dernière synthèse conservée.", { exact: true }).waitFor();
  assert.equal(await row("workers").innerText(), kept, "périmé : la dernière synthèse reste affichée, datée");
  await shots("overview", "stale", [1440, 390, 320]);
  await page.unroute("**/api/v1/human/summary**");

  stage = "states_refused";
  const identities = identityReads();
  await page.route("**/api/v1/human/summary**", (route) => route.fulfill(errorBody("permission_denied", "Lecture refusée.", "review_permissions")));
  await refreshSummary();
  await page.locator('.ov[data-state="refused"]').waitFor();
  await page.getByText("Synthèse refusée pour ce contexte. Vos accès sont relus ; aucune donnée antérieure n’est conservée.", { exact: true }).waitFor();
  assert.equal(await ledger().count(), 0, "refusé : les valeurs sont retirées");
  for (let attempt = 0; attempt < 300 && identityReads() === identities; attempt++) await page.waitForTimeout(100);
  assert.ok(identityReads() > identities, "refusé : les accès sont relus");
  await shots("overview", "refused", [1440, 390, 320]);
  await page.unroute("**/api/v1/human/summary**");
  await refreshSummary(); await summaryReady();

  stage = "states_usage";
  await button("Quotas et usage").click(); await heading("Quotas et usage du tenant.").waitFor();
  await page.locator('.ov-usage[data-state="ready"]').waitFor();
  await page.getByRole("heading", { name: "Quota du tenant", exact: true }).waitFor();
  assert.equal(await ledger().count(), 0, "quitter la Vue 360° retire ses valeurs");
  await shots("usage", "ready", [1440, 390]);
  await noSecret("états"); await logout();

  stage = "states_not_granted";
  await login("user_a1"); await openContext("personnel", "tenant-a"); await heading("Mon espace.").waitFor(); await summaryReady();
  for (const key of ["workers", "brokers", "quota", "usage"]) await expectRow("user_a1", key, "not_granted");
  await shots("overview", "not-granted", [1440, 390, 320]);
  await logout();
};

/** A revoked broker is counted as revoked in the ledger, joined in its group and listed as a point of attention. */
const runBroker = async () => {
  stage = "broker";
  await command({ command: "heartbeat" });
  await login("admin_a"); await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor(); await summaryReady();
  const revokedBefore = (await stats("brokers")).find((item) => item.label === "Autorisation révoquée")?.value ?? 0;
  assert.equal(await attentionItem("broker_revoked", a.brokers.current).count(), 0, "le broker courant n’est pas encore un point d’attention de révocation");
  await command({ command: "revoke_broker", broker: "current" });
  await refreshSummary(); await summaryReady();
  await attentionItem("broker_revoked", a.brokers.current).waitFor();
  const revokedAfter = (await stats("brokers")).find((item) => item.label === "Autorisation révoquée")?.value ?? 0;
  assert.equal(revokedAfter, revokedBefore + 1, "le registre compte un broker révoqué de plus");
  assert.equal(await total("brokers"), brokersA.length, "la révocation ne change pas le nombre de brokers enregistrés");
  if (await groupRow(a.brokers.current).count())
    await groupRow(a.brokers.current).getByText("Autorisation révoquée", { exact: true }).waitFor();
  await noneOf("révocation broker", idsB);
  await logout();
};

/** A reduced membership withdraws the summary, its fleet table and an open administration view, then reads the accesses. */
const runRights = async () => {
  const withdrawn = async (label, view) => {
    const before = requests.length;
    await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
    await heading("Choisir un contexte.").waitFor();
    for (let attempt = 0; attempt < 300 && identityReads(before) === 0; attempt++) await page.waitForTimeout(100);
    assert.ok(identityReads(before) >= 1, `${label}: les accès sont relus`);
    assert.equal(await page.locator(`.ov, ${view}, .ov-groups, dialog[open]`).count(), 0, `${label}: valeurs, flotte par broker et vue d’administration retirées`);
    assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0, `${label}: un changement de droits n’offre aucune reprise`);
  };
  stage = "rights_summary";
  await login("admin_a"); await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor(); await summaryReady();
  await page.locator(".ov-groups").waitFor();
  await command({ command: "set_capabilities", account: "admin_a", capabilities: reduced });
  await withdrawn("droits réduits", ".ov-ledger");
  await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor(); await summaryReady();
  assert.deepEqual(await tabs(), ["Contextes", "Vue 360°", "Supervision", "Journal", "Ressources", "Événements", "Brokers"], "le registre des workers retiré emporte quotas, workers et fédération");
  for (const key of ["workers", "quota", "usage"]) await expectRow("droits réduits", key, "not_granted");
  for (const key of ["brokers", "resources", "decisions"]) await expectRow("droits réduits", key, "ready");
  assert.equal(await page.locator(".ov-groups").count(), 0, "plus de flotte par broker sans registre des workers");

  stage = "rights_admin_views";
  await command({ command: "set_capabilities", account: "admin_a", capabilities: [...full, "read_memberships", "read_configuration"] });
  await withdrawn("droits élargis", ".ov-ledger");
  await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor(); await summaryReady();
  assert.deepEqual(await tabs(), ["Contextes", "Vue 360°", "Utilisateurs", "Quotas et usage", "Configuration", "Supervision", "Journal", "Ressources", "Événements", "Brokers", "Workers", "Fédération"]);
  await button("Configuration").click(); await heading("Configuration effective.").waitFor();
  await page.locator('.ov-configuration[data-state="ready"]').waitFor();
  assert.ok(await page.locator(".ov-configuration tr[data-configuration-key]").count() > 0, "la configuration effective est listée avec sa provenance");
  await shots("configuration", "ready", [1440, 390]);
  await button("Utilisateurs").click(); await heading("Utilisateurs de l’organisation.").waitFor();
  await page.locator('.ov-memberships[data-state="ready"]').waitFor();
  const members = page.locator(".ov-memberships tr[data-membership-id]");
  assert.ok(await members.count() >= 1);
  soft(await members.count() === 5, `les cinq appartenances de l’organisation de A sont listées (constaté : ${await members.count()})`);
  await page.getByText("Votre appartenance : changement réservé au contrôle", { exact: true }).waitFor();
  assert.equal(await page.locator(".ov-memberships").getByRole("button", { name: "Modifier", exact: true }).count(), 0, "aucune commande de changement n’est offerte");
  await noneOf("utilisateurs", idsB); await noSecret("utilisateurs");
  await shots("memberships", "ready", [1440, 390]);
  // The open administration view goes with the right that served it.
  await command({ command: "set_capabilities", account: "admin_a", capabilities: full });
  await withdrawn("lecture des appartenances retirée", ".ov-memberships");
  await openContext("administration", "tenant-a"); await heading("Vue 360° du tenant.").waitFor(); await summaryReady();
  assert.deepEqual(await tabs(), ADMIN_TABS, "les droits complets sont rétablis");
  await logout();
};

/** A credential revoked while the summary is displayed ends the session: back to the sign-in, nothing displayed. */
const runRevoked = async () => {
  stage = "revoked";
  await login("user_a2"); await openContext("personnel", "tenant-a"); await heading("Mon espace.").waitFor(); await summaryReady();
  await command({ command: "revoke_credential", account: "user_a2" });
  await button("Actualiser la synthèse").click();
  await page.getByLabel("Credential personnel local").waitFor();
  assert.equal(await page.locator(".human-shell, .ov, dialog[open]").count(), 0, "credential révoqué : retour à la connexion, aucune donnée affichée");
  await noSecret("credential révoqué");
};

let failure = null;
try {
  const run = { profiles: runProfiles, keyboard: runKeyboard, states: runStates, broker: runBroker, rights: runRights, revoked: runRevoked };
  for (const phase of scenario === "all" ? phases : [scenario]) await run[phase]();
  assert.deepEqual(guard.violations, [], "le navigateur reste sur l’origine de la fixture, en lecture seule");
  assert.deepEqual(softFailures, [], "observations indépendantes du scénario");
  assert.deepEqual(errors, [], "aucune erreur JavaScript de page");
  console.log(`browser_admin360 scenario=${scenario} PASS workers=${workers} events=${events} artifacts=${artifacts}`);
} catch (error) {
  failure = error;
  if (softFailures.length) console.error(`observations indépendantes en échec avant l’arrêt :\n- ${softFailures.join("\n- ")}`);
  // A failure is captured only on the authenticated dashboard, never on the sign-in form that holds a credential.
  const mayCapture = page.url() === dashboard.origin + "/" && await page.locator(".human-shell").isVisible().catch(() => false) &&
    await page.getByLabel("Credential personnel local", { exact: true }).count().catch(() => 1) === 0;
  const snapshot = mayCapture ? await saveBrowserFailure(page, { directory: artifacts, secrets }) : null;
  console.error(`browser_admin360 scenario=${scenario} FAIL stage=${stage}`);
  if (snapshot) for (const file of snapshot.files) console.error(`artifact=${join(artifacts, snapshot.directory, file)}`);
} finally {
  await browser.close();
  if (stack.exitCode === null) { stack.stdin.write(`${JSON.stringify({ command: "stop" })}\n`); stack.stdin.end(); }
  await new Promise((resolve) => {
    const timer = setTimeout(() => { stack.kill("SIGKILL"); resolve(); }, 10000);
    stack.once("exit", () => { clearTimeout(timer); resolve(); }); if (stack.exitCode !== null) { clearTimeout(timer); resolve(); }
  });
}
if (failure) { console.error(failure); process.exit(1); }
