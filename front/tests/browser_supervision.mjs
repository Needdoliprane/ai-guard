// Unified supervision in Chromium against the real Rust gateway and dashboard routers.
// The data set is the synthetic seed of examples/supervision_stack.rs: real control mutations on
// private test SQLite stores, no runtime and no worker execution. Nothing here is imported by the app.
//
//   AI_GUARD_SUPERVISION_SCENARIO  fleet (default) | partial | correlation | rights | revoked | isolation | expiry |
//                                  resume | ttl | journal | keyboard | focus | regions | details | states | load |
//                                  resource-recovery | correlation-recovery | resource-stream-recovery | resource-stream-late-denial
//                                  resource-stream-visibility-race | journal-catch-up | journal-older-recovery | human-export
//                                  (ttl waits for a real 300-second context expiry)
//
// Prerequisite: the playwright package (1.64.x) and its Chromium. It is not a dependency of the frontend
// package; without it the script exits with code 2 and BLOCKED, and the scenario is NOT_RUN.
//   AI_GUARD_SUPERVISION_STACK     path of the built supervision_stack example
//   AI_GUARD_SUPERVISION_FRONT_DIR built frontend directory (default: front/dist)
//   AI_GUARD_SUPERVISION_ARTIFACTS directory for screenshots (default: a new temporary directory)
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { access, chmod, mkdir, mkdtemp, readFile, stat } from "node:fs/promises";
import { createRequire } from "node:module";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { saveBrowserFailure } from "./browser_failure.mjs";
import { installBrowserGuard } from "./browser_live.mjs";

const scenario = process.env.AI_GUARD_SUPERVISION_SCENARIO || "fleet";
const guardedRecovery = ["partial", "keyboard", "resource-recovery", "correlation-recovery", "resource-stream-recovery", "resource-stream-late-denial",
  "resource-stream-visibility-race", "journal-catch-up", "journal-older-recovery", "human-export", "ttl", "isolation", "journal"].includes(scenario);
const authenticatedFailure = ["partial", "keyboard", "revoked", "human-export", "ttl", "isolation", "journal"].includes(scenario);
const privateFailure = guardedRecovery || authenticatedFailure;
const blocked = (reason) => { console.error(`BLOCKED: ${reason}. Scenario ${scenario} is NOT_RUN.`); process.exit(2); };
if (privateFailure && (process.env.DEBUG || process.env.PWDEBUG || process.env.NODE_DEBUG))
  blocked("disable debug logs for the authenticated browser recipe");
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

const sizes = { fleet: [120, 300], partial: [24, 60], correlation: [120, 300], rights: [24, 60], revoked: [24, 60], isolation: [24, 60], expiry: [24, 60],
  resume: [24, 60], ttl: [24, 60], journal: [24, 1500], keyboard: [24, 60], focus: [120, 300], regions: [24, 300], details: [24, 60], states: [24, 60], load: [300, 60],
  "resource-recovery": [24, 60], "correlation-recovery": [24, 60], "resource-stream-recovery": [24, 60], "resource-stream-late-denial": [24, 60],
  "resource-stream-visibility-race": [24, 60], "journal-catch-up": [24, 60], "journal-older-recovery": [24, 300], "human-export": [24, 60] };
const [workers, events] = sizes[scenario] ?? blocked(`unknown scenario ${scenario}`);
const stateDir = await mkdtemp(join(tmpdir(), "ai-guard-supervision-state-")); await chmod(stateDir, 0o700);
const artifacts = process.env.AI_GUARD_SUPERVISION_ARTIFACTS || await mkdtemp(join(tmpdir(), "ai-guard-supervision-browser-"));
await mkdir(artifacts, { recursive: true, ...(privateFailure ? { mode: 0o700 } : {}) });

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
const ready = await nextLine(600000).catch((error) => { stack.kill(); blocked(error.message); });
assert.equal(ready.ready, true);
const command = async (value) => {
  stack.stdin.write(`${JSON.stringify(value)}\n`); const reply = await nextLine(30000);
  assert.equal(reply.ok, true, `stack command ${value.command}: ${reply.error ?? ""}`); return reply;
};
const { accounts, ids } = ready; const secrets = Object.values(accounts);
const a = ids.tenant_a; const b = ids.tenant_b;
const sortedWorkersA = [...a.workers].sort();

const browser = await chromium.launch({ headless: true, chromiumSandbox: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
  ...(guardedRecovery ? { args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] } : {}) })
  .catch(() => { stack.kill("SIGKILL"); blocked("Chromium failed to launch with its sandbox"); });
// A browser that cannot open its page is closed at once, with the stack: no process outlives this script.
const opened = await (async () => { const context = await browser.newContext({ viewport: { width: 1440, height: 1050 },
  ...(guardedRecovery ? { serviceWorkers: "block" } : {}) }); return { context, page: await context.newPage() }; })()
  .catch(async (error) => { await browser.close().catch(() => {}); stack.kill("SIGKILL"); blocked(`Chromium could not open a page (${String(error?.message ?? error).split("\n")[0]})`); });
const { context, page } = opened; page.setDefaultTimeout(15000);
const errors = []; page.on("pageerror", (error) => errors.push(error.message));
const requests = []; const contexts = [];
page.on("request", (request) => {
  const url = new URL(request.url()); if (!url.pathname.startsWith("/api/")) return;
  requests.push(`${url.pathname}${url.search}`); contexts.push(request.headers()["x-aiguard-context"] ?? null);
});
const identityReads = (since = 0) => requests.slice(since).filter((path) => path === "/api/v1/human/identity").length;
// Every stream request and how it ended: an open stream is one that started and neither finished nor failed.
const streamEnds = [];
const streamEnded = (request, how) => { const url = new URL(request.url()); if (url.pathname === "/api/v1/human/stream") streamEnds.push(`${how} ${url.search}`); };
page.on("requestfailed", (request) => streamEnded(request, request.failure()?.errorText ?? "failed"));
page.on("requestfinished", (request) => streamEnded(request, "finished"));
const streamStarts = () => requests.filter((path) => path.startsWith("/api/v1/human/stream")).length;
const openStreams = async () => { for (let attempt = 0; attempt < 30 && streamStarts() > streamEnds.length; attempt++) await page.waitForTimeout(100); return streamStarts() - streamEnds.length; };
// One browser clock for expiry checks and timers, advanced only by the expiry scenarios. The scenarios fire
// the refresh cycle (30 s or 2 min) and the freshness tick (10 s) of the unified view instead of waiting.
await page.addInitScript((trackResourcePoll) => {
  const NativeDate = Date; const nativeNow = Date.now; const nativeTimeout = window.setTimeout; const nativeClear = window.clearTimeout; const nativeInterval = window.setInterval; let advance = 0;
  window.Date = new Proxy(NativeDate, {
    apply: () => new NativeDate(nativeNow() + advance).toString(),
    construct: (target, args, newTarget) => Reflect.construct(target, args.length ? args : [nativeNow() + advance], newTarget),
  });
  Date.now = () => nativeNow() + advance;
  window.__supervisionExpiry = []; const cycles = new Map(); const ticks = []; const resourcePolls = [];
  window.setTimeout = (handler, delay, ...args) => {
    if (typeof handler === "function" && delay > 200000 && delay <= 300100) window.__supervisionExpiry.push(() => handler(...args));
    const id = nativeTimeout(handler, delay, ...args);
    if (typeof handler === "function" && (delay === 30000 || delay === 120000)) cycles.set(id, () => handler(...args));
    return id;
  };
  window.clearTimeout = (id) => { cycles.delete(id); nativeClear(id); };
  window.setInterval = (handler, delay, ...args) => {
    if (typeof handler === "function" && delay === 10000) ticks.push(() => handler(...args));
    if (trackResourcePoll && typeof handler === "function" && delay === 30000) resourcePolls.push(() => handler(...args));
    return nativeInterval(handler, delay, ...args);
  };
  window.__supervisionCycle = () => { const due = [...cycles]; cycles.clear(); for (const [id, run] of due) { nativeClear(id); run(); } return due.length; };
  window.__supervisionTick = () => { for (const run of ticks) run(); return ticks.length; };
  window.__supervisionAdvance = (milliseconds) => { advance += milliseconds; };
  if (trackResourcePoll) window.__resourcePoll = () => { for (const run of resourcePolls) run(); return resourcePolls.length; };
}, scenario === "resource-stream-late-denial");
const humanReads = (since = 0) => requests.slice(since).filter((path) => path.startsWith("/api/v1/human/") && !path.startsWith("/api/v1/human/identity"));
// Independent observations of one scenario: all are reported together at its end.
const softFailures = [];
const soft = (condition, message) => { if (!condition) softFailures.push(message); };
const visibility = (state) => page.evaluate((value) => {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => value }); document.dispatchEvent(new Event("visibilitychange"));
}, state);
const fleetUrl = "/api/v1/human/federation/workers?worker_limit=100&broker_limit=100";
const fleetRoute = "**/api/v1/human/federation/workers**";

const button = (name) => page.getByRole("button", { name, exact: true });
const heading = (name) => page.getByRole("heading", { name, exact: true });
const settle = () => page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
const bodyText = () => page.locator("body").innerText();
const login = async (account) => {
  await page.goto(ready.dashboard);
  await page.getByLabel("Type d’accès").selectOption("human");
  await page.getByLabel("Credential personnel local").fill(accounts[account]); await button("Ouvrir mon espace").click();
  await heading("Choisir un contexte.").waitFor();
};
const logout = async () => { await button("Se déconnecter").click(); await page.getByLabel("Credential personnel local").waitFor(); };
const supervise = async (mode, tenant) => { await button(`Superviser ${mode} · ${tenant}`).click(); };
const source = (name) => page.locator(".sv-source", { hasText: name });
const ready100 = async (names = ["Workers", "Brokers", "Ressources"]) => { for (const name of names) await source(name).getByText("Lecture réussie", { exact: true }).waitFor(); };
const tabs = () => page.locator(".human-nav .human-tab").allInnerTexts();
const workerRows = () => page.locator(".sv-panel", { has: heading("Workers") }).locator("tbody tr");
const brokerRows = () => page.locator(".sv-panel", { has: heading("Brokers et autorités") }).locator("tbody tr");
const brokerSummary = () => page.locator(".sv-panel", { has: heading("Brokers et autorités") }).locator(".sv-summary").innerText();
const direct = (label) => page.locator(".sv-journal .sv-source", { hasText: "Direct" }).getByText(label, { exact: true });
/** The result line of the Journal. The banner region may say the same count: waits name the line itself. */
const journalResult = (text) => page.locator(".sv-journal .sv-summary").filter({ hasText: text });
/** What holds the keyboard focus, and whether it is on screen. */
const focused = () => page.evaluate(() => {
  const node = document.activeElement; const rect = node?.getBoundingClientRect();
  return { tag: node?.tagName ?? "", key: node?.getAttribute("data-focus-key") ?? "", classes: node?.className ?? "", inView: !!rect && rect.bottom > 0 && rect.top < innerHeight,
    panel: node?.closest(".sv-panel")?.querySelector("h2")?.textContent ?? "", inSupervision: !!node?.closest(".sv"), row: node?.closest("tr")?.dataset.eventId ?? node?.closest("tr")?.rowIndex ?? null };
});
const workerSummaryStarts = (prefix) => page.locator(".sv-panel", { has: heading("Workers") }).locator(".sv-summary", { hasText: new RegExp(`^${prefix} affichés sur`) }).waitFor({ timeout: 30000 });
const workerSummary = () => page.locator(".sv-panel", { has: heading("Workers") }).locator(".sv-summary").innerText();
// Records, from now on, every text given to a live region under `scope`, with its time: a region that is
// rewritten, and a region that is inserted with its text.
const listen = (scope) => page.evaluate((target) => {
  const root = document.querySelector(target); const texts = new Map(); window.__said = [];
  const regions = () => [...root.querySelectorAll('[role="status"], [aria-live], [role="alert"]')];
  for (const node of regions()) texts.set(node, node.textContent ?? "");
  window.__saidObserver?.disconnect();
  window.__saidObserver = new MutationObserver(() => {
    for (const node of regions()) {
      const text = node.textContent ?? ""; if (texts.get(node) === text) continue;
      texts.set(node, text); if (text) window.__said.push({ at: Math.round(performance.now()), classes: node.className, text });
    }
  });
  window.__saidObserver.observe(root, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["role", "aria-live"] });
}, scope);
const said = (from = 0) => page.evaluate((start) => window.__said.slice(start), from);
const told = (entries) => JSON.stringify(entries.map((entry) => ({ ...entry, text: entry.text.slice(0, 90) })));
const noSecret = async (label) => { const text = await bodyText(); for (const secret of secrets) assert.ok(!text.includes(secret), `${label}: aucun credential dans la page`); };
/** No global overflow, nothing under the fixed commands, one row of destinations, dense tables and an open detail inside their box. */
const geometry = async (label, widths, { shots = true, also = [] } = {}) => {
  for (const width of widths) for (const theme of ["light", "dark"]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((value) => { document.documentElement.dataset.theme = value; window.scrollTo(0, 0); }, theme); await settle();
    const measured = await page.evaluate(() => {
      const dock = document.querySelector(".human-toolbar").getBoundingClientRect();
      const tabs = [...document.querySelectorAll(".human-nav .human-tab")].map((node) => node.getBoundingClientRect());
      const current = document.querySelector('.human-nav [aria-current="page"]')?.getBoundingClientRect();
      const row = document.querySelector(".human-nav"); const nav = row.getBoundingClientRect(); const dialog = document.querySelector("dialog[open]");
      return { fits: document.documentElement.scrollWidth <= innerWidth, headingTop: document.querySelector(".human-page .page-heading").getBoundingClientRect().top,
        overflow: row.scrollWidth > row.clientWidth + 1, more: row.dataset.more ?? "", dialog: !dialog || dialog.scrollWidth <= dialog.clientWidth + 1,
        dockBottom: dock.bottom, tabsInsideDock: tabs.every((tab) => tab.top >= dock.top - 1 && tab.bottom <= dock.bottom + 1),
        tabRows: new Set(tabs.map((tab) => Math.round(tab.top))).size, tabCount: tabs.length,
        currentVisible: !current || current.left >= nav.left - 1 && current.right <= nav.right + 1,
        tables: [...document.querySelectorAll(".sv-table")].every((table) => table.scrollWidth <= table.parentElement.clientWidth + 1) };
    });
    assert.equal(measured.fits, true, `${label} ${width}px ${theme} : aucun débordement global`);
    assert.ok(measured.headingTop >= measured.dockBottom - 1, `${label} ${width}px ${theme} : aucun contenu sous les commandes fixes`);
    assert.equal(measured.tabsInsideDock, true, `${label} ${width}px ${theme} : les ${measured.tabCount} destinations restent dans la barre`);
    assert.equal(measured.currentVisible, true, `${label} ${width}px ${theme} : la destination courante est visible`);
    assert.equal(measured.dialog, true, `${label} ${width}px ${theme} : le détail ouvert ne défile pas horizontalement`);
    soft(!measured.overflow || measured.more !== "", `${label} ${width}px ${theme} : la barre de destinations défile sans le signaler (${measured.tabCount} destinations)`);
    if (width <= 740) {
      assert.equal(measured.tables, true, `${label} ${width}px ${theme} : tables denses sans défilement horizontal`);
      if (measured.tabCount >= 5) assert.equal(measured.tabRows, 1, `${label} ${width}px ${theme} : ${measured.tabCount} destinations sur une seule rangée défilable`);
    }
    if (shots) await page.screenshot({ path: join(artifacts, `supervision-${label}-${width}-${theme}.png`) });
    // Further captures of the same state, each with a named region brought under the fixed commands.
    for (const [name, selector] of shots ? also : []) {
      await page.evaluate((target) => {
        const node = document.querySelector(target); const dock = document.querySelector(".human-toolbar").getBoundingClientRect().bottom;
        if (node) window.scrollTo(0, node.getBoundingClientRect().top + window.scrollY - dock - 8);
      }, selector); await settle();
      await page.screenshot({ path: join(artifacts, `supervision-${label}-${name}-${width}-${theme}.png`) });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; window.scrollTo(0, 0); });
};

const runFleet = async () => {
  await login("admin_a");
  assert.equal(await button("Ouvrir administration · tenant-a").count(), 1, "l’atterrissage existant reste proposé");
  assert.equal(await button("Superviser administration · tenant-a").count(), 1, "l’entrée de la vue unifiée est proposée pour ce contexte");
  const before = requests.length;
  await supervise("administration", "tenant-a"); await heading("Supervision du tenant.").waitFor(); await ready100();
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Journal", "Ressources", "Événements", "Brokers", "Workers", "Fédération"]);
  assert.deepEqual(humanReads(before).sort(), ["/api/v1/human/federation/workers?worker_limit=100&broker_limit=100", "/api/v1/human/resources?limit=100"],
    "une composition de 100 lignes et une lecture de ressources, rien d’autre");
  assert.ok(requests.every((path) => !/tenant/i.test(path)), "aucune requête ne choisit un tenant");

  // 100 real registry rows of 120, never presented as a tenant total.
  assert.equal(await workerRows().count(), 100);
  assert.equal(await workerSummary(), "100 workers affichés sur 100 chargés · d’autres pages existent au contrôle, aucun total du tenant n’est connu");
  await page.getByText("Couverture partielle", { exact: true }).waitFor();
  await page.getByText("Workers : 100 lignes chargées, d’autres pages existent au contrôle.", { exact: true }).waitFor();
  await page.getByText("Inventaire runtime : inconnu. Ces registres ne prouvent ni exécution, ni arrêt, ni libération de quota.", { exact: true }).waitFor();
  assert.equal(await brokerRows().count(), 3, "les trois brokers enregistrés, avec ou sans worker associé");
  for (const [name, label] of [["current", "Bail courant"], ["revoked", "Autorisation révoquée"], ["unseen", "Heartbeat non reçu"]]) {
    const row = brokerRows().filter({ hasText: a.brokers[name] });
    assert.equal(await row.count(), 1); await row.getByText(label, { exact: true }).waitFor();
    await row.getByText("Association historique d’autorité · ne prouve pas l’exécution par ce broker.", { exact: true }).waitFor();
  }
  await page.getByText("Autorité de lancement non enregistrée dans le registre brokers", { exact: true }).waitFor();
  let text = await bodyText();
  for (const foreign of [...b.workers, ...b.brokers, b.session]) assert.ok(!text.includes(foreign), "aucun identifiant du tenant B dans la vue de A");
  await noSecret("flotte");
  await geometry("fleet", [1440, 1024, 768, 390, 320], { also: [["brokers", ".sv-panel"], ["workers", ".sv-toolbar"]] });

  // The broker table has its own search over its loaded rows; it never filters the workers.
  const brokerSearch = page.getByLabel("Rechercher dans les brokers chargés");
  soft(await brokerSearch.count() === 1, "la table des brokers a sa propre recherche");
  if (await brokerSearch.count()) {
    await brokerSearch.fill(a.brokers.revoked);
    assert.equal(await brokerRows().count(), 1);
    assert.ok((await brokerSummary()).startsWith("1 broker affiché sur 3 chargés · fin du registre à cette lecture"));
    assert.equal(await workerRows().count(), 100, "la recherche des brokers ne filtre pas les workers");
    await brokerSearch.fill("no-such-broker");
    await heading("Aucun broker chargé ne correspond").waitFor();
    await brokerSearch.fill(""); assert.equal(await brokerRows().count(), 3);
  }

  // Search and filters work on the loaded rows and say so; typing never loses focus.
  const first = sortedWorkersA[0]; const search = page.getByLabel("Rechercher dans les lignes chargées");
  await search.click(); await page.keyboard.type(first);
  assert.equal(await page.evaluate(() => document.activeElement?.id), "sv-worker-search", "la saisie garde le focus pendant la mise à jour des résultats");
  assert.equal(await workerRows().count(), 1);
  assert.ok((await workerSummary()).startsWith("1 worker affiché sur 100 chargés · d’autres pages existent au contrôle"));
  assert.ok((await workerSummary()).includes("recherche et filtres appliqués aux seules lignes chargées"));
  await workerRows().getByText(first, { exact: true }).waitFor();
  const outside = sortedWorkersA.at(-1);
  await search.fill(outside);
  await heading("Aucune ligne chargée ne correspond").waitFor();
  await page.getByText(/une ligne absente d’ici n’est pas absente du tenant/).waitFor();
  await button("Réinitialiser").click(); assert.equal(await workerRows().count(), 100);
  await page.getByLabel("État du contrôle").selectOption("uncertain");
  const uncertain = await workerRows().count();
  assert.ok(uncertain > 0 && uncertain < 100);
  assert.equal(await workerRows().filter({ hasText: "in_doubt" }).count(), uncertain, "le filtre ne garde que l’état incertain du contrôle");
  await page.getByLabel("État du contrôle").selectOption("all");
  await button("Points d’attention").click();
  assert.equal(await button("Points d’attention").getAttribute("aria-pressed"), "true");
  const flagged = await workerRows().count();
  assert.ok(flagged > 0 && flagged < 100);
  assert.equal(await workerRows().filter({ has: page.locator(".sv-flags .badge") }).count(), flagged);
  await button("Réinitialiser").click();
  assert.equal(await button("Points d’attention").getAttribute("aria-pressed"), "false");

  // Broker -> workers through the exact authority association.
  const currentRow = brokerRows().filter({ hasText: a.brokers.current });
  await currentRow.getByRole("button", { name: "Voir ces workers", exact: true }).click();
  assert.equal(await page.getByLabel("Broker ou autorité").inputValue(), a.brokers.current);
  const associated = await workerRows().count();
  assert.ok(associated > 0 && associated < 100);
  assert.equal(await workerRows().filter({ hasText: a.brokers.current }).count(), associated);
  assert.equal(await page.evaluate(() => document.activeElement?.classList.contains("sv-summary")), true, "le résultat du filtre reçoit le focus");
  await button("Réinitialiser").click();

  // Details come from the loaded page; Escape returns to the triggering row.
  const detail = workerRows().first().getByRole("button", { name: "Détails", exact: true });
  const key = await detail.getAttribute("data-focus-key");
  await detail.focus(); await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog"); await dialog.waitFor();
  for (const label of ["État du contrôle", "Dernière observation runtime", "Autorité du lancement", "Autorité de l’inspection", "Session déclarée"])
    assert.ok(await dialog.getByText(label, { exact: true }).count(), `le détail worker présente « ${label} »`);
  await dialog.getByText(/consommation réelle inconnue, non facturable/).waitFor();
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("data-focus-key")), key, "Échap rend le focus au bouton de la ligne");
  await currentRow.getByRole("button", { name: "Détails du broker", exact: true }).click();
  await dialog.getByText("Bail courant", { exact: true }).waitFor();
  await dialog.getByText(/l’inventaire runtime reste inconnu/).waitFor();
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });

  // One more page: the window reaches the end of the registry and says so.
  const beforeMore = requests.length;
  await button("Charger 100 lignes de plus").click();
  await page.getByText("120 workers affichés sur 120 chargés · fin du registre à cette lecture", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 120);
  assert.equal(humanReads(beforeMore).length, 1);
  assert.match(humanReads(beforeMore)[0], /^\/api\/v1\/human\/federation\/workers\?worker_limit=100&broker_limit=100&worker_cursor=/);
  const states = new Set(await workerRows().locator("td:nth-child(2) .sv-state .mono").allInnerTexts());
  for (const state of ["reserved", "running", "terminal", "in_doubt", "cancelled"]) assert.ok(states.has(state), `l’état ${state} du registre est affiché tel quel`);
  assert.ok(await workerRows().filter({ hasText: "Observation inconnue" }).count() > 0, "une observation absente reste inconnue");

  // A transport failure after a success keeps the last authorized window, dated and stale.
  await page.route("**/api/v1/human/federation/workers**", (route) => route.abort("connectionrefused"));
  await button("Actualiser maintenant").click();
  await source("Workers").getByText("Contrôle injoignable · dernière lecture conservée", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 120, "la dernière fenêtre autorisée reste affichée");
  await page.getByText("Workers : 120 lignes de la dernière lecture réussie, périmées.", { exact: true }).waitFor();
  await page.unroute("**/api/v1/human/federation/workers**");
  await button("Réessayer").first().click(); await ready100();
  assert.equal(await workerRows().count(), 120);
  await logout(); assert.equal(await page.locator(".sv, dialog").count(), 0, "la déconnexion retire la vue");

  // A failed first read is unknown content, never an empty registry.
  await login("admin_a");
  await page.route("**/api/v1/human/federation/workers**", (route) => route.abort("connectionrefused"));
  await supervise("administration", "tenant-a");
  await source("Workers").getByText("Contrôle injoignable", { exact: true }).waitFor();
  await heading("Workers inconnus").waitFor();
  assert.equal(await heading("Aucun worker enregistré à cette lecture").count(), 0, "une panne initiale n’est pas un registre vide");
  assert.equal(await workerRows().count(), 0);
  await page.getByText("Workers : lecture en échec, contenu inconnu. Une erreur n’est pas une absence d’activité.", { exact: true }).waitFor();
  await page.unroute("**/api/v1/human/federation/workers**"); await logout();

  // Tenant B sees exactly its own row; tenant C reads a successful empty registry.
  await login("admin_b"); await supervise("administration", "tenant-b"); await ready100();
  assert.equal(await workerRows().count(), 1); assert.equal(await brokerRows().count(), 1);
  assert.equal(await workerSummary(), "1 worker affiché sur 1 chargé · fin du registre à cette lecture");
  text = await bodyText();
  assert.ok(text.includes(b.workers[0]) && text.includes(b.brokers[0]));
  for (const foreign of [...sortedWorkersA.slice(0, 20), ...Object.values(a.brokers)]) assert.ok(!text.includes(foreign), "aucun identifiant du tenant A dans la vue de B");
  await logout();
  await login("admin_c"); await supervise("administration", "tenant-c"); await ready100();
  await heading("Aucun worker enregistré à cette lecture").waitFor(); await heading("Aucun broker enregistré à cette lecture").waitFor();
  await page.getByText("Workers : aucun worker enregistré à cette lecture réussie.", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 0);
  await geometry("empty", [390, 320]);
  await logout();
};

