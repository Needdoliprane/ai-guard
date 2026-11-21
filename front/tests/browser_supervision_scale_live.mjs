// Real Rust dashboard/gateway with synthetic registry rows in private test SQLite stores.
// The parent fixture passes its exact loopback origin, seeded IDs and credentials on stdin.
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { saveBrowserFailure } from "./browser_failure.mjs";
import { installBrowserGuard } from "./browser_live.mjs";

const mode = "synthetic_registry_scale";
const identityPath = "/api/v1/human/identity";
const fleetPath = "/api/v1/human/federation/workers";
const provenance = "tenant_federation_registry";
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

class EvidenceError extends Error {
  constructor(reason, diagnostic) {
    super(reason);
    this.diagnostic = diagnostic;
  }
}
function requireEvidence(condition, reason, diagnostic) {
  if (!condition) throw new EvidenceError(reason, diagnostic);
}

function observe(promise, reason, diagnostic) {
  return promise.catch(() => { throw new EvidenceError(reason, diagnostic); });
}

function partitionIds(value, count) {
  return Array.isArray(value) && value.length === count && new Set(value).size === count &&
    value.every((id) => typeof id === "string" && uuid.test(id));
}

export function parseScaleConfig(raw) {
  let config;
  let url;
  try { config = JSON.parse(raw); url = new URL(config.dashboard); }
  catch { throw new EvidenceError("Entrée privée de fixture invalide."); }
  requireEvidence(url.protocol === "http:" && url.hostname === "127.0.0.1" && url.port && config.dashboard === url.origin,
    "L’origine du dashboard doit être l’adresse HTTP loopback exacte de la fixture.");
  const validAccount = (account, tenant, workers, brokers) => account?.tenant === tenant &&
    typeof account.token === "string" && /^aigh1_[0-9a-f]{64}$/.test(account.token) &&
    partitionIds(account.workerIds, workers) && partitionIds(account.brokerIds, brokers);
  const { a, b } = config.accounts ?? {};
  requireEvidence(validAccount(a, "tenant-a", 120, 3) && validAccount(b, "tenant-b", 1, 1) && a.token !== b.token,
    "Les deux comptes de fixture et les registres attendus sont requis.");
  const allIds = [...a.workerIds, ...a.brokerIds, ...b.workerIds, ...b.brokerIds];
  requireEvidence(new Set(allIds).size === allIds.length,
    "Les identifiants attendus des deux registres doivent être distincts.");
  return { dashboard: url.origin, a, b };
}

async function readConfig() {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    requireEvidence(bytes <= 16384, "L’entrée privée de fixture dépasse la limite.");
    chunks.push(chunk);
  }
  return parseScaleConfig(Buffer.concat(chunks).toString("utf8"));
}

function sameIds(actual, expected) {
  const wanted = new Set(expected);
  return actual.length === wanted.size && new Set(actual).size === actual.length && actual.every((id) => wanted.has(id));
}

export function fleetProjection(page, context) {
  requireEvidence(page?.version === 1 && page.source === provenance && page.consistency === "independent_snapshots" &&
    page.coverage === "partial" && page.runtime_inventory === "unknown" && !Object.hasOwn(page, "total"),
  "Le registre composé doit indiquer explicitement sa provenance, sa portée partielle et l’inventaire inconnu.");
  const legs = [page.workers, page.brokers];
  for (const leg of legs) {
    requireEvidence(leg?.context_id === context.id && leg.authorization_version === context.authorization_version &&
      leg.runtime_inventory === "unknown" && !Object.hasOwn(leg, "total") && Array.isArray(leg.items) && leg.items.length <= 100 &&
      (leg.has_more === true ? typeof leg.next_cursor === "string" && !!leg.next_cursor : leg.has_more === false && leg.next_cursor === null) &&
      typeof leg.read_receipt?.audit_event_id === "string" && uuid.test(leg.read_receipt.audit_event_id),
    "Un registre est incomplet ou son reçu ne correspond pas au contexte courant.");
  }
  return {
    workerIds: page.workers.items.map((item) => item?.worker_id),
    brokerIds: page.brokers.items.map((item) => item?.broker_id),
    nextWorker: page.workers.next_cursor, nextBroker: page.brokers.next_cursor,
    receipts: legs.map((leg) => leg.read_receipt.audit_event_id),
  };
}