/** Contexts that grant one source only: the missing one is announced and never requested. */
const runPartial = async () => {
  const guard = await prepareRecoveryGuard();
  const widths = [1440, 390, 320];
  recoveryStage = "partial_workers";
  await login("workers_only_a");
  let before = requests.length;
  await supervise("administration", "tenant-a"); await heading("Supervision du tenant.").waitFor(); await ready100(["Workers"]);
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Ressources", "Événements", "Workers"]);
  assert.equal(await page.getByLabel("Rythme d’actualisation").count(), 1, "un registre accordé garde son rythme d’actualisation");
  assert.deepEqual(humanReads(before), ["/api/v1/human/workers?limit=100"], "ni composition, ni brokers, ni ressources pour un contexte workers seuls");
  await source("Brokers").getByText("Non accordé", { exact: true }).waitFor();
  await page.getByText("Brokers : registre non accordé à ce contexte, aucune lecture émise.", { exact: true }).waitFor();
  await page.getByText("Registre des brokers non accordé à ce contexte : seules les associations portées par les lignes de workers sont affichées.", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 24);
  await noSecret("workers seuls");
  recoveryStage = "partial_workers_geometry";
  await geometry("workers-only", widths);
  await logout();

  recoveryStage = "partial_brokers";
  await login("brokers_only_a"); before = requests.length;
  await supervise("administration", "tenant-a"); await ready100(["Brokers"]);
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Ressources", "Événements", "Brokers"]);
  assert.deepEqual(humanReads(before), ["/api/v1/human/brokers?limit=100"]);
  assert.equal(await brokerRows().count(), 3);
  assert.equal(await heading("Workers").count(), 0, "aucune table de workers sans capacité");
  await brokerRows().first().getByText("Registre des workers non accordé : associations inconnues", { exact: true }).waitFor();
  await noSecret("brokers seuls");
  recoveryStage = "partial_brokers_geometry";
  await geometry("brokers-only", widths);
  await logout();

  recoveryStage = "partial_personal";
  await login("user_a1"); before = requests.length;
  assert.equal(await button("Superviser administration · tenant-a").count(), 0, "aucun contexte d’administration n’est proposé à ce compte");
  await supervise("personnel", "tenant-a"); await heading("Supervision de mon espace.").waitFor(); await ready100(["Ressources"]);
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Journal", "Ressources", "Événements"]);
  assert.deepEqual(humanReads(before), ["/api/v1/human/resources?limit=100"]);
  assert.equal(await heading("Workers").count(), 0); assert.equal(await heading("Brokers et autorités").count(), 0);
  // No registry is granted: nothing was refused, nothing is refreshed automatically and there is no period to choose.
  await page.getByText("Aucun registre n’est accordé à ce contexte : rien n’est relu automatiquement. Les ressources sont relues par « Actualiser maintenant ».", { exact: true }).waitFor();
  assert.equal(await page.getByText(/lecture refusée par le contrôle/).count(), 0, "« non accordé » n’est jamais rendu comme un refus");
  assert.equal(await page.getByLabel("Rythme d’actualisation").count(), 0, "aucun rythme à choisir quand rien n’est relu automatiquement");
  await page.getByText("Accès de ce contexte", { exact: true }).click();
  assert.equal(await page.locator(".sv-access[open] summary .icon").count(), 1, "le panneau d’accès porte un indicateur de dépliage");
  await page.getByText("Le registre workers du tenant est réservé à un contexte d’administration portant read_worker_registry.", { exact: true }).waitFor();
  await page.getByText(/Relation non fournie par le contrat nominatif/).waitFor();
  assert.ok(await page.locator(".sv-table tbody tr").count() >= 10, "sessions, agents, sandboxes et projets accordés sont listés");
  await noSecret("ressources personnelles");
  recoveryStage = "partial_personal_geometry";
  await geometry("personal", widths);
  await logout();
  await noSecret("partiel");
  recoveryStage = "partial_network";
  assert.deepEqual(guard.violations, [], "le navigateur reste sur l’origine de la fixture");
  recoveryStage = "complete";
};

let recoveryStage = "browser_guard";
let recoveryOrigin;
async function prepareRecoveryGuard() {
  const dashboard = new URL(ready.dashboard);
  assert.ok(dashboard.protocol === "http:" && dashboard.hostname === "127.0.0.1" && dashboard.port && dashboard.origin === ready.dashboard);
  recoveryOrigin = dashboard.origin;
  const guard = await installBrowserGuard(context, [recoveryOrigin], secrets);
  await guard.preparePage(page);
  return guard;
}

/** Download a complete personal export from Rust and check its page receipts in fixture SQLite. */
const runHumanExport = async () => {
  const guard = await prepareRecoveryGuard();
  const tenant = "tenant-a";
  const exportPath = "/api/v1/human/export";
  const isReply = (answer, path) => answer.request().method() === "GET" && new URL(answer.url()).pathname === path;
  const resourceRow = (id) => page.locator(".human-resource-row").filter({ has: page.getByText(id, { exact: true }) });
  const openPersonal = async (account, displayName) => {
    const [answer] = await Promise.all([page.waitForResponse((reply) => isReply(reply, "/api/v1/human/identity")), login(account)]);
    assert.equal(answer.status(), 200);
    const identity = await answer.json();
    const personal = identity.contexts.filter((item) => item.tenant_id === tenant && item.mode === "personal");
    assert.equal(identity.display_name, displayName);
    assert.equal(identity.principal.kind, "human_local");
    assert.equal(personal.length, 1);
    await button(`Ouvrir personnel · ${tenant}`).click();
    return { principal: identity.principal, selected: personal[0] };
  };

  recoveryStage = "export_owner_login";
  const { principal, selected } = await openPersonal("user_a1", "Supervision test user A1");
  assert.ok(selected.capabilities.includes("read_content") && selected.capabilities.includes("export"));
  recoveryStage = "export_owner_preview";
  const resource = { kind: "session", resource_id: a.session_owned_by_user_a1 };
  await resourceRow(resource.resource_id).getByRole("button", { name: "Contenu", exact: true }).click();
  await heading("Session · Contenu autorisé").waitFor();
  await page.locator(".human-event-panel .human-event-row").first().waitFor();

  recoveryStage = "export_owner_download";
  const wire = [];
  let downloads = 0;
  page.on("download", () => { downloads++; });
  const observeExport = (answer) => { if (isReply(answer, exportPath)) wire.push(answer); };
  page.on("response", observeExport);
  const [download] = await Promise.all([page.waitForEvent("download"), button("Exporter le contenu").click()]);
  page.off("response", observeExport);
  await page.getByText(/^Sélection autorisée entièrement reçue/).waitFor();
  assert.match(download.suggestedFilename(), new RegExp(`^ai-guard-session-${resource.resource_id}-content-\\d{4}-\\d{2}-\\d{2}\\.json$`));
  const path = await download.path();
  assert.ok((await stat(path)).size <= 4 * 1024 * 1024);
  const bytes = await readFile(path);
  for (const token of secrets) assert.ok(!bytes.includes(token));
  const exported = JSON.parse(bytes.toString("utf8"));
  assert.equal(exported.format, "ai-guard-human-export-v1");
  assert.equal(exported.coverage, "authorized_selection");
  assert.equal(exported.selection_complete, true);
  assert.equal(exported.authorization_version, selected.authorization_version);
  assert.deepEqual(exported.selection, { view: "content", resource, tenant_id: tenant, organization_scope: selected.organization_scope, mode: "personal" });

  recoveryStage = "export_owner_page_chain";
  assert.equal(wire.length, 2, "la session dense dépasse une page d’export de 25 événements");
  const pages = [];
  let cursor = null;
  for (const [index, answer] of wire.entries()) {
    const url = new URL(answer.url());
    assert.deepEqual(Object.fromEntries(url.searchParams), { limit: "25", view: "content",
      resource_kind: resource.kind, resource_id: resource.resource_id, ...(cursor === null ? {} : { cursor }) });
    assert.equal(await answer.request().headerValue("X-AiGuard-Context"), selected.id);
    assert.equal(answer.status(), 200);
    const body = await answer.json();
    const last = index === wire.length - 1;
    assert.equal(body.context_id, selected.id);
    assert.equal(body.authorization_version, selected.authorization_version);
    assert.equal(body.view, "content");
    assert.equal(body.coverage, "authorized_selection");
    assert.equal(body.selection_complete, last);
    assert.equal(body.has_more, !last);
    assert.ok(last ? body.next_cursor === null : typeof body.next_cursor === "string" && body.next_cursor.length > 0);
    cursor = body.next_cursor;
    pages.push({ items: body.items, read_receipt: body.read_receipt });
  }
  assert.deepEqual(exported.pages, pages);
  assert.equal(pages[0].items.length, 25);
  const events = pages.flatMap(({ items }) => items);
  assert.ok(events.length > 25 && new Set(events.map((item) => item.metadata.id)).size === events.length);
  for (const item of events) assert.deepEqual(item.metadata.resource, resource);
  const example = events.find((item) => item.content?.data?.label === "supervision-test-0");
  assert.deepEqual(example?.content.command.argv, ["/usr/bin/printf", "step-0"]);
  assert.equal(example.metadata.source, "adapter");
  assert.equal(example.metadata.provenance, "observed");
  assert.equal(example.metadata.coverage, "native");

  recoveryStage = "export_owner_receipts";
  const audit = await command({ command: "read_export_receipts",
    receipts: pages.map(({ read_receipt: { audit_event_id, request_id } }) => ({ audit_event_id, request_id })) });
  assert.equal(audit.chain_valid, true);
  assert.equal(audit.receipts.length, pages.length);
  let previousSequence = 0;
  for (const [index, receipt] of audit.receipts.entries()) {
    const { items, read_receipt: wireReceipt } = pages[index];
    const { audit_event_id, request_id, received_at, tenant_id, mode, source, provenance, coverage, action, result_count } = receipt;
    assert.deepEqual({ audit_event_id, request_id, received_at }, wireReceipt);
    assert.deepEqual({ tenant_id, mode, source, provenance, coverage, action, result_count },
      { tenant_id: tenant, mode: "personal", source: "gateway", provenance: "observed", coverage: "observed", action: "export", result_count: items.length });
    assert.deepEqual(receipt.principal, principal);
    assert.ok(Number.isSafeInteger(receipt.sequence) && receipt.sequence > previousSequence);
    previousSequence = receipt.sequence;
  }
  await logout();

  recoveryStage = "export_metadata_only_denial";
  const reader = await openPersonal("user_a2", "Supervision test user A2");
  assert.deepEqual(reader.selected.capabilities, ["read_metadata"]);
  const shared = resourceRow(a.session_shared_with_user_a2);
  assert.equal(await shared.getByRole("button", { name: "Contenu", exact: true }).isDisabled(), true);
  await shared.getByRole("button", { name: "Métadonnées", exact: true }).click();
  await heading("Session · Métadonnées").waitFor();
  await page.locator(".human-event-panel .human-event-row").first().waitFor();
  await page.getByText("Le contrôle n’accorde pas le droit d’export pour cette ressource.", { exact: true }).waitFor();
  assert.equal(await button("Exporter les métadonnées").isDisabled(), true);
  assert.equal(requests.filter((request) => request.startsWith(exportPath + "?")).length, wire.length);
  const query = new URLSearchParams({ limit: "25", view: "metadata", resource_kind: "session", resource_id: a.session_shared_with_user_a2 });
  const denied = await page.evaluate(async ({ url, token, contextId }) => {
    const response = await fetch(url, { headers: { Authorization: `Bearer ${token}`, "X-AiGuard-Context": contextId },
      cache: "no-store", credentials: "omit", redirect: "error", signal: AbortSignal.timeout(8000) });
    const body = await response.json();
    return { status: response.status, code: body.error?.code, hasPage: Object.hasOwn(body, "items") || Object.hasOwn(body, "read_receipt") };
  }, { url: `${exportPath}?${query}`, token: accounts.user_a2, contextId: reader.selected.id });
  assert.deepEqual(denied, { status: 403, code: "permission_denied", hasPage: false });
  assert.equal(downloads, 1);
  await logout();
  await noSecret("export nominatif");
  assert.equal(guard.violations.length, 0);
  console.log(JSON.stringify({ mode: "human_export_rust_sqlite", tenant, context_mode: "personal",
    pages: pages.length, events: events.length, sqlite_receipts: audit.receipts.length, metadata_only_status: denied.status,
    export_sha256: createHash("sha256").update(bytes).digest("hex"), browser: browser.version() }));
  recoveryStage = "complete";
};

/** The resources read fails alone, recovers, then loses its grant while its last page is stale. */
const runResourceRecovery = async () => {
  const guard = await prepareRecoveryGuard();
  const resourcesPath = "/api/v1/human/resources?limit=100";
  const resourcesUrl = recoveryOrigin + resourcesPath;
  const registryAnswer = () => page.waitForResponse((response) => response.request().method() === "GET" && response.url() === recoveryOrigin + fleetUrl);
  const resourcesAnswer = () => page.waitForResponse((response) => response.request().method() === "GET" && response.url() === resourcesUrl);
  const resourcesPanel = page.locator(".sv-panel", { has: heading("Ressources autorisées") });
  const resourceIds = () => resourcesPanel.locator("tbody tr > td:nth-child(2) .sv-id").allTextContents();
  const resourcePresentation = source("Ressources");
  const staleNotice = resourcePresentation.getByText("Contrôle injoignable · dernière lecture conservée", { exact: true });
  const readTime = resourcePresentation.getByText(/^Lu le /);
  const sentinel = "agent-planner";
  const notice = page.locator(".sv-errors [role=alert]");
  const dialog = page.getByRole("dialog");

  recoveryStage = "initial_read";
  await login("admin_a");
  const [loaded, fleet] = await Promise.all([resourcesAnswer(), registryAnswer(), supervise("administration", "tenant-a")]);
  assert.equal(loaded.status(), 200);
  assert.equal(fleet.status(), 200);
  await ready100();
  await page.getByLabel("Rythme d’actualisation").selectOption("0");
  const initialIds = await resourceIds();
  assert.ok(initialIds.includes(sentinel) && initialIds.length > 1);

  async function loseResources() {
    const lastRead = await readTime.textContent();
    const before = requests.length;
    // The fault applies to one GET on the fixture origin; the registry request still reaches Rust.
    await page.route(resourcesUrl, (route) => route.abort("connectionrefused"));
    const [, registry] = await Promise.all([
      page.waitForEvent("requestfailed", (request) => request.url() === resourcesUrl && request.method() === "GET"),
      registryAnswer(), button("Actualiser maintenant").click(),
    ]);
    assert.equal(registry.status(), 200);
    await staleNotice.waitFor();
    await ready100(["Workers", "Brokers"]);
    await notice.getByRole("button", { name: "Réessayer", exact: true }).waitFor();
    assert.deepEqual(humanReads(before).sort(), [fleetUrl, resourcesPath].sort());
    assert.deepEqual(await resourceIds(), initialIds, "la fenêtre de ressources réussie reste disponible seule en état périmé");
    assert.equal(await readTime.textContent(), lastRead, "la date de dernière lecture réussie est conservée");
    await page.getByText(`Ressources : ${initialIds.length} lignes de la dernière lecture réussie, périmées.`, { exact: true }).waitFor();
  }

  recoveryStage = "resources_stale";
  await loseResources();
  recoveryStage = "resources_recovered";
  await page.unroute(resourcesUrl);
  const [recovered, healthy] = await Promise.all([
    resourcesAnswer(), registryAnswer(), notice.getByRole("button", { name: "Réessayer", exact: true }).click(),
  ]);
  assert.equal(recovered.status(), 200);
  assert.equal(healthy.status(), 200);
  await ready100();
  assert.deepEqual(await resourceIds(), initialIds);
  assert.equal(await staleNotice.count(), 0);
  assert.equal(await notice.count(), 0);

  recoveryStage = "withdrawal_while_stale";
  await loseResources();
  await resourcesPanel.locator("tbody tr").filter({ has: page.getByText(sentinel, { exact: true }) })
    .getByRole("button", { name: "Détails", exact: true }).click();
  await dialog.getByText(sentinel, { exact: true }).waitFor();
  await command({ command: "set_capabilities", account: "admin_a", capabilities: ["read_broker_registry", "read_worker_registry"] });
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await heading("Choisir un contexte.").waitFor();
  await page.getByText("Les accès ont changé. Choisissez à nouveau un contexte autorisé.", { exact: true }).waitFor();
  await purged("retrait des ressources pendant la panne", [sentinel]);
  assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0);

  recoveryStage = "reduced_scope";
  const beforeReduced = requests.length;
  const [registry] = await Promise.all([registryAnswer(), supervise("administration", "tenant-a")]);
  assert.equal(registry.status(), 200);
  await ready100(["Workers", "Brokers"]);
  await resourcePresentation.getByText("Non accordé", { exact: true }).waitFor();
  await page.getByText("Ressources : accès non accordé à ce contexte, aucune lecture émise.", { exact: true }).waitFor();
  assert.deepEqual(humanReads(beforeReduced), [fleetUrl]);
  assert.equal(await workerRows().count(), workers);
  assert.deepEqual(await resourceIds(), []);
  assert.equal(await page.getByText(sentinel, { exact: true }).count(), 0);
  assert.equal(await staleNotice.count(), 0);
  assert.equal(await notice.count(), 0);
  await page.unroute(resourcesUrl);
  await logout();
  await noSecret("reprise et retrait de la source ressources");
  assert.equal(guard.violations.length, 0, "le navigateur reste sur l’origine de la fixture");
  recoveryStage = "complete";
};

/** A session resource is readable even when the worker registry has never supplied a page. */
const runCorrelationRecovery = async () => {
  const guard = await prepareRecoveryGuard();
  const fleet = recoveryOrigin + fleetUrl;
  const resources = recoveryOrigin + "/api/v1/human/resources?limit=100";
  const session = a.session_owned_by_user_a1;
  const resourcesPanel = page.locator(".sv-panel", { has: heading("Ressources autorisées") });
  const row = resourcesPanel.locator("tbody tr").filter({ has: page.getByText(session, { exact: true }) });
  const dialog = page.getByRole("dialog");
  const unknown = "Registre des workers non chargé : correspondance inconnue.";
  const falseEmpty = /^Aucun worker de la fenêtre chargée ne déclare cette session/;

  recoveryStage = "resource_without_registry";
  await login("admin_a");
  await page.route(fleet, (route) => route.abort("connectionrefused"));
  const [loaded] = await Promise.all([
    page.waitForResponse((response) => response.url() === resources && response.request().method() === "GET"),
    page.waitForEvent("requestfailed", (request) => request.url() === fleet && request.method() === "GET"),
    supervise("administration", "tenant-a"),
  ]);
  assert.equal(loaded.status(), 200);
  await ready100(["Ressources"]);
  await heading("Workers inconnus").waitFor();
  await page.getByLabel("Rythme d’actualisation").selectOption("0");
  await row.waitFor();

  recoveryStage = "worker_correlation_unavailable";
  await row.getByRole("button", { name: "Détails", exact: true }).click();
  await dialog.getByText(session, { exact: true }).waitFor();
  assert.equal(await dialog.getByText(falseEmpty).count(), 0);
  assert.equal(await row.getByText(falseEmpty).count(), 0);
  await dialog.getByText(unknown, { exact: true }).waitFor();
  await row.getByText(unknown, { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });

  recoveryStage = "worker_correlation_recovered";
  await page.unroute(fleet);
  const [recovered] = await Promise.all([
    page.waitForResponse((response) => response.url() === fleet && response.request().method() === "GET"),
    page.locator(".sv-errors [role=alert]").getByRole("button", { name: "Réessayer", exact: true }).click(),
  ]);
  assert.equal(recovered.status(), 200);
  const payload = await recovered.json();
  const matches = payload.workers.items.filter((worker) => worker.session_id === session);
  assert.equal(matches.length, 1);
  await ready100();
  await row.getByText("1 worker(s) de la fenêtre chargée déclarent cette session", { exact: true }).waitFor();
  await row.getByRole("button", { name: "Détails", exact: true }).click();
  await dialog.getByRole("button", { name: `Ouvrir le worker ${matches[0].worker_id}`, exact: true }).waitFor();
  assert.equal(await dialog.getByText(unknown, { exact: true }).count(), 0);
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
  await logout();
  await noSecret("reprise de la corrélation workers");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** The real resource stream is cancelled while hidden; identity can fail when the tab becomes visible again. */
const runResourceStreamRecovery = async (lateDenial = false) => {
  const guard = await prepareRecoveryGuard();
  const identityPath = "/api/v1/human/identity";
  const streamPath = "/api/v1/human/stream";
  const resourcesPath = "/api/v1/human/resources";
  const isGet = (request, path) => request.method() === "GET" && new URL(request.url()).pathname === path;
  const response = (path) => page.waitForResponse((answer) => isGet(answer.request(), path));
  const stream = page.locator(".stream-pill");
  const panel = page.locator(".human-resource-panel");
  const rows = () => panel.locator(".human-resource-row .control-id").allTextContents();
  const accessWarning = page.getByRole("alert").filter({ hasText: /^Revalidation des accès impossible\./ });

  recoveryStage = "resource_stream_connected";
  await login("user_a2");
  const [initial] = await Promise.all([response(streamPath), button("Ouvrir personnel · tenant-a").click()]);
  assert.equal(initial.status(), 200);
  await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
  await panel.getByText(a.session_owned_by_user_a2, { exact: true }).waitFor();
  const initialIds = (await rows()).sort();
  assert.deepEqual(initialIds, [a.session_owned_by_user_a2, a.session_shared_with_user_a2].sort());
  assert.equal(await openStreams(), 1);

  let releaseDenial;
  const delayedEvents = (url) => url.origin === recoveryOrigin && url.pathname === "/api/v1/human/events" &&
    url.searchParams.get("resource_id") === a.session_owned_by_user_a2;
  if (lateDenial) {
    recoveryStage = "resource_events_read_pending";
    const gate = new Promise((resolve) => { releaseDenial = resolve; });
    await page.route(delayedEvents, async (route) => {
      await gate;
      await route.fulfill({ status: 403, json: { error: { code: "permission_denied",
        message: "This context does not permit this read.", retry_action: "review_permissions" } } });
    });
    await Promise.all([
      page.waitForRequest((request) => delayedEvents(new URL(request.url())) && request.method() === "GET"),
      panel.locator(".human-resource-row").filter({ has: page.getByText(a.session_owned_by_user_a2, { exact: true }) })
        .getByRole("button", { name: "Métadonnées", exact: true }).click(),
    ]);
    await heading("Événements autorisés.").waitFor();
  }

  recoveryStage = "resource_stream_return_offline";
  await visibility("hidden");
  assert.equal(await openStreams(), 0, "le navigateur a réellement annulé son flux nominatif");
  const beforeDenial = requests.length;
  if (lateDenial) {
    const denied = page.waitForResponse((answer) => delayedEvents(new URL(answer.url())));
    releaseDenial();
    assert.equal((await denied).status(), 403);
    await heading("Ressources autorisées.").waitFor();
    await page.unroute(delayedEvents);
  }
  const hiddenStatus = await stream.innerText();
  const startsBefore = streamStarts();
  await context.setOffline(true);
  const identityFailed = page.waitForEvent("requestfailed", (request) => isGet(request, identityPath));
  await visibility("visible");
  await identityFailed;
  await accessWarning.waitFor();
  assert.equal(await openStreams(), 0);
  assert.equal(streamStarts(), startsBefore);
  const offlineStatus = await stream.innerText();
  if (!lateDenial) assert.equal(offlineStatus, "Flux suspendu · accès à relire");
  assert.equal(hiddenStatus, "Flux suspendu · onglet masqué");
  const stale = panel.getByText(/^(Données à actualiser|Données périmées · dernière lecture conservée)$/);
  if (!lateDenial) await stale.waitFor();
  const retainedStale = await stale.count() === 1;
  assert.deepEqual((await rows()).sort(), initialIds);

  recoveryStage = "resource_stream_revalidated";
  await context.setOffline(false);
  const before = requests.length;
  let beforeRefresh = before;
  if (lateDenial) {
    assert.equal(await page.evaluate(() => window.__resourcePoll()), 1);
    await panel.locator(".human-resource-row").filter({ has: page.getByText(a.session_shared_with_user_a2, { exact: true }) })
      .getByRole("button", { name: "Métadonnées", exact: true }).click();
    await heading("Événements autorisés.").waitFor();
    await button("Ressources").click();
    await heading("Ressources autorisées.").waitFor();
    await settle();
    beforeRefresh = requests.length;
  }
  const refresh = button("Actualiser");
  await refresh.focus();
  const replies = Promise.allSettled([response(identityPath), response(streamPath), response(resourcesPath)]);
  await page.keyboard.press("Enter");
  if (lateDenial) {
    const firstPath = () => requests.slice(before).map((path) => path.split("?")[0])
      .find((path) => [identityPath, resourcesPath, streamPath].includes(path));
    const first = firstPath() ?? new URL((await page.waitForRequest((request) =>
      [identityPath, resourcesPath, streamPath].some((path) => isGet(request, path)))).url()).pathname;
    if (first !== identityPath) recoveryStage = "resource_read_before_access";
    assert.equal(first, identityPath, "aucune lecture des ressources ou du flux avant revalidation des accès");
    assert.equal(offlineStatus, "Flux suspendu · accès à relire");
    assert.equal(retainedStale, true);
    assert.deepEqual(requests.slice(before, beforeRefresh).filter((path) => path.startsWith("/api/v1/human/")), [],
      "le poll et la navigation entre Ressources et Événements n’émettent aucune lecture avant revalidation");
    assert.deepEqual(requests.slice(beforeDenial, before).filter((path) => path.split("?")[0] === resourcesPath), [],
      "le refus d’une sélection ne déclenche pas de nouveau GET quand le flux est suspendu");
  }
  for (const reply of await replies) { assert.equal(reply.status, "fulfilled"); assert.equal(reply.value.status(), 200); }
  await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
  await panel.getByText("Lecture réussie", { exact: true }).waitFor();
  assert.equal(await accessWarning.count(), 0);
  assert.deepEqual((await rows()).sort(), initialIds);
  assert.equal(streamStarts(), startsBefore + 1);
  const after = requests.slice(before).map((path) => path.split("?")[0]);
  assert.equal(after.filter((path) => path === identityPath).length, 1);
  assert.ok(after.indexOf(identityPath) < after.indexOf(streamPath), "le flux est repris après relecture des accès");
  assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Actualiser");
  await logout();
  assert.equal(await openStreams(), 0);
  await noSecret("reprise du flux nominatif");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** A reply sent for an earlier activation cannot reopen the direct flow across another hide. */
const runResourceStreamVisibilityRace = async () => {
  const guard = await prepareRecoveryGuard();
  const identityPath = "/api/v1/human/identity";
  const eventsPath = "/api/v1/human/events";
  const streamPath = "/api/v1/human/stream";
  const isReply = (answer, path) => answer.request().method() === "GET" && new URL(answer.url()).pathname === path;
  // The client captures fetch at login. Arm the delay only after reaching the event view.
  await page.addInitScript((path) => {
    const fetch = window.fetch;
    const release = [];
    let holding = false;
    window.__heldVisibilityReplies = 0;
    window.__holdVisibilityReplies = () => { holding = true; };
    window.__nextVisibilityReply = () => release.shift()?.();
    window.__clearVisibilityReplies = () => { holding = false; for (const pending of release.splice(0)) pending(); };
    window.fetch = async (input, options) => {
      const url = new URL(input instanceof Request ? input.url : input, window.location.origin);
      const method = options?.method ?? (input instanceof Request ? input.method : "GET");
      const answer = await fetch(input, options);
      if (holding && url.origin === location.origin && url.pathname === path && method === "GET") {
        await new Promise((resolve) => { release.push(resolve); window.__heldVisibilityReplies++; });
      }
      return answer;
    };
  }, identityPath);
  recoveryStage = "resource_visibility_event_ready";
  await login("user_a1");
  await button("Ouvrir personnel · tenant-a").click();
  const row = page.locator(".human-resource-row").filter({ has: page.getByText(a.session_owned_by_user_a1, { exact: true }) });
  const [initial] = await Promise.all([page.waitForResponse((answer) => isReply(answer, eventsPath)),
    row.getByRole("button", { name: "Métadonnées", exact: true }).click()]);
  assert.equal(initial.status(), 200);
  const detail = page.locator(".human-event-row").first().getByRole("button", { name: "Détails", exact: true });
  const exportButton = button("Exporter les métadonnées");
  await detail.waitFor();
  assert.equal(await detail.isEnabled(), true);
  assert.equal(await exportButton.isEnabled(), true);
  await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
  await visibility("hidden");
  assert.equal(await openStreams(), 0);

  await page.evaluate(() => window.__holdVisibilityReplies());
  const before = requests.length;
  const startsBefore = streamStarts();
  try {
    recoveryStage = "resource_visibility_first_read_held";
    const [first] = await Promise.all([page.waitForResponse((answer) => isReply(answer, identityPath)), visibility("visible")]);
    assert.equal(first.status(), 200);
    await page.waitForFunction(() => window.__heldVisibilityReplies === 1);
    await visibility("hidden");
    await visibility("visible");
    await settle();
    assert.equal(identityReads(before), 1);
    assert.deepEqual(humanReads(before), []);

    recoveryStage = "resource_visibility_waiting_current_read";
    const next = page.waitForRequest((request) => new URL(request.url()).pathname.startsWith("/api/v1/human/"));
    await page.evaluate(() => window.__nextVisibilityReply());
    const nextRead = await next;
    const nextPath = new URL(nextRead.url()).pathname;
    assert.equal(nextPath, identityPath, "la réponse de la première activation ne rouvre pas les lectures");
    assert.equal((await nextRead.response()).status(), 200);
    await page.waitForFunction(() => window.__heldVisibilityReplies === 2);
    assert.equal(identityReads(before), 2);
    await page.getByText("Flux suspendu · accès à relire", { exact: true }).waitFor();
    assert.equal(await detail.isDisabled(), true);
    assert.equal(await exportButton.isDisabled(), true);
    assert.deepEqual(humanReads(before), []);

    recoveryStage = "resource_visibility_current_read_accepted";
    const replies = Promise.all([page.waitForResponse((answer) => isReply(answer, streamPath)),
      page.waitForResponse((answer) => isReply(answer, eventsPath))]);
    await page.evaluate(() => window.__nextVisibilityReply());
    for (const answer of await replies) assert.equal(answer.status(), 200);
    await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();
    await page.locator(".human-event-panel").getByText("Lecture réussie", { exact: true }).waitFor();
    assert.equal(await detail.isEnabled(), true);
    assert.equal(await exportButton.isEnabled(), true);
    assert.equal(streamStarts(), startsBefore + 1);
  } finally {
    await page.evaluate(() => window.__clearVisibilityReplies());
  }
  await logout();
  assert.equal(await openStreams(), 0);
  await noSecret("reprise du flux après deux activations");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** HTTP 200 opens the stream. A delivered batch or a new read establishes how current the rows are. */
const runJournalCatchUp = async () => {
  const guard = await prepareRecoveryGuard();
  const streamPath = "/api/v1/human/stream";
  const recentPath = "/api/v1/human/events/recent";
  const pathIs = (request, path) => request.method() === "GET" && new URL(request.url()).pathname === path;
  const reply = (path) => page.waitForResponse((answer) => pathIs(answer.request(), path));
  const journalRead = page.locator(".sv-journal .sv-source", { hasText: "Journal" });
  const stale = journalRead.getByText("Données périmées · dernière lecture conservée", { exact: true });
  const fresh = journalRead.getByText("Lecture réussie", { exact: true });
  const reload = button("Relire depuis les plus récents");
  const table = page.locator(".sv-journal [data-event-id]");
  const eventIds = () => table.evaluateAll((rows) => rows.map((row) => row.dataset.eventId));

  // Cancel only the armed request, once, in the browser. Every completed response and SSE event is real.
  await page.addInitScript((path) => {
    const fetch = window.fetch;
    let armed = false;
    window.__armJournalStream = () => { armed = true; };
    window.fetch = (input, options) => {
      const url = new URL(input instanceof Request ? input.url : input, location.origin);
      if (!armed || url.origin !== location.origin || url.pathname !== path || options?.method !== "GET") return fetch(input, options);
      armed = false;
      const interruption = new AbortController();
      window.__interruptJournalStream = () => interruption.abort();
      return fetch(input, { ...options, signal: AbortSignal.any([options.signal, interruption.signal]) });
    };
  }, streamPath);

  recoveryStage = "journal_live_before_disconnect";
  await login("user_a1"); await supervise("personnel", "tenant-a"); await ready100(["Ressources"]);
  await page.evaluate(() => window.__armJournalStream());
  const [initial] = await Promise.all([reply(streamPath), button("Journal").click()]);
  assert.equal(initial.status(), 200);
  await direct("Direct connecté").waitFor(); await fresh.waitFor(); await table.first().waitFor();
  const cached = await eventIds();
  assert.ok(cached.length > 0 && cached.length <= 100);

  const reconnect = async (armNext) => {
    if (armNext) await page.evaluate(() => window.__armJournalStream());
    const reconnected = reply(streamPath);
    const closed = page.waitForEvent("requestfailed", (request) => pathIs(request, streamPath));
    await page.evaluate(() => window.__interruptJournalStream());
    await closed;
    await direct("Direct déconnecté · reconnexion").waitFor(); await stale.waitFor();
    assert.equal((await reconnected).status(), 200);
    await direct("Direct connecté").waitFor();
  };
  recoveryStage = "journal_reconnected_without_catch_up";
  const before = requests.length;
  await reconnect(true);
  assert.equal(await stale.count(), 1, "l’ouverture HTTP du flux ne prouve pas la fraîcheur du Journal");
  assert.equal(await fresh.count(), 0);
  assert.deepEqual(await eventIds(), cached);
  assert.equal(await reload.isEnabled(), true);
  assert.deepEqual(humanReads(before).map((path) => path.split("?")[0]), [streamPath]);

  recoveryStage = "journal_catch_up_delivered";
  await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 3 });
  await page.getByText("3 événement(s) reçu(s) depuis la dernière lecture", { exact: true }).waitFor();
  await fresh.waitFor();
  const after = await eventIds();
  assert.equal(new Set(after).size, cached.length + 3);
  assert.deepEqual(after.slice(3), cached);

  recoveryStage = "journal_quiet_stream_manual_reload";
  await reconnect(false);
  assert.equal(await stale.count(), 1);
  const beforeReload = requests.length;
  await reload.focus();
  const refreshed = Promise.all([reply(recentPath), reply(streamPath)]);
  await page.keyboard.press("Enter");
  for (const answer of await refreshed) assert.equal(answer.status(), 200);
  await fresh.waitFor(); await direct("Direct connecté").waitFor();
  assert.deepEqual(await eventIds(), after.slice(0, 100));
  assert.deepEqual(humanReads(beforeReload).map((path) => path.split("?")[0]), [recentPath, streamPath]);
  assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Relire depuis les plus récents");

  await logout();
  assert.equal(await openStreams(), 0);
  await noSecret("rattrapage du Journal");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** Only the older page failed. Retry its exact position while the established direct keeps adding events. */
const runJournalOlderRecovery = async () => {
  const guard = await prepareRecoveryGuard();
  const recentPath = "/api/v1/human/events/recent";
  const sameRead = (url) => url.origin === recoveryOrigin && url.pathname === recentPath;
  const olderRead = (url) => sameRead(url) && url.searchParams.has("cursor");
  const source = page.locator(".sv-journal .sv-source", { hasText: "Journal" });
  const rows = page.locator(".sv-journal [data-event-id]");
  const rowIds = () => rows.evaluateAll((nodes) => nodes.map((row) => row.dataset.eventId));
  const retry = page.locator(".sv-journal .sv-errors").getByRole("button", { name: /^Réessayer/ });
  const alert = page.locator(".sv-journal .sv-errors [role=alert]");

  // Hold the next real read at the client boundary to inspect focus while its answer is pending.
  await page.addInitScript((path) => {
    const fetch = window.fetch;
    let hold = false;
    window.__holdOlderReply = () => { hold = true; };
    window.fetch = async (input, options) => {
      const url = new URL(input instanceof Request ? input.url : input, location.origin);
      if (!hold || url.origin !== location.origin || url.pathname !== path) return fetch(input, options);
      hold = false;
      const gate = new Promise((resolve) => { window.__releaseOlderReply = resolve; });
      const reply = await fetch(input, options);
      await gate;
      return reply;
    };
  }, recentPath);

  recoveryStage = "journal_older_two_pages";
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await button("Journal").click(); await direct("Direct connecté").waitFor();
  await journalResult(/^100 événements affichés sur 100 chargés/).waitFor();
  await button("Charger 100 événements plus anciens").click();
  await journalResult(/^200 événements affichés sur 200 chargés/).waitFor();
  const cached = await rowIds();
  assert.equal(new Set(cached).size, 200);
  const streams = streamStarts();

  recoveryStage = "journal_older_page_unavailable";
  await page.route(olderRead, (route) => route.fulfill({ status: 503, json: { error: {
    code: "identity_unavailable", message: "The cursor control is unavailable.", retry_action: "retry",
  } } }));
  const [failed] = await Promise.all([page.waitForResponse((answer) => olderRead(new URL(answer.url()))),
    button("Charger 100 événements plus anciens").click()]);
  assert.equal(failed.status(), 503);
  await page.unroute(olderRead);
  await alert.waitFor();
  assert.deepEqual(await rowIds(), cached);
  await direct("Direct connecté").waitFor();
  const appended = (await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 1 })).event_ids;
  await journalResult(/^201 événements affichés sur 201 chargés/).waitFor();
  assert.deepEqual(await rowIds(), [...appended, ...cached].slice(0, 200));
  await alert.waitFor();
  const failureNamesOlderPage = (await alert.innerText()).includes("Page plus ancienne indisponible");
  const statusNamesOlderPage = await source.getByText("Page plus ancienne indisponible", { exact: true }).count() === 1;

  recoveryStage = "journal_older_retry_position";
  await page.evaluate(() => window.__holdOlderReply());
  const before = requests.length;
  try {
    const attempted = page.waitForRequest((request) => sameRead(new URL(request.url())));
    await retry.focus(); await page.keyboard.press("Enter");
    const request = await attempted;
    if (!olderRead(new URL(request.url()))) recoveryStage = "journal_older_retry_without_cursor";
    assert.ok(request.url() === failed.url(), "la reprise utilise la position opaque de la page refusée");
    recoveryStage = "journal_older_retry_focus_and_status";
    assert.equal(await retry.getAttribute("aria-disabled"), "true");
    assert.equal(await retry.evaluate((node) => node === document.activeElement), true);
    assert.equal(failureNamesOlderPage, true);
    assert.equal(statusNamesOlderPage, true);
    assert.equal((await request.response())?.status(), 200);
  } finally { await page.evaluate(() => window.__releaseOlderReply?.()); }
  recoveryStage = "journal_older_recovered";
  await journalResult(/^301 événements affichés sur 301 chargés/).waitFor();
  assert.deepEqual(await rowIds(), [...appended, ...cached].slice(0, 200));
  assert.equal(await alert.count(), 0);
  await direct("Direct connecté").waitFor();
  assert.equal(streamStarts(), streams);
  assert.deepEqual(humanReads(before).map((path) => path.split("?")[0]), [recentPath]);
  assert.equal(await page.evaluate(() => document.activeElement === document.body), false);
  await logout();
  assert.equal(await openStreams(), 0);
  await noSecret("reprise de la page plus ancienne");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** Worker -> declared session -> authorized resource -> real events, and back; refusals stay local. */
const runCorrelation = async () => {
  const resourcesPanel = page.locator(".sv-panel", { has: heading("Ressources autorisées") });
  const dialog = page.getByRole("dialog");
  const search = page.getByLabel("Rechercher dans les lignes chargées");
  const eventsUrl = (session) => `/api/v1/human/events/recent?limit=25&view=metadata&resource_kind=session&resource_id=${session}`;
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await button("Charger 100 lignes de plus").click();
  await page.getByText("120 workers affichés sur 120 chargés · fin du registre à cette lecture", { exact: true }).waitFor();

  // A worker whose declared session equals a granted session resource, by exact identifier.
  await search.fill(a.session_owned_by_user_a1);
  assert.equal(await workerRows().count(), 1, "une seule ligne déclare cette session");
  await workerRows().getByText("Ressource autorisée correspondante", { exact: true }).waitFor();
  await workerRows().getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await dialog.getByText(/Ressource autorisée de même identifiant exact/).waitFor();
  assert.equal(await dialog.locator(".sv-events li").count(), 0, "aucun événement n’est lu sans action explicite");
  let before = requests.length;
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".sv-events li").first().waitFor();
  assert.deepEqual(humanReads(before), [eventsUrl(a.session_owned_by_user_a1)], "une lecture serveur filtrée sur cette session exacte");
  const listed = await dialog.locator(".sv-events li").count();
  assert.ok(listed > 0 && listed <= 25);
  await dialog.getByText(/du plus récent au plus ancien · d’autres, plus anciens, existent/).waitFor();
  await dialog.getByText(/provenance (observed|declared|inferred)/).first().waitFor();
  await dialog.getByRole("button", { name: "Relire les événements de cette session", exact: true }).waitFor();
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });

  // A declared session that no resource grant covers: the refusal is shown in the detail only.
  await search.fill(a.session_without_grant);
  assert.equal(await workerRows().count(), 1);
  assert.equal(await workerRows().getByText("Ressource autorisée correspondante", { exact: true }).count(), 0);
  await workerRows().getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await dialog.getByText(/Aucune ressource de session de même identifiant parmi les \d+ ressources chargées/).waitFor();
  before = requests.length;
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  // The visible refusal; the announcement region of the detail says the same sentence to assistive technology.
  await dialog.locator(".detail-body").getByText(/Aucune ressource autorisée de ce contexte ne correspond à cette session déclarée/).waitFor();
  assert.match(await dialog.getByRole("status").innerText(), /^Aucune ressource autorisée de ce contexte ne correspond à cette session déclarée/);
  assert.deepEqual(humanReads(before), [eventsUrl(a.session_without_grant)]);
  assert.equal(await dialog.locator(".sv-events li").count(), 0, "un refus n’affiche aucun événement");
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  await source("Workers").getByText("Lecture réussie", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 1, "le refus d’une session ne retire pas la flotte");
  await button("Réinitialiser").click(); assert.equal(await workerRows().count(), 120);

  // Resource -> workers of the loaded window that declare it -> worker detail.
  await page.getByLabel("Type", { exact: true }).selectOption("session");
  const sessions = await resourcesPanel.locator("tbody tr").count();
  assert.equal(sessions, a.granted_sessions.length, "chaque session accordée de l’organisation est listée pour l’administration");
  await page.getByLabel("Rechercher dans les ressources chargées").fill(a.session_owned_by_user_a1);
  assert.equal(await resourcesPanel.locator("tbody tr").count(), 1);
  await resourcesPanel.getByText("1 worker(s) de la fenêtre chargée déclarent cette session", { exact: true }).waitFor();
  await resourcesPanel.getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await dialog.getByText("Workers déclarant cette session", { exact: true }).waitFor();
  await dialog.getByRole("button", { name: /^Ouvrir le worker / }).click();
  await dialog.getByText("Autorité du lancement", { exact: true }).waitFor();
  assert.equal(await page.locator("dialog[open]").count(), 1, "un seul détail ouvert à la fois");
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  await page.getByLabel("Rechercher dans les ressources chargées").fill("agent-planner");
  await page.getByLabel("Type", { exact: true }).selectOption("all");
  await resourcesPanel.getByText("Relation agent / session / sous-agent non fournie par le contrat nominatif · inconnue", { exact: true }).waitFor();
  await resourcesPanel.getByRole("button", { name: "Détails", exact: true }).click();
  await dialog.getByText(/non fournie par le contrat nominatif, donc inconnue/).waitFor();
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  let text = await bodyText();
  for (const hidden of ["sandbox-retired", "project-elsewhere"]) assert.ok(!text.includes(hidden), "une ressource inactive ou d’une autre organisation n’est pas listée");
  await page.getByText("Accès de ce contexte", { exact: true }).click();
  await page.getByText("Accordé par read_worker_registry : états du contrôle, dernière observation runtime et associations d’autorité.", { exact: true }).waitFor();
  await geometry("correlation", [1440, 390], { also: [["resources", ".sv-toolbar-short"]] });
  await logout();

  // A registry-only context holds no event authority: the detail says so and sends nothing.
  await login("workers_only_a"); await supervise("administration", "tenant-a"); await ready100(["Workers"]);
  await workerRows().first().getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await dialog.getByText("Les ressources ne sont pas lues dans ce contexte : aucune correspondance n’est recherchée.", { exact: true }).waitFor();
  before = requests.length;
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".detail-body").getByText("Ce contexte ne porte aucune capacité de métadonnées : les événements de session ne sont pas lus.", { exact: true }).waitFor();
  assert.deepEqual(humanReads(before), [], "aucune requête d’événements sans capacité de métadonnées");
  await page.keyboard.press("Escape"); await logout();

  // A personal context with a metadata share reads the shared session and nothing of the registries.
  await login("user_a2"); before = requests.length;
  await supervise("personnel", "tenant-a"); await ready100(["Ressources"]);
  assert.equal(await resourcesPanel.locator("tbody tr").count(), 2, "la session possédée et la session partagée, rien d’autre");
  text = await bodyText();
  assert.ok(text.includes(a.session_shared_with_user_a2) && text.includes(a.session_owned_by_user_a2) && !text.includes(a.session_owned_by_user_a1));
  await page.getByLabel("Rechercher dans les ressources chargées").fill(a.session_shared_with_user_a2);
  await resourcesPanel.getByText("Registre des workers non accordé", { exact: true }).waitFor();
  await resourcesPanel.getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".sv-events li").first().waitFor();
  assert.deepEqual(humanReads(before), ["/api/v1/human/resources?limit=100", eventsUrl(a.session_shared_with_user_a2)]);
  await page.keyboard.press("Escape"); await logout();
  await noSecret("corrélation");
};