async function run(page, config, result) {
  const button = (name) => page.getByRole("button", { name, exact: true });
  const heading = (name) => page.getByRole("heading", { name, exact: true });
  let reads = 0;
  const isGet = (request, pathname) => {
    const url = new URL(request.url());
    return url.origin === config.dashboard && url.pathname === pathname && request.method() === "GET";
  };
  page.on("request", (request) => {
    if (isGet(request, fleetPath)) reads++;
  });
  const waitRead = (pathname) => page.waitForResponse((response) => isGet(response.request(), pathname));
  const workerPanel = page.locator(".sv-panel").filter({ has: heading("Workers") });
  const brokerPanel = page.locator(".sv-panel").filter({ has: heading("Brokers et autorités") });
  const workerSummary = (text) => workerPanel.getByText(text, { exact: true });
  const rowIds = (panel) => panel.locator("tbody tr > td:first-child .sv-id").allTextContents();
  const noForeign = async (account) => {
    const text = await page.locator("body").textContent();
    return [...account.workerIds, ...account.brokerIds].every((id) => !text.includes(id));
  };

  async function login(account, name, foreign) {
    delete result.identityHttp;
    const submit = async () => {
      await page.getByLabel("Type d’accès").selectOption("human");
      await page.getByLabel("Credential personnel local").fill(account.token);
      await button("Ouvrir mon espace").click();
    };
    const [response] = await Promise.all([
      observe(waitRead(identityPath), "La requête d’identité locale n’a donné aucune réponse.", "identity_missing"),
      observe(submit(), "Le formulaire de connexion locale n’a pas pu être soumis.", "login_form"),
    ]);
    result.identityHttp = response.status();
    requireEvidence(result.identityHttp === 200, "L’identité locale de la fixture a été refusée.", "identity_http");
    const identity = await observe(response.json(), "La réponse d’identité n’est pas un JSON valide.", "identity_json");
    requireEvidence(identity?.version === 1 && identity.display_name === name && identity.authentication?.method === "local_credential" &&
      identity.contexts_complete === true && Array.isArray(identity.contexts) &&
      identity.contexts.every((context) => context?.tenant_id === account.tenant),
    "L’identité n’a pas le nom ou le périmètre du compte de fixture.", "identity_scope");
    const administrations = identity.contexts.filter((context) => context.mode === "administration");
    const context = administrations[0];
    requireEvidence(administrations.length === 1 && typeof context?.id === "string" && !!context.id &&
      /^[0-9a-f]{64}$/.test(context.authorization_version) &&
      ["read_worker_registry", "read_broker_registry", "read_metadata"].every((capability) => context.capabilities?.includes(capability)),
    "Le serveur n’a pas proposé le contexte d’administration exact pour les deux registres.", "administration_scope");
    await observe(Promise.all([
      heading("Choisir un contexte.").waitFor(),
      page.locator(".authenticated-scope").and(page.getByText(name, { exact: true })).waitFor(),
    ]), "La sélection des contextes n’a pas rendu l’identité locale attendue.", "context_render");
    requireEvidence(await button(`Superviser administration · ${foreign.tenant}`).count() === 0,
      "Une action de supervision d’un autre tenant est proposée.", "context_render");
    return context;
  }

  async function readFleet(action, context, cursor = null) {
    delete result.fleetHttp;
    const [response] = await observe(Promise.all([waitRead(fleetPath), action.click()]),
      "L’action de lecture du registre composé n’a donné aucune réponse.", "fleet_response_missing");
    result.fleetHttp = response.status();
    requireEvidence(result.fleetHttp === 200, "La lecture du registre composé a échoué.", "fleet_http");
    const { searchParams } = new URL(response.url());
    requireEvidence(searchParams.size === (cursor === null ? 2 : 3) && searchParams.get("worker_limit") === "100" &&
      searchParams.get("broker_limit") === "100" && searchParams.get("worker_cursor") === cursor,
    "La requête n’utilise pas la limite par défaut et le curseur exact retourné par le contrôle.", "fleet_query");
    return fleetProjection(await response.json(), context);
  }

  async function firstView(account, context) {
    const first = await readFleet(button(`Superviser administration · ${account.tenant}`), context);
    await heading("Supervision du tenant.").waitFor();
    for (const text of ["Workers", "Brokers", "Ressources"]) {
      await page.locator(".sv-source").filter({ has: page.getByText(text, { exact: true }) })
        .getByText("Lecture réussie", { exact: true }).waitFor();
    }
    await page.getByText("Couverture partielle", { exact: true }).waitFor();
    await page.getByText("Inventaire runtime : inconnu. Ces registres ne prouvent ni exécution, ni arrêt, ni libération de quota.", { exact: true }).waitFor();
    requireEvidence(first.nextBroker === null && sameIds(first.brokerIds, account.brokerIds) && sameIds(await rowIds(brokerPanel), first.brokerIds),
      "Les lignes Brokers affichées diffèrent du registre autorisé.");
    return first;
  }

  async function logout(account) {
    await button("Se déconnecter").click();
    await page.getByLabel("Credential personnel local").waitFor();
    requireEvidence(await page.locator(".sv, dialog, .authenticated-scope").count() === 0 && await noForeign(account),
      "La déconnexion n’a pas retiré l’ancien registre de la page.");
  }

  result.stage = "login_a";
  const document = await observe(page.goto(config.dashboard + "/"),
    "Impossible de naviguer vers le dashboard de fixture.", "dashboard_document");
  if (document) result.dashboardHttp = document.status();
  requireEvidence(result.dashboardHttp === 200, "Le document du dashboard de fixture est indisponible.", "dashboard_document");
  const contextA = await login(config.a, "Supervision test administrator A", config.b);
  result.stage = "first_page_a";
  const first = await firstView(config.a, contextA);
  const loadedA = new Set(config.a.workerIds);
  requireEvidence(partitionIds(first.workerIds, 100) && first.workerIds.every((id) => loadedA.has(id)) && first.nextWorker !== null &&
    sameIds(await rowIds(workerPanel), first.workerIds) && await noForeign(config.b),
  "La première page n’affiche pas les cent workers exacts du tenant A.");
  await workerSummary("100 workers affichés sur 100 chargés · d’autres pages existent au contrôle, aucun total du tenant n’est connu").waitFor();

  result.stage = "next_page_a";
  const second = await readFleet(button("Charger 100 lignes de plus"), contextA, first.nextWorker);
  const aWorkers = [...first.workerIds, ...second.workerIds];
  await workerSummary("120 workers affichés sur 120 chargés · fin du registre à cette lecture").waitFor();
  requireEvidence(sameIds(aWorkers, config.a.workerIds) && sameIds(await rowIds(workerPanel), aWorkers) && second.nextWorker === null &&
    sameIds(second.brokerIds, config.a.brokerIds) && sameIds(await rowIds(brokerPanel), second.brokerIds) &&
    await button("Charger 100 lignes de plus").count() === 0 && await noForeign(config.b),
  "Le chargement suivant n’a pas produit la fenêtre exacte de 120 lignes sans doublon.");

  result.stage = "filter_a";
  const searched = second.workerIds[0];
  const requestsBeforeFilter = reads;
  const search = workerPanel.getByLabel("Rechercher dans les lignes chargées", { exact: true });
  await search.fill(searched);
  await workerSummary("1 worker affiché sur 120 chargés · fin du registre à cette lecture · recherche et filtres appliqués aux seules lignes chargées").waitFor();
  requireEvidence(sameIds(await rowIds(workerPanel), [searched]) && reads === requestsBeforeFilter,
    "Le filtrage local ne correspond pas à la ligne de la seconde page.");
  await workerPanel.getByRole("button", { name: "Détails", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByText(searched, { exact: true }).waitFor();
  await dialog.getByText(/2 reçu\(s\) durable\(s\) pour cette fenêtre/).waitFor();
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });

  result.stage = "logout_a";
  await logout(config.a);
  result.stage = "login_b";
  const contextB = await login(config.b, "Supervision test administrator B", config.a);
  result.stage = "first_page_b";
  const onlyB = await firstView(config.b, contextB);
  await workerSummary("1 worker affiché sur 1 chargé · fin du registre à cette lecture").waitFor();
  requireEvidence(onlyB.nextWorker === null && sameIds(onlyB.workerIds, config.b.workerIds) &&
    sameIds(await rowIds(workerPanel), onlyB.workerIds) && await noForeign(config.a) && await search.inputValue() === "" &&
    new Set([...first.receipts, ...second.receipts, ...onlyB.receipts]).size === 6 && reads === 3,
  "Le tenant B n’a pas exactement son propre registre ou les lectures ne sont pas distinctes.");
  result.stage = "logout_b";
  await logout(config.b);
  return { status: "PASS", mode, stage: "complete", firstPageA: first.workerIds.length, aWorkers: aWorkers.length,
    aBrokers: second.brokerIds.length, bWorkers: onlyB.workerIds.length, bBrokers: onlyB.brokerIds.length,
    pagesA: 2, visibleFilteredA: 1, provenance, coverage: "partial", runtimeInventory: "unknown", isolation: true };
}