const purged = async (label, identifiers) => {
  assert.equal(await page.locator(".sv, dialog").count(), 0, `${label} : la vue et ses détails sont retirés`);
  const text = await bodyText();
  for (const id of identifiers) assert.ok(!text.includes(id), `${label} : aucun identifiant retiré ne reste affiché`);
};
const full = ["read_metadata", "read_content", "export", "read_tenant_metadata", "read_broker_registry", "read_worker_registry"];
const reduced = ["read_metadata", "read_tenant_metadata", "read_broker_registry"];

/** Rights re-evaluated by the control: a reduced membership, then a revoked credential. */
const runRights = async () => {
  const dialog = page.getByRole("dialog");
  await login("admin_a");
  assert.deepEqual(await tabs(), ["Contextes", "Ressources", "Événements"]);
  await geometry("rights-3", [390, 320], { shots: false });
  await supervise("administration", "tenant-a"); await ready100();
  assert.equal(await workerRows().count(), 24);
  await workerRows().first().getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await command({ command: "set_capabilities", account: "admin_a", capabilities: reduced });
  // Returning to the tab revalidates the identity: the selected context is no longer offered.
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await heading("Choisir un contexte.").waitFor();
  await page.getByText("Les accès ont changé. Choisissez à nouveau un contexte autorisé.", { exact: true }).waitFor();
  await purged("droits réduits", sortedWorkersA);
  assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0, "un changement de droits n’offre aucune reprise");
  await page.getByText(/Capacités serveur : .*read_broker_registry/).first().waitFor();
  assert.equal(await page.getByText(/Capacités serveur : .*read_worker_registry/).count(), 0, "le contexte reproposé ne porte plus le registre des workers");
  let before = requests.length;
  await supervise("administration", "tenant-a"); await ready100(["Brokers", "Ressources"]);
  assert.deepEqual(humanReads(before).sort(), ["/api/v1/human/brokers?limit=100", "/api/v1/human/resources?limit=100"], "la source retirée n’est plus demandée");
  await page.getByText("Workers : registre non accordé à ce contexte, aucune lecture émise.", { exact: true }).waitFor();
  assert.equal(await heading("Workers").count(), 0);
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Journal", "Ressources", "Événements", "Brokers"]);
  await geometry("rights-6", [390, 320], { shots: false });

  // Both registries without any metadata capability: seven destinations.
  await command({ command: "set_capabilities", account: "admin_a", capabilities: ["read_broker_registry", "read_worker_registry"] });
  await button("Contextes").click(); await heading("Choisir un contexte.").waitFor();
  await page.getByText(/Capacités serveur : .*read_worker_registry/).first().waitFor();
  await supervise("administration", "tenant-a"); await ready100(["Workers", "Brokers"]);
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Ressources", "Événements", "Brokers", "Workers", "Fédération"]);
  await geometry("rights-7", [390, 320], { shots: false });

  // The same reduction met by a read in flight: nothing of the refused registry stays on screen.
  await command({ command: "set_capabilities", account: "admin_a", capabilities: full });
  await button("Contextes").click(); await heading("Choisir un contexte.").waitFor();
  await page.getByText(/Capacités serveur : .*read_worker_registry/).first().waitFor();
  await supervise("administration", "tenant-a"); await ready100(); assert.equal(await workerRows().count(), 24);
  await command({ command: "set_capabilities", account: "admin_a", capabilities: reduced });
  // Order made explicit. The composed read is refused first, without a readable reason, and makes the view
  // read the identity; the refusal of the resources, which names the lost context, lands during that read
  // and aborts it. The list of contexts must still be read, once.
  let releaseResources; const resourcesGate = new Promise((resolve) => { releaseResources = resolve; }); const heldResources = [];
  await page.route("**/api/v1/human/resources**", (route) => { heldResources.push(resourcesGate.then(() => route.continue())); });
  let identityRoutes = 0;
  await page.route("**/api/v1/human/identity", (route) => identityRoutes++ ? route.continue() : undefined);
  const identityAsked = page.waitForRequest((request) => new URL(request.url()).pathname === "/api/v1/human/identity");
  before = requests.length;
  await button("Actualiser maintenant").click();
  await identityAsked; releaseResources(); await Promise.all(heldResources);
  await heading("Choisir un contexte.").waitFor();
  for (let attempt = 0; attempt < 50 && !(await page.getByText(/Capacités serveur : /).count()); attempt++) await page.waitForTimeout(100);
  assert.ok(await page.getByText(/Capacités serveur : /).count() > 0, "un contexte perdu pendant la relecture d’identité n’empêche pas de relire la liste des contextes");
  assert.equal(identityReads(before), 2, "la lecture d’identité abandonnée, puis une seule relecture");
  await page.unroute("**/api/v1/human/resources**"); await page.unroute("**/api/v1/human/identity");
  await purged("lecture refusée", sortedWorkersA);

  // A revoked credential ends the session on the next read.
  await supervise("administration", "tenant-a"); await ready100(["Brokers", "Ressources"]);
  await command({ command: "revoke_credential", account: "admin_a" });
  await button("Actualiser maintenant").click();
  await page.getByLabel("Credential personnel local").waitFor();
  await purged("credential révoqué", Object.values(a.brokers));
  assert.equal(await page.locator(".human-shell").count(), 0);
  assert.equal(await page.getByLabel("Credential personnel local").inputValue(), "");
  await noSecret("droits");
};

/** Data of one tenant or context never survives into another one, including a late response. */
const runIsolation = async () => {
  const guard = await prepareRecoveryGuard();
  recoveryStage = "isolation_context_switch";
  const search = page.getByLabel("Rechercher dans les lignes chargées");
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await search.fill("typed-in-administration");
  // Same account, another context: rows and typed filters of the first one are gone.
  await button("Contextes").click(); await heading("Choisir un contexte.").waitFor();
  await purged("retour aux contextes", sortedWorkersA);
  await supervise("personnel", "tenant-a"); await heading("Supervision de mon espace.").waitFor(); await ready100(["Ressources"]);
  let text = await bodyText();
  for (const id of [...sortedWorkersA, ...Object.values(a.brokers)]) assert.ok(!text.includes(id), "le contexte personnel n’affiche aucune ligne des registres");
  await button("Contextes").click(); await heading("Choisir un contexte.").waitFor();
  await supervise("administration", "tenant-a"); await ready100();
  assert.equal(await search.inputValue(), "", "un filtre saisi dans un contexte ne survit pas à son remplacement");
  assert.equal(await workerRows().count(), 24);

  // A read of A answered by the control but not yet delivered when A signs out: the page aborts it, so
  // nothing of it can reach the next session. (A late answer that is delivered is covered without a
  // browser by supervision-store.test.mjs, « une réponse tardive après fermeture ne repeuple rien ».)
  recoveryStage = "isolation_hold_a_reply";
  const contextA = contexts.filter(Boolean).at(-1);
  assert.ok(contextA);
  const endpoint = recoveryOrigin + fleetUrl;
  const matchesFleet = (url) => url.href === endpoint;
  const aborted = []; let held = 0;
  let heldRequest, heldStatus, heldPage;
  const failed = (request) => {
    if (new URL(request.url()).pathname === "/api/v1/human/federation/workers")
      aborted.push({ request, reason: request.failure()?.errorText ?? "" });
  };
  page.on("requestfailed", failed);
  await page.route(matchesFleet, async (route) => {
    const request = route.request();
    if (request.method() !== "GET" || request.frame() !== page.mainFrame() ||
        await request.headerValue("X-AiGuard-Context") !== contextA || heldRequest) return route.fallback();
    // The page route runs before the context guard. Relay only this exact origin, path and context.
    heldRequest = request;
    try {
      const reply = await route.fetch({ maxRedirects: 0, maxRetries: 0 });
      heldStatus = reply.status();
      if (reply.url() === endpoint && heldStatus === 200) heldPage = await reply.json();
    } catch { /* The scenario reports a missing successful server reply below. */ }
    held++;
  });
  await button("Actualiser maintenant").click();
  await page.waitForFunction(() => { const node = document.querySelector('[data-focus-key="sv-refresh"]'); return node?.disabled === true || node?.getAttribute("aria-disabled") === "true"; });
  for (let attempt = 0; attempt < 50 && !held; attempt++) await page.waitForTimeout(100);
  assert.equal(held, 1, "la réponse de A est retenue après son retour du contrôle");
  assert.equal(heldStatus, 200);
  assert.equal(heldPage?.workers?.context_id, contextA);
  assert.equal(heldPage.brokers.context_id, contextA);
  assert.deepEqual(heldPage.workers.items.map(({ worker_id }) => worker_id).sort(), sortedWorkersA);
  recoveryStage = "isolation_cancel_a_on_logout";
  await logout();
  for (let attempt = 0; attempt < 30 && !aborted.length; attempt++) await page.waitForTimeout(100);
  assert.deepEqual(aborted.map(({ reason }) => reason), ["net::ERR_ABORTED"], "la déconnexion annule la lecture en vol de A, avant toute connexion de B");
  assert.equal(aborted[0].request, heldRequest);
  page.off("requestfailed", failed); await page.unroute(matchesFleet);
  recoveryStage = "isolation_read_b";
  await login("admin_b");
  const [replyB] = await Promise.all([page.waitForResponse((reply) => reply.url() === endpoint && reply.request().method() === "GET"),
    supervise("administration", "tenant-b")]);
  assert.equal(replyB.status(), 200);
  const pageB = await replyB.json();
  const contextB = await replyB.request().headerValue("X-AiGuard-Context");
  assert.ok(contextB && contextB !== contextA);
  assert.equal(pageB.workers.context_id, contextB);
  assert.equal(pageB.brokers.context_id, contextB);
  assert.deepEqual(pageB.workers.items.map(({ worker_id }) => worker_id), b.workers);
  await ready100();
  assert.equal(await workerRows().count(), 1, "seule la ligne de B est affichée");
  text = await bodyText();
  assert.ok(text.includes(b.workers[0]));
  for (const id of [...sortedWorkersA, ...Object.values(a.brokers), a.session_owned_by_user_a1]) assert.ok(!text.includes(id), "aucune donnée de A après la connexion de B");
  assert.ok(requests.every((path) => !/tenant/i.test(path)), "aucune requête ne choisit un tenant");
  const used = new Set(contexts.filter(Boolean));
  assert.ok(used.size >= 3, "chaque contexte sélectionné a son propre identifiant opaque");
  await logout(); await noSecret("isolation");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/**
 * Rights reduced or a credential revoked while nobody acts: the automatic cycle of the unified view and the
 * live stream of the Journal are the first to meet the control's refusal.
 */
const runRevoked = async () => {
  const resourcesPanel = page.locator(".sv-panel", { has: heading("Ressources autorisées") });
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await page.getByLabel("Rechercher dans les ressources chargées").fill(a.session_owned_by_user_a1);
  await resourcesPanel.getByRole("button", { name: "Détails", exact: true }).click(); await page.getByRole("dialog").waitFor();
  await command({ command: "set_capabilities", account: "admin_a", capabilities: reduced });
  // The composed read answers 403 context_revoked; the client publishes that refusal without reading its reason.
  const refusal = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/federation/workers" && response.status() === 403);
  let before = requests.length;
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1, "le cycle automatique de la flotte est armé");
  await refusal;
  for (let attempt = 0; attempt < 40 && await page.locator(".sv, dialog").count(); attempt++) await page.waitForTimeout(100);
  assert.equal(await page.locator(".sv, dialog").count(), 0, "sans aucun clic, un contexte révoqué vu par le cycle automatique retire la vue, ses ressources et son détail");
  await heading("Choisir un contexte.").waitFor();
  await page.getByText("Les accès ont changé. Choisissez à nouveau un contexte autorisé.", { exact: true }).waitFor();
  await purged("cycle automatique refusé", [...sortedWorkersA, a.session_owned_by_user_a1]);
  assert.deepEqual(humanReads(before), [fleetUrl], "le cycle n’a lu que la flotte");
  assert.equal(identityReads(before), 1, "une seule relecture d’identité tranche");
  assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0, "un changement de droits n’offre aucune reprise");

  // The same reduction met by the live stream of the Journal.
  await command({ command: "set_capabilities", account: "admin_a", capabilities: full });
  await button("Actualiser").click(); await page.getByText(/Capacités serveur : .*read_worker_registry/).first().waitFor();
  await supervise("administration", "tenant-a"); await ready100();
  await button("Journal").click(); await direct("Direct connecté").waitFor();
  assert.ok(await page.locator("[data-event-id]").count() > 0);
  const streams = streamStarts();
  await command({ command: "set_capabilities", account: "admin_a", capabilities: reduced });
  await heading("Choisir un contexte.").waitFor({ timeout: 20000 });
  assert.equal(await page.locator(".sv, [data-event-id]").count(), 0, "le direct refusé retire le Journal et ses lignes sans action de l’utilisateur");
  assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0);
  await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 2 });
  await page.waitForTimeout(1500);
  assert.equal(await page.locator("[data-event-id]").count(), 0, "aucun événement ajouté ensuite n’est affiché");
  assert.equal(streamStarts(), streams, "aucun nouveau flux après le retrait des droits");
  assert.equal(await openStreams(), 0);
  await logout();

  // A credential revoked while the Journal is live ends the session by itself.
  await login("user_a1"); await supervise("personnel", "tenant-a"); await button("Journal").click(); await direct("Direct connecté").waitFor();
  await command({ command: "revoke_credential", account: "user_a1" });
  await page.getByLabel("Credential personnel local").waitFor({ timeout: 20000 });
  assert.equal(await page.locator(".human-shell, .sv, [data-event-id]").count(), 0);
  assert.equal(await openStreams(), 0);
  await noSecret("révocation");
};

/**
 * The resume offer has the life of the context it proposes. Only the browser clock is driven here: the
 * control keeps offering the same context, which the page sees as valid or elapsed.
 */
const runResume = async () => {
  const resume = page.getByRole("button", { name: "Reprendre la supervision · administration · tenant-a", exact: true });
  const expire = () => page.evaluate(() => { window.__supervisionAdvance(301000); window.__supervisionExpiry.forEach((run) => run()); });
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await page.getByLabel("Rechercher dans les lignes chargées").fill("running");
  const filtered = await workerRows().count(); assert.ok(filtered > 0 && filtered < 24);
  await expire(); await heading("Choisir un contexte.").waitFor();
  // The clocks agree again before the next automatic identity read: the control's context is an offer.
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  await resume.waitFor({ timeout: 20000 });
  await page.getByText(/Le contrôle ne renouvelle pas un contexte : il en propose un nouveau, de mêmes droits/).waitFor();

  // The list stays open until the offered context has itself expired.
  await expire(); await settle();
  soft(await page.getByText(/a expiré à son tour/).count() === 1, "l’offre de reprise dit que le contexte proposé a expiré à son tour");
  soft(await resume.count() === 1, "l’intention de reprise reste proposée");
  let release; const gate = new Promise((resolve) => { release = resolve; });
  await page.route("**/api/v1/human/identity", async (route) => { await gate; await route.continue(); });
  const before = requests.length;
  await resume.click(); await settle(); await page.waitForTimeout(300);
  assert.equal(await page.getByText("Ce contexte a expiré. Relisez vos accès.", { exact: true }).count(), 0, "cliquer sur une offre périmée n’affiche pas une erreur et ne détruit pas l’intention");
  assert.equal(identityReads(before), 1, "le clic relit les accès une fois");
  assert.deepEqual(humanReads(before), [], "aucune donnée n’est lue avant que le contrôle ait proposé un contexte valide");
  soft((await focused()).key === "human-resume", `pendant la lecture des accès qu’il a lancée, « Reprendre » garde le focus (constaté : ${JSON.stringify(await focused())})`);
  await page.keyboard.press("Enter"); await settle(); await page.waitForTimeout(200);
  soft(identityReads(before) === 1 && (await focused()).key === "human-resume", `une seconde activation de « Reprendre » pendant cette lecture est ignorée, sans perte du focus (lectures d’identité : ${identityReads(before)}, focus : ${JSON.stringify(await focused())})`);
  // The control answers with a context that is valid: the click already confirmed, the view reopens.
  await page.evaluate(() => window.__supervisionAdvance(-301000)); release();
  await heading("Supervision du tenant.").waitFor(); await ready100(); await page.unroute("**/api/v1/human/identity");
  assert.equal(await page.getByLabel("Rechercher dans les lignes chargées").inputValue(), "running", "le filtre saisi est rétabli");
  assert.equal(await workerRows().count(), filtered);
  assert.equal(identityReads(before), 1);

  // The same click while the control still proposes the context this browser sees as elapsed: the accesses
  // are read again by themselves after a moment, and the confirmation given by that click still stands.
  await expire(); await heading("Choisir un contexte.").waitFor();
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  await resume.waitFor({ timeout: 20000 });
  await expire(); await settle();
  await page.getByText(/a expiré à son tour/).waitFor();
  const again = requests.length;
  const answered = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/identity");
  await resume.click(); await answered; await settle(); await page.waitForTimeout(200);
  assert.equal(await heading("Supervision du tenant.").count(), 0, "précondition : le contexte proposé est encore échu pour ce navigateur, la vue ne peut pas rouvrir");
  assert.deepEqual(humanReads(again), [], "aucune donnée n’est lue tant que le contrôle propose le contexte échu");
  soft(await page.getByText(/^Reprise confirmée\. Le contrôle propose encore le contexte expiré : vos accès sont relus dans un instant/).count() === 1, "la reprise confirmée en attente est dite, au lieu d’une liste muette");
  // The clocks agree again before the deferred read.
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  for (let attempt = 0; attempt < 150 && identityReads(again) < 2; attempt++) await page.waitForTimeout(100);
  assert.ok(identityReads(again) >= 2, "précondition : une relecture différée des accès a eu lieu sans action");
  for (let attempt = 0; attempt < 50 && !(await heading("Supervision du tenant.").count()) && !(await resume.count()); attempt++) await page.waitForTimeout(100);
  await settle();
  assert.equal(await heading("Supervision du tenant.").count(), 1, "après une relecture différée, la reprise déjà confirmée rouvre la vue sans second clic");
  await ready100();
  assert.equal(await page.getByLabel("Rechercher dans les lignes chargées").inputValue(), "running", "le filtre saisi est rétabli après la relecture différée");
  assert.ok(identityReads(again) <= 4, `au plus quatre lectures d’identité pour cette reprise (constaté : ${identityReads(again)})`);

  // The tab is hidden while a confirmed resume waits for the control. A hidden tab starts no deferred read
  // and reopens nothing: the confirmation ends there, and the page stops saying that the view will reopen
  // without any other action. `offeredThenElapsed` brings the list back to an offer that expired in turn.
  const identityAnswered = () => page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/identity");
  const confirmedPending = page.getByText(/^Reprise confirmée\. Le contrôle propose encore le contexte expiré/);
  const interrupted = page.getByText(/^Reprise interrompue : l’onglet a été masqué/);
  const reopened = () => heading("Supervision du tenant.").count();
  const offeredThenElapsed = async () => {
    await expire(); await heading("Choisir un contexte.").waitFor();
    await page.evaluate(() => window.__supervisionAdvance(-301000));
    await resume.waitFor({ timeout: 20000 });
    await expire(); await settle();
    await page.getByText(/a expiré à son tour/).waitFor();
  };
  // Back on the tab with the clocks in agreement: the accesses are read, the resume is offered again with
  // the focus, nothing is read before the new click, and that click reopens the view with its filter.
  const returnsToAnOffer = async (label) => {
    const returned = requests.length;
    await visibility("visible");
    await resume.waitFor({ timeout: 20000 });
    for (let attempt = 0; attempt < 30 && identityReads(returned) < 1; attempt++) await page.waitForTimeout(100);
    for (let attempt = 0; attempt < 30 && await page.getByText("Lecture des accès…", { exact: true }).count(); attempt++) await page.waitForTimeout(100);
    await settle();
    soft(identityReads(returned) >= 1, `${label} : au retour sur l’onglet, les accès sont relus`);
    soft(await reopened() === 0 && await page.getByText(/^Reprise (confirmée|interrompue)/).count() === 0, `${label} : au retour sur l’onglet, la reprise est proposée de nouveau et attend un clic`);
    soft((await focused()).key === "human-resume", `${label} : au retour sur l’onglet, l’offre de reprise reçoit le focus (constaté : ${JSON.stringify(await focused())})`);
    assert.deepEqual(humanReads(returned), [], `${label} : aucune donnée n’est lue avant ce nouveau clic`);
    await resume.click();
    await heading("Supervision du tenant.").waitFor(); await ready100();
    assert.equal(await page.getByLabel("Rechercher dans les lignes chargées").inputValue(), "running", `${label} : le filtre saisi est rétabli après la reprise interrompue`);
  };
  // A view that reopened in the hidden tab is the defect these observations name: the scenario then goes on
  // from that open view, so that every hidden-tab case is observed in one run.
  const afterHidden = async (label) => {
    if (!(await reopened())) return returnsToAnOffer(label);
    await visibility("visible"); await ready100();
  };

  // Even a short absence ends the confirmation immediately. Return before the first deferred read:
  // the fresh identity may offer a valid context, but it cannot reuse the click from before the absence.
  await offeredThenElapsed();
  const brieflyHidden = requests.length;
  const firstAnswer = identityAnswered();
  await resume.click(); await firstAnswer; await settle(); await confirmedPending.waitFor();
  await visibility("hidden"); await page.waitForTimeout(300);
  soft(await confirmedPending.count() === 0 && await interrupted.count() === 1,
    "masquer l’onglet, même brièvement, termine aussitôt la confirmation et le dit");
  soft(identityReads(brieflyHidden) === 1 && await reopened() === 0,
    "pendant cette brève absence, aucune relecture différée ni réouverture");
  assert.deepEqual(humanReads(brieflyHidden), [], "aucune donnée n’est lue pour la confirmation interrompue");
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  await returnsToAnOffer("absence de 300 ms avant la première relecture différée");

  // (1) Hidden after the first answer, while the deferred read is armed. The clocks agree again at once: a
  // deferred read, were one to start, would be offered a valid context and the view would reopen unseen.
  await offeredThenElapsed();
  let masked = requests.length;
  let identityRead = identityAnswered();
  await resume.click(); await identityRead; await settle();
  await confirmedPending.waitFor();
  await visibility("hidden");
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  // The first deferred read is due two seconds after the answer; the next ones four and eight seconds later.
  await page.waitForTimeout(5000);
  soft(identityReads(masked) === 1, `onglet masqué : aucune relecture différée des accès ne démarre (lectures d’identité depuis le clic : ${identityReads(masked)})`);
  soft(await reopened() === 0, "onglet masqué : la reprise confirmée ne rouvre pas la vue, même lorsque le contrôle proposerait un contexte valide");
  soft(await confirmedPending.count() === 0, "onglet masqué : la confirmation tombée, la page n’annonce plus que la vue se rouvrira sans autre action");
  soft(await interrupted.count() === 1, "onglet masqué : la page dit que la reprise confirmée est interrompue et qu’elle sera proposée de nouveau");
  soft(humanReads(masked).length === 0, `onglet masqué : aucune donnée n’est lue (constaté : ${JSON.stringify(humanReads(masked))})`);
  await page.waitForTimeout(9000);
  soft(identityReads(masked) === 1, `onglet masqué : aucune des relectures différées suivantes ne démarre non plus (lectures d’identité depuis le clic : ${identityReads(masked)})`);
  await afterHidden("relecture différée non partie");

  // (2) Hidden while the read made for the click is in flight, and the control answers with a valid context.
  // That read started in a visible tab; its answer still reopens nothing in a hidden one: the offer is kept.
  const hold = async () => {
    let release; const gate = new Promise((resolve) => { release = resolve; });
    await page.route("**/api/v1/human/identity", async (route) => { await gate; await route.continue(); });
    return async () => { const answered = identityAnswered(); release(); await answered; await page.unroute("**/api/v1/human/identity"); await settle(); await page.waitForTimeout(300); };
  };
  await offeredThenElapsed();
  let answer = await hold();
  masked = requests.length;
  await resume.click(); await settle(); await page.waitForTimeout(300);
  assert.equal(identityReads(masked), 1, "précondition : la lecture faite pour ce clic est en vol");
  await visibility("hidden");
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  await answer();
  soft(await reopened() === 0, "onglet masqué : un contexte valide proposé après la confirmation ne rouvre pas la vue");
  soft(await resume.count() === 1, "onglet masqué : l’offre de reprise est gardée pour un nouveau clic");
  soft(await interrupted.count() === 1, "onglet masqué : la page dit que la reprise confirmée est interrompue, bien qu’un contexte valide soit proposé");
  soft(humanReads(masked).length === 0, `onglet masqué : aucune donnée n’est lue pour un contexte valide proposé après la confirmation (constaté : ${JSON.stringify(humanReads(masked))})`);
  await page.waitForTimeout(3000);
  soft(identityReads(masked) === 1, `onglet masqué : l’offre gardée ne relit rien (lectures d’identité depuis le clic : ${identityReads(masked)})`);
  await afterHidden("contexte valide proposé onglet masqué");

  // (3) Hidden while the read made for the click is in flight, and the control still proposes the context
  // this browser sees as elapsed: the confirmation ends, and no deferred read is armed.
  await offeredThenElapsed();
  answer = await hold();
  masked = requests.length;
  await resume.click(); await settle(); await page.waitForTimeout(300);
  await visibility("hidden");
  await answer();
  soft(await reopened() === 0 && await confirmedPending.count() === 0 && await interrupted.count() === 1, "onglet masqué et contexte encore échu : la reprise confirmée est dite interrompue");
  await page.waitForTimeout(5000);
  soft(identityReads(masked) === 1, `onglet masqué et contexte encore échu : aucune relecture différée n’est armée (lectures d’identité depuis le clic : ${identityReads(masked)})`);
  assert.deepEqual(humanReads(masked), [], "aucune donnée n’est lue pendant cette attente");
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  await afterHidden("contexte encore échu onglet masqué");
  await logout();
};

/** Keyboard focus across reads that change the page: it stays where the person was, on screen. */
const runFocus = async () => {
  const describe = (state) => JSON.stringify(state);
  // Holds the matching reads until released. The held routes are continued before the route is removed:
  // removing it first would let the browser continue them by itself. A read the page cancelled meanwhile
  // has nothing left to continue.
  const hold = async (matches) => {
    let release; const gate = new Promise((resolve) => { release = resolve; }); const held = [];
    const predicate = (url) => matches(url);
    await page.route(predicate, (route) => { held.push(gate.then(() => route.continue()).catch(() => {})); });
    return async () => { release(); await Promise.all(held); await page.unroute(predicate); };
  };
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();

  // One more page of workers by keyboard, with its read held so that the focus is observed during it.
  let release = await hold((url) => url.pathname === "/api/v1/human/federation/workers" && url.searchParams.has("worker_cursor"));
  await listen(".sv");
  await button("Charger 100 lignes de plus").focus(); await page.keyboard.press("Enter");
  await page.waitForFunction(() => { const node = document.querySelector('[data-focus-key="sv-load-more"]'); return node?.disabled === true || node?.getAttribute("aria-disabled") === "true"; });
  let state = await focused();
  soft(state.key === "sv-load-more", `pendant la lecture, le focus reste sur « Charger 100 lignes de plus » (constaté : ${describe(state)})`);
  await release();
  await page.getByText("120 workers affichés sur 120 chargés · fin du registre à cette lecture", { exact: true }).waitFor(); await settle();
  state = await focused();
  soft(state.panel === "Workers" && state.classes.includes("sv-end") && state.inView, `après la dernière page, le focus est sur la note de fin du pied de table, à l’écran (constaté : ${describe(state)})`);
  await page.waitForTimeout(300);
  const loaded = await said();
  soft(loaded.length === 1 && loaded[0].classes.includes("sv-announce") && loaded[0].text === "120 workers affichés sur 120 chargés.",
    `la page de workers chargée est annoncée une fois, par la région du bandeau (constaté : ${told(loaded)})`);

  // « Voir ces workers » from a broker detail: focus goes to the filtered result, not back to the broker row.
  const currentRow = brokerRows().filter({ hasText: a.brokers.current });
  await currentRow.getByRole("button", { name: "Détails du broker", exact: true }).focus(); await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog"); await dialog.waitFor();
  await dialog.getByRole("button", { name: "Voir ces workers", exact: true }).focus(); await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "detached" }); await settle();
  state = await focused();
  soft(state.panel === "Workers" && state.classes.includes("sv-summary") && state.inView, `après « Voir ces workers » depuis un détail de broker, le focus est sur le résultat filtré (constaté : ${describe(state)})`);
  await button("Réinitialiser").click();

  // Journal: older pages by keyboard, then more rendered rows.
  await button("Journal").click(); await direct("Direct connecté").waitFor();
  release = await hold((url) => url.pathname === "/api/v1/human/events/recent" && url.searchParams.has("cursor"));
  await button("Charger 100 événements plus anciens").focus(); await page.keyboard.press("Enter");
  await page.waitForFunction(() => { const node = document.querySelector('[data-focus-key="jr-older"]'); return node?.disabled === true || node?.getAttribute("aria-disabled") === "true"; });
  state = await focused();
  soft(state.key === "jr-older", `pendant la lecture, le focus reste sur « Charger 100 événements plus anciens » (constaté : ${describe(state)})`);
  await release();
  await journalResult(/^200 événements affichés sur 200 chargés/).waitFor(); await settle();
  state = await focused();
  soft(state.key === "jr-older" && state.inView, `après la lecture, le focus est resté sur le bouton, prêt pour la page suivante (constaté : ${describe(state)})`);
  await button("Charger 100 événements plus anciens").click();
  await journalResult(/^300 événements affichés sur 300 chargés .* 200 lignes rendues$/).waitFor();
  const ids = await page.locator(".sv-journal tbody tr").evaluateAll((nodes) => nodes.map((node) => node.dataset.eventId));
  await button("Afficher 100 lignes de plus").focus(); await page.keyboard.press("Enter"); await settle();
  assert.equal(await page.locator(".sv-journal tbody tr").count(), 300);
  const firstNew = await page.locator(".sv-journal tbody tr").nth(200).getAttribute("data-event-id");
  state = await focused();
  soft(!ids.includes(firstNew) && state.row === firstNew && state.inView, `« Afficher 100 lignes de plus » place le focus sur la première ligne nouvellement rendue (constaté : ${describe(state)})`);

  // Every command that starts a read stays focusable while a read is in flight: announced as unavailable,
  // it ignores another activation and keeps the keyboard focus during the read and after it. `pressed`
  // activates the focused command with its read held, activates it again, then lets the read end.
  const recentPath = "/api/v1/human/events/recent"; const fleetPath = "/api/v1/human/federation/workers";
  const firstPage = (path) => (url) => url.pathname === path && !url.searchParams.has("cursor") && !url.searchParams.has("worker_cursor");
  const holdThenFail = async (matches) => {
    let release; const gate = new Promise((resolve) => { release = resolve; }); const held = [];
    await page.route(matches, (route) => { held.push(gate.then(() => route.abort("connectionrefused")).catch(() => {})); });
    return async () => { release(); await Promise.all(held); await page.unroute(matches); };
  };
  const marked = (key) => page.evaluate((focusKey) => {
    const node = document.querySelector(`[data-focus-key="${focusKey}"]`);
    return { present: !!node, unavailable: node?.getAttribute("aria-disabled") === "true", disabled: node?.disabled === true };
  }, key);
  const pressed = async ({ label, key, path, release, ended, after = key, announced = true }) => {
    // A command that appears while another read is still in flight is unavailable until that read ended.
    await page.waitForFunction((focusKey) => { const node = document.querySelector(`[data-focus-key="${focusKey}"]`); return !!node && !node.disabled && node.getAttribute("aria-disabled") !== "true"; }, key);
    const before = requests.length;
    await page.keyboard.press("Enter");
    for (let attempt = 0; attempt < 50 && !requests.slice(before).some((item) => item.split("?")[0] === path); attempt++) await page.waitForTimeout(50);
    await settle();
    let state = await focused(); const mark = await marked(key);
    soft(state.key === key && state.inView, `${label} : pendant la lecture, le focus clavier reste sur la commande (constaté : ${describe(state)})`);
    if (announced) soft(mark.present && mark.unavailable && !mark.disabled, `${label} : pendant la lecture, la commande est annoncée indisponible sans quitter l’ordre de tabulation (constaté : ${describe(mark)})`);
    const during = humanReads(before).length;
    await page.keyboard.press("Enter"); await settle(); await page.waitForTimeout(250);
    soft(humanReads(before).length === during, `${label} : une seconde activation pendant la lecture ne lance aucune lecture (lectures avant ${during}, après ${humanReads(before).length} : ${JSON.stringify(humanReads(before))})`);
    state = await focused();
    soft(state.key === key, `${label} : la seconde activation ne déplace pas le focus (constaté : ${describe(state)})`);
    await release(); await ended(); await settle(); await page.waitForTimeout(150);
    state = await focused();
    soft(state.key === after && state.tag !== "BODY", `${label} : après la lecture, le focus est sur ${after === key ? "la même commande" : `« ${after} »`} (constaté : ${describe(state)})`);
  };
  const tabbed = async (label, key, next) => {
    await page.keyboard.press("Tab");
    const reached = await page.evaluate(() => ({ tag: document.activeElement?.tagName ?? "", id: document.activeElement?.id ?? "", text: (document.activeElement?.textContent ?? "").trim().slice(0, 40) }));
    soft(next(reached), `${label} : la tabulation suivante atteint la commande qui suit (constaté : ${describe(reached)})`);
    await page.keyboard.press("Shift+Tab");
    soft((await focused()).key === key, `${label} : la tabulation arrière revient sur la commande (constaté : ${describe(await focused())})`);
  };
  const journalSettled = async () => { await page.locator(".sv-journal .sv-source", { hasText: "Journal" }).getByText("Lecture réussie", { exact: true }).waitFor(); await direct("Direct connecté").waitFor(); };
  const journalIdle = () => page.waitForFunction(() => { const node = document.querySelector('[data-focus-key="jr-reload"]'); return !!node && !node.disabled && node.getAttribute("aria-disabled") !== "true"; });

  // Journal: « Relire depuis les plus récents », the same read asked from the destination bar, then « Réessayer ».
  release = await hold(firstPage(recentPath));
  await button("Relire depuis les plus récents").focus();
  await pressed({ label: "« Relire depuis les plus récents »", key: "jr-reload", path: recentPath, release, ended: journalSettled });
  await tabbed("« Relire depuis les plus récents »", "jr-reload", (reached) => reached.id === "jr-search");
  release = await hold(firstPage(recentPath));
  await button("Actualiser").focus();
  await pressed({ label: "« Actualiser » dans le Journal", key: "human-refresh", path: recentPath, release, ended: journalSettled });
  const recentFails = firstPage(recentPath);
  await page.route(recentFails, (route) => route.abort("connectionrefused"));
  await button("Relire depuis les plus récents").focus(); await page.keyboard.press("Enter");
  const journalRetry = page.locator('.sv-journal [data-focus-key="jr-retry"]'); await journalRetry.waitFor();
  await page.unroute(recentFails);
  // A retry that fails again: its error line, and the focus on « Réessayer », outlive the read.
  release = await holdThenFail(firstPage(recentPath));
  await journalRetry.focus();
  await pressed({ label: "« Réessayer » du Journal, lecture de nouveau en échec", key: "jr-retry", path: recentPath, release,
    ended: async () => { await journalIdle(); await page.locator(".sv-journal .sv-source", { hasText: "Journal" }).getByText(/^(Contrôle injoignable|Données périmées)/).waitFor(); } });
  // The retry that succeeds removes its error line: the focus goes to the result line, not to the page.
  if ((await focused()).key !== "jr-retry") await journalRetry.focus();
  await page.keyboard.press("Enter"); await journalSettled(); await settle();
  state = await focused();
  soft(state.classes.includes("sv-summary") && state.inView, `« Réessayer » du Journal, lecture réussie : le focus passe à la ligne de résultat (constaté : ${describe(state)})`);

  // Supervision: « Actualiser maintenant », the same read asked from the destination bar, then « Réessayer ».
  await button("Supervision").click(); await heading("Supervision du tenant.").waitFor(); await ready100();
  release = await hold(firstPage(fleetPath));
  await button("Actualiser maintenant").focus();
  await pressed({ label: "« Actualiser maintenant »", key: "sv-refresh", path: fleetPath, release, ended: ready100 });
  await tabbed("« Actualiser maintenant »", "sv-refresh", (reached) => reached.tag === "SUMMARY" && reached.text === "Accès de ce contexte");
  release = await hold(firstPage(fleetPath));
  await button("Actualiser").focus();
  await pressed({ label: "« Actualiser » dans Supervision", key: "human-refresh", path: fleetPath, release, ended: ready100 });
  const fleetFails = firstPage(fleetPath);
  await page.route(fleetFails, (route) => route.abort("connectionrefused"));
  await button("Actualiser maintenant").focus(); await page.keyboard.press("Enter");
  const fleetRetry = page.locator('.sv [data-focus-key="sv-fleet-retry"]'); await fleetRetry.waitFor();
  await source("Workers").getByText("Contrôle injoignable · dernière lecture conservée", { exact: true }).waitFor();
  await page.unroute(fleetFails);
  release = await hold(firstPage(fleetPath));
  await fleetRetry.focus();
  await pressed({ label: "« Réessayer » de Supervision", key: "sv-fleet-retry", path: fleetPath, release, ended: ready100, after: "sv-refresh" });
  await logout();

  // A context without the worker registry: after a successful retry the focus stays in the view.
  await login("user_a1");
  await page.route("**/api/v1/human/resources**", (route) => route.abort("connectionrefused"));
  await supervise("personnel", "tenant-a"); await source("Ressources").getByText("Contrôle injoignable", { exact: true }).waitFor();
  await page.unroute("**/api/v1/human/resources**");
  await button("Réessayer").first().focus(); await page.keyboard.press("Enter"); await ready100(["Ressources"]); await settle();
  state = await focused();
  soft(state.tag !== "BODY" && state.inSupervision && state.inView, `après une reprise réussie dans un contexte sans registre des workers, le focus reste dans la vue (constaté : ${describe(state)})`);
  await logout();
};

/**
 * What assistive technology is told, and what survives a publication: one short live sentence per view,
 * no re-announcement while nothing changes, rows that keep their nodes.
 */