async function main() {
  const result = { mode, stage: "configuration" };
  let config;
  let browser;
  let context;
  let page;
  let guard;
  let report;
  try {
    requireEvidence(!process.env.DEBUG && !process.env.PWDEBUG && !process.env.NODE_DEBUG,
      "La recette exige des journaux de débogage désactivés.");
    config = await readConfig();
    result.stage = "chromium";
    const require = createRequire(import.meta.url);
    let playwright;
    try { playwright = require("playwright"); }
    catch {
      const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
      if (modules) playwright = require(join(modules, "playwright"));
    }
    requireEvidence(playwright?.chromium, "Playwright Chromium est requis.");
    browser = await playwright.chromium.launch({ headless: true,
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
      args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP * ~NOTFOUND"] });
    context = await browser.newContext({ serviceWorkers: "block", viewport: { width: 1440, height: 900 } });
    guard = await installBrowserGuard(context, [config.dashboard], [config.a.token, config.b.token]);
    page = await context.newPage();
    page.setDefaultTimeout(12000);
    await guard.preparePage(page);
    report = await run(page, config, result);
    requireEvidence(guard.violations.length === 0, "Une requête hors du parcours autorisé a été bloquée.");
  } catch (error) {
    const artifacts = page && page.url() === config.dashboard + "/" ? await saveBrowserFailure(page, {
      directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR, secrets: [config.a.token, config.b.token],
    }) : null;
    const evidenceError = error instanceof EvidenceError ? error : null;
    const diagnostic = guard?.violations.length ? "browser_route_blocked" : evidenceError?.diagnostic;
    report = { ...result, status: result.stage === "chromium" ? "BLOCKED" : "FAIL", ...(diagnostic ? { diagnostic } : {}),
      reason: evidenceError?.message ?? "L’étape ne s’est pas terminée.", ...(artifacts ? { artifacts } : {}) };
  } finally {
    await context?.close().catch(() => {});
    await browser?.close().catch(() => {});
  }
  console.log(JSON.stringify(report));
  process.exitCode = report.status === "PASS" ? 0 : report.status === "BLOCKED" ? 2 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