const runRegions = async () => {
  // Counts mutations of a region, and announcements, from now on.
  const watch = (selector) => page.evaluate((target) => {
    const node = document.querySelector(target); window.__watched = { mutations: 0, alerts: 0 };
    window.__observer?.disconnect();
    window.__observer = new MutationObserver((records) => {
      window.__watched.mutations += records.length;
      for (const record of records) for (const added of record.addedNodes)
        if (added.nodeType === 1 && (added.getAttribute("role") === "alert" || added.querySelector('[role="alert"]'))) window.__watched.alerts++;
    });
    window.__observer.observe(node, { childList: true, subtree: true, characterData: true });
  }, selector);
  const watched = () => page.evaluate(() => window.__watched);
  // Every live region under `scope`: what assistive technology may be told about.
  const live = (scope) => page.evaluate((target) => [...document.querySelectorAll(`${target} [role="status"], ${target}[role="status"], ${target} [aria-live], ${target} [role="alert"]`)].map((node) => ({
    text: node.textContent, buttons: node.querySelectorAll("button").length, items: node.querySelectorAll("li").length, time: /\d{2}:\d{2}/.test(node.textContent), classes: node.className })), scope);
  const brief = (regions) => JSON.stringify(regions.map((region) => ({ ...region, text: region.text.slice(0, 60) })));
  const fleetAnswered = () => page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/federation/workers");
  const workerOrder = () => workerRows().evaluateAll((nodes) => nodes.map((node) => node.querySelector(".sv-id")?.textContent ?? ""));
  const selection = () => page.evaluate(() => getSelection().toString());

  // Every mutation of one region from now on, with the text it leaves: the real sequence, where `listen`
  // keeps only the texts that differ. Returns the text the region holds when the recording starts.
  const record = (selector) => page.evaluate((target) => {
    const node = document.querySelector(target); window.__sequence = [];
    window.__sequenceObserver?.disconnect();
    window.__sequenceObserver = new MutationObserver(() => window.__sequence.push({ at: Math.round(performance.now()), text: node.textContent ?? "" }));
    window.__sequenceObserver.observe(node, { childList: true, subtree: true, characterData: true });
    return node.textContent ?? "";
  }, selector);
  const recorded = () => page.evaluate(() => window.__sequence);
  // The sentence the region already held, said again: emptied first, then written once after a short pause.
  const saidAgain = (sequence, sentence) => sequence.length === 2 && sequence[0].text === "" && sequence[1].text === sentence && sequence[1].at - sequence[0].at >= 50;
  // No read is in flight any more: the command that asks for one is available again.
  const available = (key) => page.waitForFunction((focusKey) => { const node = document.querySelector(`[data-focus-key="${focusKey}"]`); return !!node && !node.disabled && node.getAttribute("aria-disabled") !== "true"; }, key);
  const unreachable = "Contrôle injoignable ou délai dépassé. Vérifiez la connexion au dashboard puis réessayez.";

  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100(); await settle();
  let from; let read; let written;
  let regions = await live(".sv-status");
  soft(regions.length === 1 && regions[0].buttons === 0 && !regions[0].time, `le bandeau de Supervision n’a qu’une région d’annonce, courte, sans bouton ni heure (constaté : ${brief(regions)})`);
  soft(regions.some((region) => /^Workers, brokers, ressources : lecture réussie\. \d+ workers affichés sur \d+ chargés\. \d+ brokers affichés sur \d+ chargés\. \d+ ressources affichées sur \d+ chargées\.$/.test(region.text)),
    `l’annonce de la première lecture dit l’état des sources puis le compte de chaque liste, en une fois (constaté : ${JSON.stringify(regions.map((region) => region.text))})`);
  // The banner region is the only live region of the view: the result lines are not announced by themselves.
  regions = await live(".sv");
  soft(regions.length === 1 && regions[0].classes.includes("sv-announce") && regions[0].buttons === 0 && regions[0].items === 0 && !regions[0].time,
    `Supervision n’a qu’une région d’annonce, celle du bandeau (constaté : ${brief(regions)})`);

  // Typing in a search: the list follows every keystroke, and its count is said once, after the pause.
  const countOf = (line) => `${line.split(" · ")[0]}.`;
  const unfiltered = await workerSummary();
  await listen(".sv");
  await page.getByLabel("Rechercher dans les lignes chargées").click();
  // Seven keystrokes 40 ms apart, paced inside the page so that the load of the host cannot stretch the burst.
  const typed = await page.evaluate(async () => {
    const input = document.querySelector("#sv-worker-search"); const times = []; const lines = new Set();
    for (const letter of "running") {
      input.value += letter; input.dispatchEvent(new Event("input", { bubbles: true })); times.push(Math.round(performance.now()));
      lines.add(document.querySelector("#sv-worker-search").closest(".sv-panel").querySelector(".sv-summary").textContent);
      await new Promise((resolve) => setTimeout(resolve, 40));
    }
    return { times, lines: [...lines], focused: document.activeElement === input };
  });
  const narrowed = await workerSummary();
  assert.equal(typed.times.length, 7, "précondition : sept frappes ont changé la recherche");
  assert.ok(typed.times.at(-1) - typed.times[0] < 450, `précondition : les sept frappes forment une seule rafale (${typed.times.at(-1) - typed.times[0]} ms)`);
  soft(typed.focused && typed.lines.at(-1) === narrowed && narrowed !== unfiltered && / · recherche et filtres appliqués aux seules lignes chargées$/.test(narrowed),
    `la ligne de résultat suit chaque frappe sans attendre, et la saisie garde le focus (constaté : ${JSON.stringify(typed.lines)})`);
  await page.waitForTimeout(1200);
  read = await said();
  soft(read.length === 1 && read[0].classes.includes("sv-announce") && read[0].text === countOf(narrowed),
    `sept frappes dans la recherche des workers : une seule annonce, par la région du bandeau, du compte affiché (constaté : ${told(read)})`);
  soft(read.length >= 1 && read[0].at - typed.times.at(-1) >= 450, `l’annonce attend une pause d’une demi-seconde après la dernière frappe (constaté : ${read.length ? read[0].at - typed.times.at(-1) : "aucune annonce"} ms)`);
  // An automatic cycle that changes the number of rows says the new count once. The control no longer lists
  // one displayed worker: its answer is altered at the browser edge, everything else is the real registry.
  const leaving = await workerRows().first().locator(".sv-id").first().innerText(); const shownBefore = await workerRows().count();
  await page.route(fleetRoute, async (route) => {
    const response = await route.fetch(); const body = await response.json();
    body.workers.items = body.workers.items.filter((item) => item.worker_id !== leaving);
    await route.fulfill({ response, json: body });
  });
  from = (await said()).length;
  let cycle = fleetAnswered();
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await cycle; await ready100();
  for (let attempt = 0; attempt < 30 && await workerRows().count() === shownBefore; attempt++) await page.waitForTimeout(100);
  await settle(); await page.waitForTimeout(300);
  assert.equal(await workerRows().count(), shownBefore - 1, "précondition : le worker retiré de la réponse a quitté la liste");
  read = await said(from);
  soft(read.length === 1 && read[0].classes.includes("sv-announce") && read[0].text === countOf(await workerSummary()),
    `un cycle qui change le nombre de lignes affichées annonce le nouveau compte une fois (constaté : ${told(read)})`);
  await page.unroute(fleetRoute);
  cycle = fleetAnswered();
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await cycle; await ready100();
  for (let attempt = 0; attempt < 30 && await workerRows().count() !== shownBefore; attempt++) await page.waitForTimeout(100);
  await settle();
  // The other two lists, changed within the same pause, are said together; then every filter is removed.
  await page.waitForTimeout(700); from = (await said()).length;
  await page.evaluate((broker) => {
    const type = (id, value) => { const input = document.querySelector(id); input.value = value; input.dispatchEvent(new Event("input", { bubbles: true })); };
    type("#sv-broker-search", broker); type("#sv-resource-search", "no-such-resource");
  }, a.brokers.revoked);
  assert.equal(await brokerRows().count(), 1, "précondition : la recherche des brokers est appliquée");
  await page.waitForTimeout(1200);
  read = await said(from);
  soft(read.length === 1 && read[0].classes.includes("sv-announce") && /^1 broker affiché sur 3 chargés\. 0 ressource affichée sur \d+ chargées\.$/.test(read[0].text),
    `deux recherches changées dans la même pause sont annoncées ensemble, une fois (constaté : ${told(read)})`);
  await page.getByLabel("Rechercher dans les brokers chargés").fill(""); await page.getByLabel("Rechercher dans les ressources chargées").fill("");
  from = (await said()).length;
  await button("Réinitialiser").click(); await page.waitForTimeout(1200);
  read = (await said(from)).filter((entry) => / workers? affichés? sur /.test(entry.text));
  soft(read.length === 1 && read[0].classes.includes("sv-announce") && read[0].text.startsWith(countOf(unfiltered)), `« Réinitialiser » annonce une fois le compte rétabli (constaté : ${told(read)})`);

  // Freshness ticks and a successful cycle with unchanged data touch nothing in the banner but the read time.
  await watch(".sv-status");
  assert.ok(await page.evaluate(() => window.__supervisionTick()) >= 1, "le tic de fraîcheur de la vue est capturé");
  await page.evaluate(() => { window.__supervisionTick(); window.__supervisionTick(); }); await settle();
  soft((await watched()).mutations === 0, `un tic de fraîcheur sans changement ne modifie pas le bandeau (mutations : ${(await watched()).mutations})`);
  // A text selection in a row and a row node survive an automatic cycle that changes nothing.
  const firstId = workerRows().first().locator(".sv-id").first();
  const selected = await firstId.evaluate((node) => { node.closest("tr").dataset.probe = "kept"; getSelection().selectAllChildren(node); return getSelection().toString(); });
  await listen(".sv");
  let answered = fleetAnswered();
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await answered; await ready100(); await settle();
  soft((await said()).length === 0, `un cycle réussi sans changement n’écrit dans aucune région d’annonce de la vue (constaté : ${told(await said())})`);
  soft(await page.evaluate(() => getSelection().toString()) === selected, "une sélection de texte dans une ligne survit à un cycle automatique");
  soft(await page.locator('.sv-panel tbody tr[data-probe="kept"]').count() === 1, "une ligne inchangée garde son nœud d’un cycle au suivant");
  // The same read asked by the person changes nothing either, and still says how it ended. Asked once more,
  // it says the sentence the region already holds: assistive technology may stay silent for an unchanged
  // text, so the region is emptied and written again. An automatic cycle writes nothing at all.
  const banner = ".sv-status .sv-announce";
  let carried = await record(banner);
  answered = fleetAnswered();
  await button("Actualiser maintenant").click(); await answered; await ready100(); await settle(); await page.waitForTimeout(400);
  written = await recorded();
  soft(written.length === 1 && written[0].text === "Workers, brokers, ressources : lecture réussie.",
    `« Actualiser maintenant » qui ne change ni état ni compte dit l’issue de la lecture, en une écriture (région avant : « ${carried} » ; mutations : ${JSON.stringify(written)})`);
  carried = await record(banner);
  answered = fleetAnswered();
  await button("Actualiser maintenant").click(); await answered; await ready100(); await settle(); await page.waitForTimeout(400);
  written = await recorded();
  soft(carried === "Workers, brokers, ressources : lecture réussie." && saidAgain(written, carried),
    `la même actualisation demandée de nouveau vide la région puis y réécrit la même phrase après une courte pause, une seule fois (région avant : « ${carried} » ; mutations : ${JSON.stringify(written)})`);
  await record(banner);
  answered = fleetAnswered();
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await answered; await ready100(); await settle(); await page.waitForTimeout(400);
  written = await recorded();
  soft(written.length === 0, `un cycle automatique sans changement ne touche pas la région d’annonce, même après une actualisation demandée (mutations : ${JSON.stringify(written)})`);

  // An automatic cycle in which one worker changed. The control now reports the first displayed worker as
  // cancelled: its answer is altered at the browser edge, everything else is the real registry. That row is
  // rebuilt further down the default sort; the rows it passes did not change and must not move.
  const beforeOrder = await workerOrder(); const moving = beforeOrder[0];
  const passed = workerRows().nth(1).getByRole("button", { name: "Détails", exact: true }); const passedKey = await passed.getAttribute("data-focus-key");
  await passed.focus();
  const picked = await workerRows().nth(2).locator(".sv-id").first().evaluate((node) => { node.closest("tr").dataset.probe = "passed"; getSelection().selectAllChildren(node); return getSelection().toString(); });
  assert.equal((await focused()).key, passedKey, "précondition : le focus est sur le bouton d’une ligne qui ne changera pas");
  assert.ok(picked && await selection() === picked, "précondition : une sélection de texte existe dans une autre ligne qui ne changera pas");
  await page.route(fleetRoute, async (route) => {
    const response = await route.fetch(); const body = await response.json();
    for (const item of body.workers.items) if (item.worker_id === moving) item.state = "cancelled";
    await route.fulfill({ response, json: body });
  });
  from = (await said()).length;
  answered = fleetAnswered();
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await answered; await ready100();
  for (let attempt = 0; attempt < 30 && (await workerOrder())[0] === moving; attempt++) await page.waitForTimeout(100);
  await settle();
  soft((await said(from)).length === 0, `un cycle qui change l’état d’un worker sans changer aucun compte n’annonce rien (constaté : ${told(await said(from))})`);
  const afterOrder = await workerOrder(); const landed = afterOrder.indexOf(moving);
  assert.ok(landed >= 3, `précondition : le worker modifié a reculé dans le tri, derrière des lignes inchangées (ligne ${landed + 1} sur ${afterOrder.length})`);
  assert.deepEqual(afterOrder.slice(0, landed), beforeOrder.slice(1, landed + 1), "les lignes dépassées gardent leur ordre");
  let held = await focused();
  soft(held.key === passedKey, `un cycle automatique qui réordonne la table laisse le focus clavier sur le bouton d’une ligne inchangée (constaté : ${JSON.stringify(held)})`);
  soft(await selection() === picked, `une sélection de texte dans une ligne inchangée survit à un cycle qui réordonne la table (avant « ${picked} », après « ${await selection()} »)`);
  soft(await page.locator('.sv-panel tbody tr[data-probe="passed"]').count() === 1, "une ligne inchangée dépassée par une autre garde son nœud");
  await page.unroute(fleetRoute);
  // The same worker is read again as the registry holds it: its row is rebuilt at its first place, and the
  // focus it held returns to the same control of the new row.
  const rebuilt = workerRows().filter({ hasText: moving }).getByRole("button", { name: "Détails", exact: true }); const rebuiltKey = await rebuilt.getAttribute("data-focus-key");
  await rebuilt.focus(); await rebuilt.evaluate((node) => { node.dataset.probe = "replaced"; });
  answered = fleetAnswered();
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await answered; await ready100();
  for (let attempt = 0; attempt < 30 && (await workerOrder())[0] !== moving; attempt++) await page.waitForTimeout(100);
  await settle();
  assert.deepEqual(await workerOrder(), beforeOrder, "précondition : la table a retrouvé l’ordre du registre");
  held = await focused();
  soft(held.key === rebuiltKey && await page.locator('[data-probe="replaced"]').count() === 0, `une ligne reconstruite rend le focus à la même commande de la nouvelle ligne (constaté : ${JSON.stringify(held)})`);

  // The sort order changes while the focus stays in a row: the select is operated without taking the focus,
  // as speech control or a script does. A kept row that has to change place gives the focus back.
  const shown = await workerOrder(); const rankOf = new Map([...shown].sort().map((id, index) => [id, index])); const sequence = shown.map((id) => rankOf.get(id));
  // A row that belongs to no longest ordered run has to move, whichever run is left in place.
  const ending = sequence.map(() => 1); const starting = sequence.map(() => 1);
  for (let right = 0; right < sequence.length; right++) for (let left = 0; left < right; left++) if (sequence[left] < sequence[right]) ending[right] = Math.max(ending[right], ending[left] + 1);
  for (let left = sequence.length - 1; left >= 0; left--) for (let right = left + 1; right < sequence.length; right++) if (sequence[left] < sequence[right]) starting[left] = Math.max(starting[left], starting[right] + 1);
  const run = Math.max(...ending); const mover = shown.find((_, index) => ending[index] + starting[index] - 1 < run);
  assert.ok(mover, "précondition : le tri par identifiant déplace au moins une ligne, quelle que soit la suite laissée en place");
  const moverButton = workerRows().filter({ hasText: mover }).getByRole("button", { name: "Détails", exact: true }); const moverKey = await moverButton.getAttribute("data-focus-key");
  await moverButton.focus();
  await page.evaluate(() => { const select = document.querySelector("#sv-worker-order"); select.value = "identifier"; select.dispatchEvent(new Event("change", { bubbles: true })); });
  await settle();
  assert.deepEqual(await workerOrder(), [...shown].sort(), "le tri par identifiant est appliqué");
  held = await focused();
  soft(held.key === moverKey, `une ligne conservée qui a dû changer de place garde le focus clavier (constaté : ${JSON.stringify(held)})`);
  await page.getByLabel("Tri", { exact: true }).selectOption("activity");

  // An open detail across an automatic cycle. Only its dated line follows the new read: the rest keeps its
  // nodes, a text selection and what was already announced.
  const dialog = page.getByRole("dialog"); const detailBody = dialog.locator(".detail-body");
  const cycleUnderDetail = async (precondition) => {
    const shownBefore = await detailBody.innerText();
    // The read time is displayed to the second.
    await page.waitForTimeout(1100);
    const answer = fleetAnswered();
    assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await answer;
    for (let attempt = 0; attempt < 30 && await detailBody.innerText() === shownBefore; attempt++) await page.waitForTimeout(100);
    assert.notEqual(await detailBody.innerText(), shownBefore, precondition); await settle();
  };
  await page.getByLabel("Rechercher dans les lignes chargées").fill(a.session_owned_by_user_a1);
  await workerRows().getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await listen("dialog[open]");
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".sv-events li").first().waitFor(); await settle();
  let heard = await said();
  soft(heard.length === 1 && /^\d+ événement\(s\) reçus/.test(heard[0].text) && heard[0].text.length <= 200, `la lecture des événements d’une session est annoncée une fois, en une phrase (constaté : ${told(heard)})`);
  regions = await live("dialog[open]");
  soft(regions.length === 1 && regions[0].buttons === 0 && regions[0].items === 0, `un détail n’a qu’une région d’annonce, sans bouton ni liste d’événements (constaté : ${brief(regions)})`);
  // The same read asked again ends the same way: the region of the detail is emptied, then written again.
  carried = await record("dialog[open] .sv-announce");
  await dialog.getByRole("button", { name: "Relire les événements de cette session", exact: true }).click();
  for (let attempt = 0; attempt < 50 && !(await recorded()).some((entry) => entry.text); attempt++) await page.waitForTimeout(100);
  await settle(); await page.waitForTimeout(400);
  written = await recorded();
  soft(/^\d+ événement\(s\) reçus/.test(carried) && saidAgain(written, carried),
    `la relecture des événements d’une session, à l’issue inchangée, vide la région du détail puis y réécrit la même phrase, une seule fois (région avant : « ${carried} » ; mutations : ${JSON.stringify(written)})`);
  heard = await said();
  const quoted = await dialog.locator(".sv-events li").first().evaluate((node) => {
    node.closest(".sv-session").dataset.probe = "detail"; node.closest("dialog").querySelector(".sv-definition").dataset.probe = "detail";
    getSelection().selectAllChildren(node); return getSelection().toString();
  });
  assert.ok(quoted && await selection() === quoted, "précondition : une sélection de texte existe dans les événements du détail");
  let heardBefore = heard.length;
  await cycleUnderDetail("précondition : la ligne datée du détail worker suit la nouvelle lecture");
  soft(await dialog.locator('[data-probe="detail"]').count() === 2, "les sections inchangées d’un détail worker ouvert gardent leur nœud d’un cycle au suivant");
  soft(await selection() === quoted, `une sélection de texte dans un détail ouvert survit à un cycle automatique (avant « ${quoted.slice(0, 40)} », après « ${(await selection()).slice(0, 40)} »)`);
  heard = await said(heardBefore);
  soft(heard.length === 0, `un cycle qui ne change que la date de lecture n’annonce rien dans le détail ouvert (constaté : ${told(heard)})`);
  held = await focused();
  soft(held.key === "sv-session-read", `le focus reste sur la commande du détail pendant le cycle (constaté : ${JSON.stringify(held)})`);
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  await button("Réinitialiser").click();
  await brokerRows().filter({ hasText: a.brokers.current }).getByRole("button", { name: "Détails du broker", exact: true }).click(); await dialog.waitFor();
  const lease = await dialog.locator(".sv-definition dd").first().evaluate((node) => { node.closest(".sv-definition").dataset.probe = "broker"; getSelection().selectAllChildren(node); return getSelection().toString(); });
  assert.ok(lease && await selection() === lease, "précondition : une sélection de texte existe dans la fiche du broker");
  await cycleUnderDetail("précondition : le reçu de lecture affiché dans le détail broker suit la nouvelle lecture");
  soft(await dialog.locator('[data-probe="broker"]').count() === 1, "la fiche d’un broker ouvert garde son nœud quand seul le reçu de lecture change");
  soft(await selection() === lease, `une sélection de texte dans la fiche d’un broker survit à un cycle automatique (avant « ${lease} », après « ${await selection()} »)`);
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });

  // An error is announced once, when it appears; it is not created again while it lasts.
  await page.route(fleetRoute, (route) => route.abort("connectionrefused"));
  await watch(".sv-status");
  let failures = page.waitForEvent("requestfailed", (request) => new URL(request.url()).pathname === "/api/v1/human/federation/workers");
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await failures;
  await source("Workers").getByText("Contrôle injoignable · dernière lecture conservée", { exact: true }).waitFor(); await settle();
  soft((await watched()).alerts === 1, `l’erreur est insérée une fois comme alerte (constaté : ${(await watched()).alerts})`);
  await page.evaluate(() => window.__supervisionTick());
  failures = page.waitForEvent("requestfailed", (request) => new URL(request.url()).pathname === "/api/v1/human/federation/workers");
  assert.equal(await page.evaluate(() => window.__supervisionCycle()), 1); await failures; await settle(); await page.waitForTimeout(200);
  soft((await watched()).alerts === 1, `tant que la même erreur dure, l’alerte n’est pas recréée (constaté : ${(await watched()).alerts})`);
  regions = await live(".sv-status");
  soft(regions.some((region) => region.text === "Workers, brokers : données périmées, dernière lecture conservée. Ressources : lecture réussie."), `la péremption est annoncée en une phrase (constaté : ${JSON.stringify(regions.map((region) => region.text.slice(0, 120)))})`);
  // The person asks again while the control is still unreachable. Nothing changes on screen and the same
  // error line stays: the banner region says that error again, once, and the alert is not created again.
  carried = await record(banner);
  failures = page.waitForEvent("requestfailed", (request) => new URL(request.url()).pathname === "/api/v1/human/federation/workers");
  await button("Réessayer").first().click(); await failures; await available("sv-refresh"); await settle(); await page.waitForTimeout(400);
  written = await recorded();
  soft(written.filter((entry) => entry.text).length === 1 && written.at(-1).text === unreachable && (await watched()).alerts === 1,
    `« Réessayer » qui échoue de nouveau de la même façon redit l’erreur par la région du bandeau, une fois, sans recréer l’alerte (région avant : « ${carried} » ; mutations : ${JSON.stringify(written)} ; alertes : ${(await watched()).alerts})`);
  await page.unroute(fleetRoute);

  // Journal: the live batches keep the rows already displayed, a click in progress and the reading position.
  const rows = () => page.locator(".sv-journal tbody tr");
  await button("Journal").click(); await direct("Direct connecté").waitFor(); await settle();
  regions = await live(".sv-journal");
  soft(regions.length === 1 && regions[0].classes.includes("sv-announce") && regions[0].buttons === 0 && !regions[0].time, `le Journal n’a qu’une région d’annonce, dans son bandeau, sans bouton ni heure (constaté : ${brief(regions)})`);
  await listen(".sv-journal");
  const marked = await rows().nth(30).evaluate((node) => { node.dataset.probe = "kept"; node.scrollIntoView({ block: "center" }); return node.dataset.eventId; });
  await settle();
  const top = () => page.evaluate(() => { const node = document.querySelector('.sv-journal tr[data-probe="kept"]'); return node ? Math.round(node.getBoundingClientRect().top) : null; });
  const before = await top();
  // Press the mouse on « Limiter » of the marked row, let a batch arrive, then release: the click must count.
  const limit = page.locator('.sv-journal tr[data-probe="kept"]').getByRole("button", { name: "Limiter à cette ressource", exact: true });
  const box = await limit.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2); await page.mouse.down();
  await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 3 });
  await journalResult(/^103 événements affichés sur 103 chargés/).waitFor(); await settle();
  const after = await top();
  soft(after !== null, "une ligne déjà affichée garde son nœud quand un lot arrive en direct");
  soft(after !== null && Math.abs(after - before) <= 2, `la ligne en cours de lecture ne bouge pas à l’arrivée d’un lot (avant ${before} px, après ${after} px)`);
  if (after !== null) { const now = await limit.boundingBox(); await page.mouse.move(now.x + now.width / 2, now.y + now.height / 2); }
  const counts = (entries) => entries.filter((entry) => / en direct depuis la dernière lecture\.$/.test(entry.text));
  // The count of that first batch is said at once; what follows is told about the read the click asks for.
  for (let attempt = 0; attempt < 30 && !counts(await said()).length; attempt++) await page.waitForTimeout(100);
  // A read the person asked for says its result once when it ends, in the banner region: `asked` waits
  // for the read to settle and returns what the live regions of the Journal were told since `from`.
  const journalRead = page.locator(".sv-journal .sv-source", { hasText: "Journal" });
  const asked = async (from, summary, live = "Direct connecté", badge = "Lecture réussie") => {
    await journalResult(summary).waitFor();
    await journalRead.getByText(badge, { exact: true }).waitFor(); await direct(live).waitFor();
    await settle(); await page.waitForTimeout(400);
    return said(from);
  };
  const result = /^\d+ événements? affichés? sur \d+ chargés?\.$/;
  const once = (entries, text) => entries.length === 1 && entries[0].classes.includes("sv-announce") && text.test(entries[0].text);
  from = (await said()).length;
  await page.mouse.up(); await settle(); await page.waitForTimeout(300);
  soft(await page.locator(".sv-journal").getByText("Limité côté serveur à", { exact: false }).count() === 1, "un clic commencé avant l’arrivée d’un lot aboutit");
  assert.ok(marked);
  read = await asked(from, / affichés? sur \d+ chargés? · /);
  soft(once(read, result), `« Limiter à cette ressource » : le résultat de la lecture est annoncé une fois, par la région du bandeau (constaté : ${told(read)})`);

  // What a live region is told while batches arrive: nothing batch by batch, and their count at a bounded
  // rate. The batch above was the first of this view, so its count was said at once; the four below, one
  // second apart, are said together once fifteen seconds have passed.
  const journalSummary = page.locator(".sv-journal .sv-summary");
  from = (await said()).length;
  await button("Retirer la limite").click();
  read = await asked(from, /^100 événements affichés sur 100 chargés/);
  soft(once(read, /^100 événements affichés sur 100 chargés\.$/), `« Retirer la limite » : le résultat de la lecture est annoncé une fois, par la région du bandeau (constaté : ${told(read)})`);
  const batches = (await said()).length;
  for (let batch = 1; batch <= 4; batch++) {
    await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 1 });
    await journalSummary.filter({ hasText: new RegExp(`^${100 + batch} événements affichés sur ${100 + batch} chargés`) }).waitFor();
    await page.waitForTimeout(1000);
  }
  for (let attempt = 0; attempt < 220 && counts(await said()).length < 2; attempt++) await page.waitForTimeout(100);
  heard = await said(); const counted = counts(heard); const duringBatches = heard.slice(batches);
  soft(heard.every((entry) => entry.classes.includes("sv-announce")), `dans le Journal, seule la région d’annonce du bandeau est écrite pendant le direct (constaté ailleurs : ${told(heard.filter((entry) => !entry.classes.includes("sv-announce")))})`);
  soft(counted.length === 2 && /^[123] événements? reçus? en direct/.test(counted[0].text) && counted[1].text.startsWith("4 événements reçus en direct"), `quatre lots espacés d’une seconde sont annoncés par un seul compte (constaté : ${told(counted)})`);
  soft(counted.length === 2 && counted[1].at - counted[0].at >= 14900, `deux comptes annoncés sont séparés d’au moins 15 secondes (constaté : ${counted.length === 2 ? counted[1].at - counted[0].at : "sans objet"} ms)`);
  soft(duringBatches.length === 1 && counts(duringBatches).length === 1, `quatre lots en direct n’écrivent que leur compte : le résultat n’est jamais annoncé pour un lot (constaté : ${told(duringBatches)})`);
  console.log(`regions: journal live counts announced ${counted.length === 2 ? counted[1].at - counted[0].at : "?"} ms apart; ${counted.length} write(s) of a live region for five batches, ${heard.length - counted.length} for the two reads asked in between, ${heard.filter((entry) => !entry.classes.includes("sv-announce")).length} outside the banner region`);
  // A search is an action of the person: its result is announced once, when typing pauses.
  heardBefore = heard.length;
  await page.getByLabel("Rechercher dans les événements chargés").click(); await page.keyboard.type("no-such-event");
  await heading("Aucun événement chargé ne correspond").waitFor();
  for (let attempt = 0; attempt < 30 && !(await said(heardBefore)).length; attempt++) await page.waitForTimeout(100);
  await page.waitForTimeout(700);
  heard = await said(heardBefore);
  soft(heard.length === 1 && heard[0].classes.includes("sv-announce") && /^0 événement affiché sur 104 chargés/.test(heard[0].text), `le résultat d’une recherche est annoncé une fois par la région d’annonce, après la saisie (constaté : ${told(heard)})`);
  // An older page is a read the person asked for as well. The search is removed first, and its own result said.
  from = (await said()).length;
  await button("Réinitialiser").click();
  for (let attempt = 0; attempt < 30 && !(await said(from)).length; attempt++) await page.waitForTimeout(100);
  await page.waitForTimeout(700);
  from = (await said()).length;
  await button("Charger 100 événements plus anciens").click();
  read = await asked(from, /^204 événements affichés sur 204 chargés/);
  soft(once(read, /^204 événements affichés sur 204 chargés\.$/), `« Charger 100 événements plus anciens » : le résultat de la lecture est annoncé une fois, par la région du bandeau (constaté : ${told(read)})`);
  // A reload whose direct is refused: the window may now miss events, so the state changes with the read,
  // and one sentence says the state, then the result. A result written first would be replaced by the
  // state before it is read out.
  const tooMany = { error: { code: "capacity_exhausted", message: "Concurrent stream capacity is full.", retry_action: "close_unused_streams" } };
  await page.route("**/api/v1/human/stream**", (route) => route.fulfill({ status: 429, contentType: "application/json", body: JSON.stringify(tooMany) }));
  from = (await said()).length;
  await button("Relire depuis les plus récents").click();
  read = await asked(from, /^100 événements affichés sur 100 chargés/, "Direct refusé", "Données périmées · dernière lecture conservée");
  soft(read.length === 1 && read[0].classes.includes("sv-announce") && read[0].text === "Journal : données périmées, dernière lecture conservée. Direct refusé. 100 événements affichés sur 100 chargés.",
    `une lecture demandée qui change aussi l’état est annoncée par une seule phrase, l’état puis le résultat (constaté : ${told(read)})`);
  await page.unroute("**/api/v1/human/stream**");
  from = (await said()).length;
  await button("Relire depuis les plus récents").click();
  read = await asked(from, /^100 événements affichés sur 100 chargés/);
  soft(read.length === 1 && read[0].classes.includes("sv-announce") && read[0].text === "Journal : lecture réussie. Direct connecté. 100 événements affichés sur 100 chargés.",
    `la relecture qui rétablit le direct dit l’état rétabli et le résultat en une seule phrase (constaté : ${told(read)})`);
  // The state is now the one already said: the same reload says its result only.
  from = (await said()).length;
  await button("Relire depuis les plus récents").click();
  read = await asked(from, /^100 événements affichés sur 100 chargés/);
  soft(once(read, /^100 événements affichés sur 100 chargés\.$/), `« Relire depuis les plus récents » : le résultat de la lecture est annoncé une fois, par la région du bandeau (constaté : ${told(read)})`);
  // The same reload once more: its result is the sentence the region already holds. It is still one
  // effective announcement: the region is emptied, then the sentence is written once after a short pause.
  carried = await record(".sv-journal .sv-announce");
  from = (await said()).length;
  await button("Relire depuis les plus récents").click();
  read = await asked(from, /^100 événements affichés sur 100 chargés/);
  written = await recorded();
  soft(carried === "100 événements affichés sur 100 chargés." && saidAgain(written, carried),
    `une relecture au résultat inchangé vide la région d’annonce puis y réécrit la même phrase après une courte pause, une seule fois (région avant : « ${carried} » ; mutations : ${JSON.stringify(written)})`);
  // A reload that fails says its state and shows its error line. « Réessayer » that fails again the same
  // way leaves both as they were: the banner region then says the error again, once.
  const recentReads = (url) => url.pathname === "/api/v1/human/events/recent";
  await page.route(recentReads, (route) => route.abort("connectionrefused"));
  await button("Relire depuis les plus récents").click();
  const journalRetry = page.locator('.sv-journal [data-focus-key="jr-retry"]'); await journalRetry.waitFor();
  await available("jr-reload"); await settle(); await page.waitForTimeout(400);
  carried = await record(".sv-journal .sv-announce");
  const failedAgain = page.waitForEvent("requestfailed", (request) => new URL(request.url()).pathname === "/api/v1/human/events/recent");
  await journalRetry.click(); await failedAgain; await available("jr-reload"); await settle(); await page.waitForTimeout(400);
  written = await recorded();
  soft(carried === "Journal : données périmées, dernière lecture conservée. Direct arrêté." && written.filter((entry) => entry.text).length === 1 && written.at(-1).text === unreachable,
    `« Réessayer » du Journal qui échoue de nouveau de la même façon redit l’erreur par la région du bandeau, une fois (région avant : « ${carried} » ; mutations : ${JSON.stringify(written)})`);
  await page.unroute(recentReads);
  await logout();
};

/** Details at phone widths: nothing scrolls sideways, inside the modal detail or outside it. */
const runDetails = async () => {
  const resourcesPanel = page.locator(".sv-panel", { has: heading("Ressources autorisées") });
  const dialog = page.getByRole("dialog");
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await page.getByLabel("Rechercher dans les ressources chargées").fill(a.session_owned_by_user_a1);
  await resourcesPanel.getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  await dialog.getByRole("button", { name: /^Ouvrir le worker / }).waitFor();
  await geometry("detail-resource", [390, 320]);
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".sv-events li").first().waitFor();
  await geometry("detail-resource-events", [390, 320], { shots: false });
  await dialog.getByRole("button", { name: /^Ouvrir le worker / }).click();
  await dialog.getByText("Autorité du lancement", { exact: true }).waitFor();
  await geometry("detail-worker", [390, 320], { shots: false });
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".sv-events li").first().waitFor();
  await geometry("detail-worker-events", [390, 320], { shots: false });
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  await brokerRows().filter({ hasText: a.brokers.current }).getByRole("button", { name: "Détails du broker", exact: true }).click(); await dialog.waitFor();
  await geometry("detail-broker", [390, 320], { shots: false });
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  await logout();
};

/** States other than the nominal one, at phone widths and with their resume action. Refusals and the budget stop are injected at the browser edge. */
const runStates = async () => {
  await login("admin_a");
  // Loading: a first read in flight is unknown content, with its own labels.
  let releaseFirst; const firstGate = new Promise((resolve) => { releaseFirst = resolve; }); const heldFirst = [];
  await page.route(fleetRoute, (route) => { heldFirst.push(firstGate.then(() => route.continue())); });
  await supervise("administration", "tenant-a");
  await source("Workers").getByText("Lecture en cours…", { exact: true }).waitFor(); await source("Workers").getByText("Aucune lecture réussie", { exact: true }).waitFor();
  await heading("Lecture des workers…").waitFor(); await heading("Lecture des brokers…").waitFor();
  await page.getByText("Workers : lecture en cours, contenu encore inconnu.", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 0); assert.equal(await heading("Aucun worker enregistré à cette lecture").count(), 0, "un chargement n’est pas un registre vide");
  releaseFirst(); await Promise.all(heldFirst); await page.unroute(fleetRoute);
  await ready100();
  // Stale after a failed refresh.
  await page.route(fleetRoute, (route) => route.abort("connectionrefused"));
  await button("Actualiser maintenant").click();
  await source("Workers").getByText("Contrôle injoignable · dernière lecture conservée", { exact: true }).waitFor();
  await geometry("stale", [390, 320]);
  await page.unroute(fleetRoute); await button("Réessayer").first().click(); await ready100();

  // A refusal of the composed read without a readable reason, while the control still offers the context.
  await page.route(fleetRoute, (route) => route.fulfill({ status: 403, contentType: "application/json", body: "{}" }));
  let before = requests.length;
  await button("Actualiser maintenant").click();
  await source("Workers").getByText("Lecture refusée", { exact: true }).waitFor(); await source("Brokers").getByText("Lecture refusée", { exact: true }).waitFor();
  await source("Ressources").getByText("Lecture réussie", { exact: true }).waitFor();
  assert.equal(await workerRows().count(), 0); assert.equal(await brokerRows().count(), 0);
  await heading("Workers retirés").waitFor();
  await page.getByText("Actualisation automatique arrêtée : lecture refusée par le contrôle.", { exact: true }).waitFor();
  for (let attempt = 0; attempt < 30 && identityReads(before) < 1; attempt++) await page.waitForTimeout(100);
  soft(identityReads(before) === 1, `le refus sans motif fait relire les accès une fois (lectures d’identité : ${identityReads(before)})`);
  assert.equal(await heading("Choisir un contexte.").count(), 0, "le contrôle propose toujours ce contexte : seul le registre est refusé");
  await geometry("refused", [390, 320]);
  await page.unroute(fleetRoute);
  await button("Relire mes accès").first().click(); await heading("Choisir un contexte.").waitFor();
  assert.equal(await page.locator(".sv").count(), 0);

  // The durable read budget stops: no automatic read, then one explicit read after the operator acted.
  await supervise("administration", "tenant-a"); await ready100();
  const exhausted = { error: { code: "audit_read_budget_exhausted", message: "The durable read budget is exhausted.", retry_action: "contact_operator" } };
  await page.route(fleetRoute, (route) => route.fulfill({ status: 429, contentType: "application/json", body: JSON.stringify(exhausted) }));
  await button("Actualiser maintenant").click();
  await page.getByText(/^Lecture automatique suspendue : budget durable de lecture atteint\./).waitFor();
  await page.getByText("Actualisation automatique arrêtée : budget durable de lecture atteint.", { exact: true }).waitFor();
  assert.equal(await button("Actualiser maintenant").isDisabled(), true);
  before = requests.length;
  await page.evaluate(() => window.__supervisionCycle()); await page.waitForTimeout(500);
  assert.deepEqual(humanReads(before), [], "aucune lecture après l’arrêt du budget");
  await geometry("budget", [390, 320], { shots: false });
  await page.unroute(fleetRoute); before = requests.length;
  await button("Relire après intervention de l’opérateur").click(); await ready100();
  assert.deepEqual(humanReads(before), [fleetUrl, "/api/v1/human/resources?limit=100"], "la flotte d’abord, les ressources une fois le budget rouvert");
  soft(await page.getByText(/[Bb]udget durable de lecture atteint/).count() === 0, "aucune erreur de budget ne reste affichée après la reprise");

  // Back on the tab while the identity cannot be read once: no view keeps claiming a hidden tab.
  const missedReturn = async () => {
    await page.route("**/api/v1/human/identity", (route) => route.abort("connectionrefused"));
    const failed = page.waitForEvent("requestfailed", (request) => new URL(request.url()).pathname === "/api/v1/human/identity");
    await visibility("visible"); await failed; await page.unroute("**/api/v1/human/identity"); await settle(); await page.waitForTimeout(400);
  };
  await visibility("hidden");
  await page.getByText("Actualisation automatique suspendue : onglet masqué.", { exact: true }).waitFor();
  await missedReturn();
  soft(await page.getByText("Actualisation automatique suspendue : onglet masqué.", { exact: true }).count() === 0, "onglet visible : Supervision n’annonce plus un onglet masqué après une revalidation manquée");
  await ready100();

  // An identity read that is answered after the person opened another destination releases its loading
  // state: « Actualiser » stays usable and the next return to the tab revalidates the accesses.
  let releaseIdentity; const identityGate = new Promise((resolve) => { releaseIdentity = resolve; }); const heldIdentity = [];
  await page.route("**/api/v1/human/identity", (route) => { heldIdentity.push(identityGate.then(() => route.continue())); });
  const identityAsked = page.waitForRequest((request) => new URL(request.url()).pathname === "/api/v1/human/identity");
  await visibility("visible"); await identityAsked;
  await button("Brokers").click(); await heading("Brokers enregistrés du tenant.").waitFor();
  const identityAnswered = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/identity");
  releaseIdentity(); await Promise.all(heldIdentity); await identityAnswered; await page.unroute("**/api/v1/human/identity");
  await page.locator("tbody tr").first().waitFor(); await settle(); await page.waitForTimeout(300);
  soft(!(await button("Actualiser").isDisabled()), "une lecture d’identité qui aboutit après un changement de destination libère « Actualiser »");
  before = requests.length;
  await visibility("visible");
  for (let attempt = 0; attempt < 30 && identityReads(before) < 1; attempt++) await page.waitForTimeout(100);
  await page.waitForTimeout(300);
  soft(identityReads(before) === 1, `le retour d’onglet suivant revalide les accès une fois (lectures d’identité : ${identityReads(before)})`);
  await button("Supervision").click(); await heading("Supervision du tenant.").waitFor(); await ready100();

  // Journal. The control answers "retry" to the stream: a disconnection, reopened without any action.
  const streamRoute = "**/api/v1/human/stream**"; const journalRead = page.locator(".sv-journal .sv-source", { hasText: "Journal" });
  const unavailable = { error: { code: "identity_unavailable", message: "The human gateway is unavailable.", retry_action: "retry" } };
  await page.route(streamRoute, (route) => route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify(unavailable) }));
  await button("Journal").click();
  await direct("Direct déconnecté · reconnexion").waitFor();
  await journalRead.getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
  assert.ok(await page.locator("[data-event-id]").count() > 0, "la page lue reste affichée pendant la reconnexion");
  assert.equal(await direct("Direct refusé").count(), 0, "une indisponibilité temporaire n’est pas un refus du contrôle");
  await page.unroute(streamRoute);
  await direct("Direct connecté").waitFor({ timeout: 20000 });
  await journalRead.getByText("Données périmées · dernière lecture conservée", { exact: true }).waitFor();
  await button("Relire depuis les plus récents").click();
  await direct("Direct connecté").waitFor();
  await journalRead.getByText("Lecture réussie", { exact: true }).waitFor();
  // A real refusal stops at once and tells what to do.
  const tooMany = { error: { code: "capacity_exhausted", message: "Concurrent stream capacity is full.", retry_action: "close_unused_streams" } };
  await page.route(streamRoute, (route) => route.fulfill({ status: 429, contentType: "application/json", body: JSON.stringify(tooMany) }));
  await button("Relire depuis les plus récents").click();
  await direct("Direct refusé").waitFor();
  await page.getByText("Trop de flux ouverts pour ce compte. Fermez les onglets inutilisés, puis relisez depuis les plus récents.", { exact: true }).waitFor();
  await geometry("direct-refused", [390, 320], { shots: false });
  await page.unroute(streamRoute);
  // A refusal that does not follow the error contract, as an intermediary may answer in place of the
  // control, is a refusal too: it is not taken for a disconnection and never asked again by itself.
  await page.route(streamRoute, (route) => route.fulfill({ status: 403, contentType: "application/json", body: "{}" }));
  const streamsBefore = streamStarts();
  await button("Relire depuis les plus récents").click();
  await direct("Direct refusé").waitFor();
  await page.getByText("Consultation refusée pour cette ressource. Faites vérifier vos permissions.", { exact: true }).waitFor();
  await page.waitForTimeout(1500);
  assert.equal(streamStarts() - streamsBefore, 1, "un refus du direct sans corps conforme n’est pas relancé");
  assert.equal(await direct("Direct déconnecté · reconnexion").count(), 0);
  await page.unroute(streamRoute);
  await button("Relire depuis les plus récents").click(); await direct("Direct connecté").waitFor();
  await visibility("hidden"); await direct("Direct suspendu · onglet masqué").waitFor();
  await missedReturn();
  const stillHidden = await direct("Direct suspendu · onglet masqué").count();
  soft(stillHidden === 0, "onglet visible : le Journal n’annonce plus un onglet masqué après une revalidation manquée");
  if (!stillHidden) await direct("Direct connecté").waitFor();
  await logout();
};

/**
 * The browser clock alone runs past the expiry. The control still offers the same context with the same
 * expiry, so nothing can resume: rows are removed, the identity is read at most three more times, then the
 * choice is manual. Once the clocks agree again, an explicit refresh offers the resume; a click on that
 * offer after it expired in its turn repeats the read, although the three automatic reads were used. A real
 * renewal is exercised by the ttl scenario.
 */
const runExpiry = async () => {
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await page.getByLabel("Rechercher dans les lignes chargées").fill("running");
  const before = requests.length;
  assert.ok(await page.evaluate(() => window.__supervisionExpiry.length) >= 1, "le minuteur d’expiration du contexte est armé");
  await page.evaluate(() => { window.__supervisionAdvance(301000); window.__supervisionExpiry.forEach((expire) => expire()); });
  await heading("Choisir un contexte.").waitFor();
  await purged("expiration", sortedWorkersA);
  await page.getByText(/^Le contexte a expiré et le contrôle propose encore l’ancien\. Actualisez la liste dans un instant/).waitFor({ timeout: 30000 });
  assert.equal(identityReads(before), 4, "une relecture d’identité puis au plus trois nouvelles tentatives");
  assert.deepEqual(humanReads(before), [], "aucune lecture de données après l’expiration");
  assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0, "le même contexte expiré n’est jamais présenté comme un nouveau");
  assert.equal(await button("Ouvrir administration · tenant-a").isDisabled(), true);
  assert.equal(await button("Superviser administration · tenant-a").isDisabled(), true);
  await page.waitForTimeout(9000);
  assert.equal(identityReads(before), 4, "les tentatives s’arrêtent : aucune boucle de lecture");
  // An explicit refresh reads the identity once more; the same expired context is still not an offer. It
  // is asked by keyboard, with its read held: « Actualiser » keeps the focus during the read and after it,
  // and another activation meanwhile reads nothing more.
  let releaseIdentity; const identityGate = new Promise((resolve) => { releaseIdentity = resolve; });
  await page.route("**/api/v1/human/identity", async (route) => { await identityGate; await route.continue(); });
  await button("Actualiser").focus(); await page.keyboard.press("Enter");
  for (let attempt = 0; attempt < 50 && identityReads(before) < 5; attempt++) await page.waitForTimeout(100);
  await settle();
  soft((await focused()).key === "human-refresh", `pendant la lecture des accès qu’il a lancée, « Actualiser » garde le focus clavier (constaté : ${JSON.stringify(await focused())})`);
  await page.keyboard.press("Enter"); await settle(); await page.waitForTimeout(200);
  const identityAnswered = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/identity");
  releaseIdentity(); await identityAnswered; await page.unroute("**/api/v1/human/identity"); await settle();
  await page.waitForTimeout(500);
  assert.equal(identityReads(before), 5, "une seule lecture d’identité par actualisation explicite, même activée deux fois pendant la lecture");
  assert.equal(await page.getByRole("button", { name: /^Reprendre/ }).count(), 0);
  soft((await focused()).key === "human-refresh", `après la lecture des accès, « Actualiser » a gardé le focus clavier (constaté : ${JSON.stringify(await focused())})`);

  // The clocks agree again: an explicit refresh offers the resume. The three automatic reads are used.
  // The focus was not moved: the same key asks again, and the intention of resume is still there.
  const resume = page.getByRole("button", { name: "Reprendre la supervision · administration · tenant-a", exact: true });
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  await page.keyboard.press("Enter");
  const offeredAgain = await resume.waitFor({ timeout: 8000 }).then(() => true, () => false);
  assert.ok(offeredAgain, `une nouvelle activation au clavier, sans déplacer le focus, relit les accès et la reprise est proposée (focus : ${JSON.stringify(await focused())})`);
  // The list stays open until the offered context has expired in its turn, and the control still proposes it.
  // « Reprendre » is an explicit action: the accesses are read again as after an expiry, three times at most.
  await page.evaluate(() => { window.__supervisionAdvance(301000); window.__supervisionExpiry.forEach((expire) => expire()); }); await settle();
  await page.getByText(/a expiré à son tour/).waitFor();
  const clicked = requests.length;
  const answered = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/human/identity");
  await resume.click(); await answered; await settle(); await page.waitForTimeout(200);
  assert.equal(await heading("Supervision du tenant.").count(), 0, "précondition : le contexte proposé est encore échu pour ce navigateur");
  soft(await page.getByText(/^Reprise confirmée\. Le contrôle propose encore le contexte expiré/).count() === 1 && await page.getByText(/^Le contexte a expiré et le contrôle propose encore l’ancien/).count() === 0,
    "après trois relectures automatiques déjà consommées, un clic sur une offre échue garde la reprise confirmée en attente au lieu de l’abandonner à la première lecture");
  // The clocks agree before the deferred read: the view reopens without another click.
  await page.evaluate(() => window.__supervisionAdvance(-301000));
  for (let attempt = 0; attempt < 80 && identityReads(clicked) < 2; attempt++) await page.waitForTimeout(100);
  soft(identityReads(clicked) >= 2, `le dashboard répète la lecture des accès après ce clic, sans autre action (lectures d’identité depuis le clic : ${identityReads(clicked)})`);
  const reopened = await heading("Supervision du tenant.").waitFor({ timeout: 5000 }).then(() => true, () => false);
  soft(reopened, "la vue se rouvre d’elle-même dès que le contrôle propose un contexte valide");
  if (reopened) {
    await ready100();
    assert.equal(await page.getByLabel("Rechercher dans les lignes chargées").inputValue(), "running", "le filtre saisi est rétabli");
    assert.ok(identityReads(clicked) <= 4, `au plus quatre lectures d’identité pour ce clic (constaté : ${identityReads(clicked)})`);
    assert.deepEqual(humanReads(clicked).sort(), [fleetUrl, "/api/v1/human/resources?limit=100"], "les données ne sont relues qu’une fois la vue rouverte");
  }
  await logout();
};

/** Real expiry after 300 seconds: the control offers a different context; the person confirms the resume. */
const runTtl = async () => {
  const guard = await prepareRecoveryGuard();
  recoveryStage = "ttl_initial_context";
  const search = page.getByLabel("Rechercher dans les lignes chargées");
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  await search.fill("running");
  const filtered = await workerRows().count(); assert.ok(filtered > 0 && filtered < 24);
  const firstContext = contexts.filter(Boolean).at(-1);
  const started = Date.now(); const beforeWait = requests.length; const clocks = () => page.evaluate(() => ({ wall: Date.now(), monotonic: performance.now() }));
  const clock0 = await clocks();
  recoveryStage = "ttl_wait_expiry";
  await heading("Choisir un contexte.").waitFor({ timeout: 330000 });
  const waited = Date.now() - started; const atExpiry = requests.length; const clock1 = await clocks();
  console.log(`ttl: browser wall clock advanced ${Math.round((clock1.wall - clock0.wall) - (clock1.monotonic - clock0.monotonic))} ms more than its monotonic clock`);
  assert.ok(waited >= 240000, `le contexte a vécu ${Math.round(waited / 1000)} s avant son expiration réelle`);
  await purged("expiration réelle", sortedWorkersA);
  recoveryStage = "ttl_resume_offer";
  const resume = page.getByRole("button", { name: "Reprendre la supervision · administration · tenant-a", exact: true });
  // The control keeps offering the expired context until its own lifetime ends. The dashboard reads the
  // identity three more times by itself; after that only an explicit refresh of the list reads it again.
  let explicit = 0;
  const offered = await resume.waitFor({ timeout: 20000 }).then(() => true, () => false);
  for (; !offered && explicit < 24 && !(await resume.count()); explicit++) { await button("Actualiser").click(); await page.waitForTimeout(5000); }
  await resume.waitFor({ timeout: 5000 });
  console.log(`ttl: resume offered ${Math.round((Date.now() - started - waited) / 1000)} s after the client expiry, after ${explicit} explicit refresh(es)`);
  if (!explicit) assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("data-focus-key")), "human-resume", "le bouton de reprise reçoit le focus");
  else await resume.focus();
  await page.getByText(/Le contrôle ne renouvelle pas un contexte : il en propose un nouveau, de mêmes droits/).waitFor();
  // No identity read happens while the view is open and visible: these are the reads caused by the expiry.
  const caused = identityReads(beforeWait);
  assert.ok(caused >= 1 && caused <= 4 + explicit, `au plus quatre lectures d’identité automatiques (constaté : ${caused}, dont ${explicit} explicites)`);
  assert.deepEqual(humanReads(atExpiry), [], "aucune donnée n’est relue avant la confirmation");
  await purged("avant confirmation", sortedWorkersA);
  recoveryStage = "ttl_resume_confirmed";
  await page.keyboard.press("Enter");
  await heading("Supervision du tenant.").waitFor(); await ready100();
  assert.equal(await search.inputValue(), "running", "le filtre saisi est rétabli");
  assert.equal(await workerRows().count(), filtered, "les données sont relues puis filtrées de la même façon");
  // The only heartbeat was declared when the stack started, with a 300-second lease: the control now
  // evaluates that lease as stale. It is shown as such, and proves neither a stop nor a released quota.
  await brokerRows().filter({ hasText: a.brokers.current }).getByText("Bail périmé", { exact: true }).waitFor();
  assert.equal(await brokerRows().filter({ hasText: a.brokers.current }).getByText("Bail courant", { exact: true }).count(), 0);
  const secondContext = contexts.filter(Boolean).at(-1);
  assert.ok(firstContext && secondContext && firstContext !== secondContext, "le contexte repris est un nouveau contexte du contrôle");
  assert.deepEqual(humanReads(atExpiry).sort(), ["/api/v1/human/federation/workers?worker_limit=100&broker_limit=100", "/api/v1/human/resources?limit=100"]);
  console.log(`ttl: context lived ${Math.round(waited / 1000)} s after its selection, ${caused} identity read(s) before the offer`);
  await logout();
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** Journal: newest first over a dense real history, live without replay, reconnection without duplicate. */
const runJournal = async () => {
  const guard = await prepareRecoveryGuard();
  const panel = page.locator(".sv-journal"); const rows = () => panel.locator("tbody tr");
  const eventIds = () => rows().evaluateAll((nodes) => nodes.map((node) => node.dataset.eventId));
  const summary = () => panel.locator(".sv-summary").innerText();
  const streams = (since) => requests.slice(since).filter((path) => path.startsWith("/api/v1/human/stream"));
  const recents = (since) => requests.slice(since).filter((path) => path.startsWith("/api/v1/human/events/recent"));
  const headPath = "/api/v1/human/events/recent?limit=100&view=metadata";
  const readHead = async (action) => {
    const [reply] = await Promise.all([page.waitForResponse((answer) =>
      answer.url() === recoveryOrigin + headPath && answer.request().method() === "GET"), action()]);
    assert.equal(reply.status(), 200);
    const body = await reply.json();
    assert.ok(typeof body.live_cursor === "string" && body.live_cursor.length > 0);
    return body;
  };
  const streamCursor = (path) => new URL(path, recoveryOrigin).searchParams.get("cursor");

  recoveryStage = "journal_initial_page";
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  assert.deepEqual(await tabs(), ["Contextes", "Supervision", "Journal", "Ressources", "Événements", "Brokers", "Workers", "Fédération"]);
  await page.getByLabel("Rythme d’actualisation").selectOption("0");
  await page.getByText("Actualisation manuelle : aucune lecture sans action de votre part.", { exact: true }).waitFor();
  let before = requests.length;
  const initialHead = await readHead(() => button("Journal").click());
  await heading("Journal des événements autorisés.").waitFor();
  await panel.locator(".sv-source", { hasText: "Journal" }).getByText("Lecture réussie", { exact: true }).waitFor();
  await direct("Direct connecté").waitFor();
  assert.equal(await page.locator(".sv:not(.sv-journal)").count(), 0, "la vue quittée ne garde aucune ligne");
  assert.deepEqual(recents(before), [headPath], "une seule page : les cent plus récents d’un historique de plus de 1 500 événements");
  assert.equal(streams(before).length, 1);
  assert.match(streams(before)[0], /^\/api\/v1\/human\/stream\?limit=100&view=metadata&cursor=[^&]+$/, "le direct part de la position de la page lue");
  assert.equal(streamCursor(streams(before)[0]), initialHead.live_cursor);
  assert.equal(await rows().count(), 100);
  assert.equal(await summary(), "100 événements affichés sur 100 chargés · des événements plus anciens existent au contrôle");
  const initial = await eventIds(); assert.equal(new Set(initial).size, 100);
  await page.waitForTimeout(2500);
  assert.equal(await rows().count(), 100, "le direct ne rejoue pas l’historique");

  // Real events appended by the control appear on top, without any new page read.
  recoveryStage = "journal_live_append";
  before = requests.length;
  const appended = (await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 3 })).event_ids;
  await page.getByText("103 événements affichés sur 103 chargés · des événements plus anciens existent au contrôle", { exact: true }).waitFor();
  assert.deepEqual((await eventIds()).slice(0, 3), [...appended].reverse(), "les nouveaux événements en tête, le plus récent d’abord");
  assert.deepEqual((await eventIds()).slice(3), initial, "la page déjà lue n’est ni relue ni réordonnée");
  await direct("Direct connecté").waitFor();
  await panel.getByText("3 événement(s) reçu(s) depuis la dernière lecture", { exact: true }).waitFor();
  assert.deepEqual(recents(before), [], "aucune relecture de page pour un lot en direct");
  // Not authorized for another account: checked below with user A2.

  // The tab is hidden: the stream is closed. Events appended meanwhile are read when it comes back,
  // from the newest, on a new stream position. (Only the browser's visibility state is driven by the test.)
  recoveryStage = "journal_resume_cursor";
  assert.equal(await openStreams(), 1, "un seul flux ouvert pendant le direct");
  await visibility("hidden");
  await direct("Direct suspendu · onglet masqué").waitFor();
  assert.equal(await openStreams(), 0, "onglet masqué : le flux ouvert est terminé, pas seulement remplacé");
  assert.equal(await rows().count(), 103, "la dernière fenêtre lue reste affichée");
  const missed = (await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 2 })).event_ids;
  before = requests.length;
  await page.waitForTimeout(1500);
  assert.deepEqual(humanReads(before), [], "aucune lecture ni flux onglet masqué");
  const resumedHead = await readHead(() => visibility("visible"));
  await direct("Direct connecté").waitFor({ timeout: 20000 });
  await page.getByText("100 événements affichés sur 100 chargés · des événements plus anciens existent au contrôle", { exact: true }).waitFor();
  const afterReturn = await eventIds();
  assert.equal(new Set(afterReturn).size, 100, "aucun doublon après la reprise");
  assert.deepEqual(afterReturn.slice(0, 5), [...missed].reverse().concat([...appended].reverse()), "les événements manqués sont en tête, puis ceux déjà reçus");
  assert.deepEqual(recents(before), [headPath], "une relecture depuis les plus récents");
  assert.equal(streams(before).length, 1);
  assert.match(streams(before)[0], /&cursor=/, "le nouveau flux part de la position de la page relue, jamais du début");
  assert.equal(streamCursor(streams(before)[0]), resumedHead.live_cursor);

  // Older pages on request; a server-side limit to one resource; back to the whole selection.
  recoveryStage = "journal_pagination_and_links";
  before = requests.length;
  await button("Charger 100 événements plus anciens").click();
  await page.getByText("200 événements affichés sur 200 chargés · des événements plus anciens existent au contrôle", { exact: true }).waitFor();
  assert.match(recents(before)[0], /^\/api\/v1\/human\/events\/recent\?limit=100&view=metadata&cursor=/);
  await button("Charger 100 événements plus anciens").click();
  await page.getByText(/^300 événements affichés sur 300 chargés · des événements plus anciens existent au contrôle · 200 lignes rendues$/).waitFor();
  assert.equal(await rows().count(), 200, "200 lignes rendues, les autres sur demande");
  await button("Afficher 100 lignes de plus").click(); assert.equal(await rows().count(), 300);
  assert.equal(new Set(await eventIds()).size, 300, "aucun doublon entre les pages");
  await page.getByLabel("Décision").selectOption("deny");
  const denied = await rows().count(); assert.ok(denied > 0 && denied < 300);
  assert.equal(await rows().filter({ hasText: "Refusé" }).count(), denied);
  assert.match(await summary(), new RegExp(`^${denied} événements affichés sur 300 chargés`));
  await button("Réinitialiser").click();
  await geometry("journal", [1440, 390, 320], { shots: false });
  before = requests.length;
  await rows().filter({ hasText: a.session_owned_by_user_a1 }).first().getByRole("button", { name: "Limiter à cette ressource", exact: true }).click();
  await panel.getByText("Limité côté serveur à", { exact: false }).waitFor();
  await panel.locator(".sv-source", { hasText: "Journal" }).getByText("Lecture réussie", { exact: true }).waitFor();
  assert.deepEqual(recents(before), [`/api/v1/human/events/recent?limit=100&view=metadata&resource_kind=session&resource_id=${a.session_owned_by_user_a1}`]);
  assert.equal(await rows().count(), 100);
  assert.equal(await rows().filter({ hasText: a.session_owned_by_user_a1 }).count(), 100, "toutes les lignes appartiennent à la ressource limitée");
  assert.equal(await page.evaluate(() => document.activeElement?.classList.contains("sv-summary")), true);

  // Event -> session -> workers of the unified view, and back from a worker to its session in the Journal.
  await rows().first().getByRole("button", { name: "Voir les workers de cette session", exact: true }).click();
  await heading("Supervision du tenant.").waitFor(); await ready100();
  assert.equal(await page.getByLabel("Rechercher dans les lignes chargées").inputValue(), a.session_owned_by_user_a1);
  assert.equal(await openStreams(), 0, "quitter le Journal termine son flux : plus aucun lot ne consomme de reçu");
  assert.equal(await page.getByLabel("Rythme d’actualisation").inputValue(), "0", "le lien Journal → Workers conserve le rythme choisi");
  await page.getByText("Actualisation manuelle : aucune lecture sans action de votre part.", { exact: true }).waitFor();
  const leftJournal = requests.length;
  assert.equal(await workerRows().count(), 1, "le worker qui déclare cette session");
  const dialog = page.getByRole("dialog");
  await workerRows().getByRole("button", { name: "Détails", exact: true }).click(); await dialog.waitFor();
  before = requests.length;
  await dialog.getByRole("button", { name: "Lire les événements de cette session", exact: true }).click();
  await dialog.locator(".sv-events li").first().waitFor();
  assert.deepEqual(humanReads(before), [`/api/v1/human/events/recent?limit=25&view=metadata&resource_kind=session&resource_id=${a.session_owned_by_user_a1}`]);
  await dialog.getByText(/25 événement\(s\) reçus, du plus récent au plus ancien · d’autres, plus anciens, existent/).waitFor();
  await dialog.getByRole("button", { name: "Ouvrir cette session dans le Journal", exact: true }).click();
  await heading("Journal des événements autorisés.").waitFor();
  await panel.getByText("Limité côté serveur à", { exact: false }).waitFor();
  assert.equal(await page.locator("dialog").count(), 0);
  assert.equal(streams(leftJournal).filter((path) => !path.includes("resource_id")).length, 0, "aucun flux global ne reste ouvert après avoir quitté le Journal");
  await button("Retirer la limite").click();
  await journalResult(/^100 événements affichés sur 100 chargés/).waitFor();
  await logout(); assert.equal(await page.locator(".sv").count(), 0);
  assert.equal(await openStreams(), 0, "la déconnexion termine le flux");

  // Another account reads its own authorized selection only, live included.
  recoveryStage = "journal_a2_selection";
  await login("user_a2"); await supervise("personnel", "tenant-a");
  const personalHead = await readHead(() => button("Journal").click());
  assert.ok(personalHead.items.every(({ metadata }) => metadata.resource.resource_id !== a.session_owned_by_user_a1));
  assert.ok(personalHead.items.every(({ metadata }) => !appended.includes(metadata.id) && !missed.includes(metadata.id)));
  await direct("Direct connecté").waitFor();
  const own = await rows().count(); assert.ok(own > 0 && own <= 100);
  let text = await panel.innerText();
  assert.ok(!text.includes(a.session_owned_by_user_a1), "aucun événement d’une session non partagée");
  const privateIds = (await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a1, count: 2 })).event_ids;
  const ownIds = (await command({ command: "append_event", resource_kind: "session", resource_id: a.session_owned_by_user_a2, count: 1 })).event_ids;
  await panel.getByText("1 événement(s) reçu(s) depuis la dernière lecture", { exact: true }).waitFor();
  text = await panel.innerText();
  assert.ok(!text.includes(a.session_owned_by_user_a1), "le direct ne livre que la sélection autorisée");
  assert.equal(await rows().count(), own + 1);
  const personalIds = await eventIds();
  assert.deepEqual(personalIds.slice(0, 1), ownIds);
  assert.ok([...appended, ...missed, ...privateIds].every((id) => !personalIds.includes(id)));
  await logout(); await noSecret("journal");
  assert.equal(guard.violations.length, 0);
  recoveryStage = "complete";
};

/** Keyboard only, eight destinations, reduced motion. */
const runKeyboard = async () => {
  const guard = await prepareRecoveryGuard();
  await page.emulateMedia({ reducedMotion: "reduce" });
  recoveryStage = "keyboard_context";
  await login("admin_a");
  const press = async (key, until, limit = 80) => {
    for (let count = 0; count < limit; count++) { if (await page.evaluate(until)) return; await page.keyboard.press(key); }
    assert.ok(await page.evaluate(until), `le clavier atteint la cible avec ${key}`);
  };
  await press("Tab", () => document.activeElement?.textContent === "Superviser administration · tenant-a");
  await page.keyboard.press("Enter"); await heading("Supervision du tenant.").waitFor(); await ready100();
  assert.equal(await page.evaluate(() => document.activeElement?.tagName), "H1", "le titre de la vue reçoit le focus à l’ouverture");
  recoveryStage = "keyboard_filters_and_details";
  await press("Tab", () => document.activeElement?.id === "sv-worker-search");
  await page.keyboard.type("running");
  assert.equal(await page.evaluate(() => document.activeElement?.id), "sv-worker-search", "la saisie garde le focus");
  const filtered = await workerRows().count(); assert.ok(filtered > 0 && filtered < 24);
  await press("Tab", () => document.activeElement?.getAttribute("data-focus-key") === "sv-flagged");
  await page.keyboard.press("Space");
  assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("aria-pressed")), "true", "Espace bascule le filtre et le focus y reste");
  await page.keyboard.press("Space");
  await press("Tab", () => (document.activeElement?.getAttribute("data-focus-key") ?? "").startsWith("sv-worker-"));
  const key = await page.evaluate(() => document.activeElement.getAttribute("data-focus-key"));
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog"); await dialog.waitFor();
  assert.equal(await page.evaluate(() => !!document.activeElement?.closest("dialog")), true, "le focus entre dans le détail");
  await press("Tab", () => document.activeElement?.getAttribute("data-focus-key") === "sv-session-read", 30);
  await page.keyboard.press("Enter"); await dialog.locator(".sv-events li, .sv-refusal").first().waitFor();
  assert.equal(await page.evaluate(() => !!document.activeElement?.closest("dialog")), true, "la lecture des événements garde le focus dans le détail");
  await page.keyboard.press("Escape"); await dialog.waitFor({ state: "detached" });
  assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("data-focus-key")), key, "Échap rend le focus à la ligne d’origine");
  // Eight destinations: the bar stays one scrollable row on small screens and the open one is visible.
  recoveryStage = "keyboard_compact_navigation";
  await geometry("keyboard", [390, 320], { shots: false });
  // The bar itself by keyboard: every destination that takes focus is entirely visible, clear of the edge
  // fade that signals more destinations. « Événements » is disabled without a selected resource.
  const tabState = () => page.evaluate(() => {
    const nav = document.querySelector(".human-nav"); const box = nav.getBoundingClientRect(); const node = document.activeElement; const rect = node.getBoundingClientRect();
    const more = nav.dataset.more ?? "";
    return { label: node.textContent, tab: node.classList.contains("human-tab"), left: Math.round(rect.left - box.left), right: Math.round(box.right - rect.right), more,
      visible: rect.left >= box.left - 1 && rect.right <= box.right + 1,
      clear: rect.left >= box.left + (/start|both/.test(more) ? 30 : 0) - 1 && rect.right <= box.right - (/end|both/.test(more) ? 30 : 0) + 1 };
  });
  for (const width of [768, 390, 320]) {
    await page.setViewportSize({ width, height: 900 }); await settle();
    const enabled = await page.locator(".human-nav .human-tab:not(:disabled)").allInnerTexts();
    assert.deepEqual(enabled, ["Contextes", "Supervision", "Journal", "Ressources", "Brokers", "Workers", "Fédération"]);
    await page.locator(".human-nav .human-tab").first().focus();
    for (const [key, order] of [["Tab", enabled], ["Shift+Tab", [...enabled].reverse()]]) {
      for (let index = 0; index < order.length; index++) {
        await settle(); const state = await tabState();
        assert.equal(state.label, order[index], `${width}px ${key} : le focus suit l’ordre des destinations`);
        assert.ok(state.tab && state.visible, `${width}px ${key} : la destination « ${state.label} » focalisée au clavier est entièrement visible (bords ${state.left}/${state.right} px)`);
        assert.ok(state.clear, `${width}px ${key} : « ${state.label} » focalisée n’est pas sous le fondu de bord (${state.more}, bords ${state.left}/${state.right} px)`);
        if (index < order.length - 1) await page.keyboard.press(key);
      }
    }
  }
  recoveryStage = "keyboard_federation";
  await press("Tab", () => document.activeElement?.textContent === "Fédération", 10);
  await page.keyboard.press("Enter");
  await heading("Workers et brokers du tenant.").waitFor();
  const bar = await page.evaluate(() => {
    const nav = document.querySelector(".human-nav").getBoundingClientRect(); const current = document.querySelector('.human-nav [aria-current="page"]').getBoundingClientRect();
    return { visible: current.left >= nav.left - 1 && current.right <= nav.right + 1, fits: document.documentElement.scrollWidth <= innerWidth,
      transition: getComputedStyle(document.querySelector(".human-tab")).transitionDuration, behavior: getComputedStyle(document.querySelector(".human-nav")).scrollBehavior };
  });
  assert.deepEqual(bar, { visible: true, fits: true, transition: "0s", behavior: "auto" }, "destination ouverte visible, aucun débordement, aucun mouvement animé");
  await page.setViewportSize({ width: 1440, height: 1050 });
  await logout();
  recoveryStage = "keyboard_network";
  assert.deepEqual(guard.violations, [], "le navigateur reste sur l’origine de la fixture");
  recoveryStage = "complete";
};

/** Dense registry: 300 real rows loaded in three pages, 200 rendered, filtering stays usable. A measure, not a qualification. */
const runLoad = async () => {
  await login("admin_a"); await supervise("administration", "tenant-a"); await ready100();
  for (let loaded = 2; loaded <= 3; loaded++) {
    await button("Charger 100 lignes de plus").click();
    await workerSummaryStarts(`${loaded * 100} workers`);
  }
  assert.equal(await workerRows().count(), 200, "200 lignes rendues sur 300 chargées");
  assert.equal(await workerSummary(), "300 workers affichés sur 300 chargés · fin du registre à cette lecture · 200 lignes rendues");
  await page.getByText("Actualisation automatique suspendue : la fenêtre dépasse deux pages. Utilisez Actualiser pour relire toute la fenêtre.", { exact: true }).waitFor();
  const lastRendered = await workerRows().nth(199).getByRole("button", { name: "Détails", exact: true }).getAttribute("data-focus-key");
  await button("Afficher 100 lignes de plus").focus(); await page.keyboard.press("Enter"); assert.equal(await workerRows().count(), 300);
  const firstNew = await workerRows().nth(200).getByRole("button", { name: "Détails", exact: true }).getAttribute("data-focus-key");
  const after = await focused();
  assert.ok(after.key === firstNew && after.key !== lastRendered && after.inView, `le focus continue sur la première ligne nouvellement rendue (constaté : ${JSON.stringify(after)})`);
  const search = page.getByLabel("Rechercher dans les lignes chargées");
  await search.click();
  const typed = await page.evaluate(async () => {
    const input = document.querySelector("#sv-worker-search"); const started = performance.now();
    for (const letter of "cancelled") { input.value += letter; input.dispatchEvent(new Event("input", { bubbles: true })); }
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return { milliseconds: Math.round(performance.now() - started), focused: document.activeElement === input };
  });
  assert.equal(typed.focused, true, "la saisie garde le focus avec 300 lignes chargées");
  const cancelled = await workerRows().count(); assert.ok(cancelled > 0 && cancelled <= 200);
  assert.equal(await workerRows().filter({ hasText: "cancelled" }).count(), cancelled);
  console.log(`load: 9 keystrokes filtered 300 loaded rows in ${typed.milliseconds} ms (measure only, not a qualification)`);
  await button("Réinitialiser").click();
  await geometry("load", [1440, 320], { shots: false });
  const before = requests.length;
  await button("Actualiser maintenant").click();
  await source("Workers").getByText("Actualisation…", { exact: true }).waitFor().catch(() => {});
  await source("Workers").getByText("Lecture réussie", { exact: true }).waitFor({ timeout: 60000 });
  assert.equal(humanReads(before).filter((path) => path.includes("/federation/workers")).length, 3, "une actualisation explicite relit les trois pages de la fenêtre");
  assert.equal(await workerSummary(), "300 workers affichés sur 300 chargés · fin du registre à cette lecture · 200 lignes rendues");
  await logout();
};

let failure = null;
try {
  await ({ journal: runJournal, keyboard: runKeyboard, load: runLoad, fleet: runFleet, partial: runPartial, correlation: runCorrelation, rights: runRights, revoked: runRevoked,
    isolation: runIsolation, expiry: runExpiry, resume: runResume, ttl: runTtl, focus: runFocus, regions: runRegions, details: runDetails, states: runStates,
    "resource-recovery": runResourceRecovery, "correlation-recovery": runCorrelationRecovery,
    "resource-stream-recovery": runResourceStreamRecovery, "resource-stream-late-denial": () => runResourceStreamRecovery(true),
    "resource-stream-visibility-race": runResourceStreamVisibilityRace, "journal-catch-up": runJournalCatchUp,
    "journal-older-recovery": runJournalOlderRecovery, "human-export": runHumanExport })[scenario]();
  assert.deepEqual(softFailures, [], "observations indépendantes du scénario");
  assert.deepEqual(errors, [], "aucune erreur JavaScript de page");
  console.log(`browser_supervision scenario=${scenario} PASS workers=${workers} events=${events} artifacts=${artifacts}`);
} catch (error) {
  failure = error;
  if (privateFailure) {
    let mayCapture = recoveryOrigin && page.url() === recoveryOrigin + "/";
    if (authenticatedFailure) {
      const dashboard = URL.parse(ready.dashboard);
      mayCapture = dashboard?.protocol === "http:" && dashboard.hostname === "127.0.0.1" &&
        page.url() === dashboard.origin + "/" &&
        await page.locator(".human-shell").isVisible().catch(() => false) &&
        await page.getByLabel("Credential personnel local", { exact: true }).count().catch(() => 1) === 0;
    }
    const snapshot = mayCapture ? await saveBrowserFailure(page, { directory: artifacts, secrets }) : null;
    console.error(`browser_supervision scenario=${scenario} FAIL stage=${guardedRecovery ? recoveryStage : "revocation"}`);
    if (snapshot) for (const file of snapshot.files) console.error(`artifact=${join(artifacts, snapshot.directory, file)}`);
  } else {
    if (softFailures.length) console.error(`observations indépendantes en échec avant l’arrêt :\n- ${softFailures.join("\n- ")}`);
    await page.screenshot({ path: join(artifacts, `supervision-${scenario}-failure.png`), fullPage: true }).catch(() => {});
    console.error(`browser_supervision scenario=${scenario} FAIL artifacts=${artifacts}`);
  }
} finally {
  await browser.close();
  if (stack.exitCode === null) { stack.stdin.write(`${JSON.stringify({ command: "stop" })}\n`); stack.stdin.end(); }
  await new Promise((resolve) => { const timer = setTimeout(() => { stack.kill("SIGKILL"); resolve(); }, 5000); stack.once("exit", () => { clearTimeout(timer); resolve(); }); if (stack.exitCode !== null) { clearTimeout(timer); resolve(); } });
}
if (failure) { if (!privateFailure) console.error(failure); process.exit(1); }
