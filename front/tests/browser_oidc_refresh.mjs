// Signed synthetic OIDC issuer, real Rust gateway/dashboard and browser timers.
// The Rust fixture provisions the account, starts the servers and verifies the durable audit.
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { saveBrowserFailure } from "./browser_failure.mjs";

const mode = "oidc_signed_fixture_refresh";
const providerLabel = "Controlled issuer";
const displayName = "Named OIDC reader";
const tenant = "tenant-a";
const adminResource = "fixture-admin-only-session";
const personalResource = "fixture-personal-session";
const oidcStatus = "/auth/oidc/status";
const callback = "/auth/oidc/callback";
const complete = "/auth/oidc/complete";
const tokens = "/auth/oidc/tokens";
const refresh = "/auth/oidc/refresh";
const identity = "/api/v1/human/identity";
const resources = "/api/v1/human/resources";
const stream = "/api/v1/human/stream";
const authorization = "/realm/authorize";
const redirectStatuses = new Set([301, 302, 303, 307, 308]);
const publicErrorCodes = new Set(["origin_denied", "oidc_not_configured", "identity_unavailable",
  "audit_unavailable", "invalid_request", "capacity_exhausted"]);

class EvidenceError extends Error {
  constructor(reason, diagnostic) {
    super(reason);
    this.diagnostic = diagnostic;
  }
}
function requireEvidence(condition, reason, diagnostic) {
  if (!condition) throw new EvidenceError(reason, diagnostic);
}

function loopbackUrl(raw, base) {
  try {
    const url = new URL(raw, base);
    return url.protocol === "http:" && url.hostname === "127.0.0.1" && url.port &&
      !url.username && !url.password && !url.hash ? url : null;
  } catch {
    return null;
  }
}

export function readRefreshConfig(env) {
  const dashboard = loopbackUrl(env.AI_GUARD_TEST_OIDC_DASHBOARD);
  const issuer = loopbackUrl(env.AI_GUARD_TEST_OIDC_ISSUER);
  requireEvidence(dashboard && dashboard.href === dashboard.origin + "/" &&
    [dashboard.origin, dashboard.href].includes(env.AI_GUARD_TEST_OIDC_DASHBOARD),
  "Le dashboard doit être une origine HTTP de fixture sur 127.0.0.1 avec port explicite.");
  requireEvidence(issuer && issuer.origin !== dashboard.origin &&
    env.AI_GUARD_TEST_OIDC_ISSUER === issuer.origin + "/realm",
  "L’émetteur signé doit être le realm de fixture sur un autre port de 127.0.0.1.");
  return { dashboard, issuer };
}

export function validAuthorization(url, config) {
  const params = url.searchParams;
  const single = (key) => params.getAll(key).length === 1 && params.get(key);
  return url.origin === config.issuer.origin && url.pathname === authorization &&
    single("client_id") === "web-public" && single("response_type") === "code" &&
    single("redirect_uri") === config.dashboard.origin + callback &&
    single("code_challenge_method") === "S256" &&
    /^[A-Za-z0-9_-]{43}$/.test(single("code_challenge") || "") && Boolean(single("state"));
}

export function requestAllowed(raw, method, config) {
  const url = loopbackUrl(raw);
  if (!url) return false;
  if (url.origin === config.issuer.origin) return method === "GET" && validAuthorization(url, config);
  if (url.origin !== config.dashboard.origin) return false;
  if (method === "POST") return !url.search && ["/auth/oidc/start", tokens, refresh].includes(url.pathname);
  if (method !== "GET") return false;
  if ([callback, resources, stream].includes(url.pathname)) return true;
  if (url.pathname === "/favicon.svg") return ["", "?v=ovpt-20261030"].includes(url.search);
  return !url.search && (["/", complete, oidcStatus, identity, "/api/v1/status", "/api/v1/gateway/status"].includes(url.pathname) ||
    /^\/assets\/[\w.-]+$/.test(url.pathname));
}

export function redirectAllowed(raw, location, method, status, config) {
  const source = loopbackUrl(raw);
  const target = location && loopbackUrl(location, source?.href);
  if (!source || !target || method !== "GET" || status !== 303 || !requestAllowed(target.href, method, config)) return false;
  return (validAuthorization(source, config) && target.origin === config.dashboard.origin && target.pathname === callback) ||
    (source.origin === config.dashboard.origin && source.pathname === callback && target.href === config.dashboard.origin + complete);
}

async function guardBrowser(context, page, config, violations) {
  requireEvidence(typeof context.routeWebSocket === "function", "Playwright avec interception WebSocket est requis.");
  await context.routeWebSocket("**/*", async (socket) => { violations.push("websocket"); await socket.close(); });
  await context.route("**/*", async (route) => {
    const request = route.request();
    let requestPage;
    try { requestPage = request.frame().page(); } catch { /* No page for a worker request. */ }
    if (requestPage === page && requestAllowed(request.url(), request.method(), config)) await route.continue();
    else { violations.push("requête"); await route.abort("blockedbyclient"); }
  });
  // Playwright routes the first hop only. Inspect redirect targets before Chromium follows them.
  const cdp = await context.newCDPSession(page);
  cdp.on("Fetch.requestPaused", async (event) => {
    try {
      if (redirectStatuses.has(event.responseStatusCode)) {
        const location = event.responseHeaders?.find(({ name }) => name.toLowerCase() === "location")?.value;
        if (!redirectAllowed(event.request.url, location, event.request.method, event.responseStatusCode, config)) {
          violations.push("redirection");
          await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "BlockedByClient" });
          return;
        }
      }
      await cdp.send("Fetch.continueRequest", { requestId: event.requestId });
    } catch {
      if (!page.isClosed() && !event.responseErrorReason) violations.push("interception");
      await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "BlockedByClient" }).catch(() => {});
    }
  });
  await cdp.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Response" }] });
}

function resourceProjection(value, administrator) {
  const expected = administrator ? [adminResource, personalResource] : [personalResource];
  return value?.version === 1 && value.coverage === "authorized_selection" && value.has_more === false &&
    Array.isArray(value.items) && value.items.length === expected.length &&
    expected.every((id) => value.items.some((item) => item?.resource?.kind === "session" &&
      item.resource.resource_id === id && item?.permissions?.metadata === true));
}

function identityProjection(value, issuer, administrator) {
  const expected = administrator ? ["administration", "personal"] : ["personal"];
  return value?.version === 1 && value.contexts_complete === true && value.display_name === displayName &&
    value.authentication?.method === "oidc_access_token" && value.principal?.kind === "human_oidc" &&
    value.principal.issuer === issuer.href && Array.isArray(value.contexts) && value.contexts.length === expected.length &&
    expected.every((mode) => value.contexts.some((context) => context?.tenant_id === tenant && context.mode === mode &&
      Array.isArray(context.capabilities) && context.capabilities.includes("read_metadata") &&
      context.capabilities.includes("read_tenant_metadata") === (mode === "administration")));
}

function dashboardTraffic(page, config) {
  const responses = [];
  const streamRequests = new Set();
  let resourceRequests = 0;
  let resourceRequestsAtRenewal;
  let streamStarts = 0;
  let streamStartsAtRenewal;
  const isRequest = (request, path, method = "GET") => {
    const url = new URL(request.url());
    return url.origin === config.dashboard.origin && url.pathname === path && request.method() === method;
  };
  page.on("response", (response) => {
    for (const [path, method] of [[callback, "GET"], [complete, "GET"], [tokens, "POST"], [refresh, "POST"]]) {
      if (!isRequest(response.request(), path, method)) continue;
      responses.push({ path, status: response.status() });
      if (path === refresh) {
        resourceRequestsAtRenewal = resourceRequests;
        streamStartsAtRenewal = streamStarts;
      }
    }
  });
  page.on("request", (request) => {
    if (isRequest(request, resources)) resourceRequests++;
    if (isRequest(request, stream)) { streamRequests.add(request); streamStarts++; }
  });
  page.on("requestfailed", (request) => streamRequests.delete(request));
  page.on("requestfinished", (request) => streamRequests.delete(request));
  return {
    responses, streamRequests,
    waitFor: (path, method = "GET", timeout) => page.waitForResponse((response) => isRequest(response.request(), path, method), { timeout }),
    noReadsAfterRenewal: () => resourceRequests === resourceRequestsAtRenewal && streamStarts === streamStartsAtRenewal,
  };
}

export async function checkPublicStatus(response, config, result) {
  result.publicStatusHttp = response.status();
  const status = await response.json().catch(() => null);
  if (!response.ok() && publicErrorCodes.has(status?.error?.code)) result.publicStatusError = status.error.code;
  requireEvidence(response.ok(), "État SSO indisponible.", "public_status_http");
  requireEvidence(status && typeof status === "object" && !Array.isArray(status),
    "La route d’état SSO n’a pas donné une réponse JSON valide.", "public_status_json");
  requireEvidence(status.version === 1, "Version de l’état SSO inattendue.", "public_status_version");
  requireEvidence(status.enabled === true, "Le SSO n’est pas actif dans la fixture.", "public_status_disabled");
  requireEvidence(status.provider_label === providerLabel, "Le nom public du fournisseur ne correspond pas à la fixture.", "public_status_provider");
  requireEvidence(status.issuer === config.issuer.href, "L’émetteur de l’état SSO ne correspond pas à la fixture.", "public_status_issuer");
}

async function run(browser, config, result) {
  const context = await browser.newContext({ serviceWorkers: "block" });
  let page;
  let captureAllowed = false;
  const violations = [];
  try {
    page = await context.newPage();
    page.setDefaultTimeout(10000);
    await guardBrowser(context, page, config, violations);
    const traffic = dashboardTraffic(page, config);
    const heading = (name) => page.getByRole("heading", { name, exact: true });
    const button = (name) => page.getByRole("button", { name, exact: true });
    const scope = (name) => page.locator(".authenticated-scope").and(page.getByText(name, { exact: true }));
    const resource = (id) => page.locator(".human-resource-row").getByText(id, { exact: true });
    const admin = button("Ouvrir administration · " + tenant);
    const personal = button("Ouvrir personnel · " + tenant);
    const signIn = button("Se connecter avec " + providerLabel);
    const clearedStorage = () => page.evaluate(() => localStorage.length === 0 && sessionStorage.length === 0);

    result.stage = "public_status";
    const [publicStatus] = await Promise.all([
      traffic.waitFor(oidcStatus).catch(() => {
        throw new EvidenceError("Aucune réponse reçue pour la route d’état SSO.", "public_status_missing");
      }),
      page.goto(config.dashboard.href).catch(() => {
        throw new EvidenceError("La navigation vers le dashboard n’a pas abouti.", "dashboard_navigation_error");
      }).then((response) => {
        if (response) result.dashboardHttp = response.status();
        requireEvidence(response?.status() === 200, "Le document du dashboard est indisponible.", "dashboard_navigation_http");
      }),
    ]);
    await checkPublicStatus(publicStatus, config, result);

    result.stage = "login";
    const [initial] = await Promise.all([traffic.waitFor(identity), signIn.click()]);
    requireEvidence(initial.status() === 200, "Première identité OIDC refusée.");
    requireEvidence(identityProjection(await initial.json(), config.issuer, true), "Permissions initiales inattendues.");
    await page.waitForURL(config.dashboard.href);
    await heading("Choisir un contexte.").waitFor();
    await scope(displayName).waitFor();
    await admin.waitFor();
    await personal.waitFor();
    requireEvidence(JSON.stringify(traffic.responses) === JSON.stringify([
      { path: callback, status: 303 }, { path: complete, status: 200 }, { path: tokens, status: 200 },
    ]), "Callback, page de fin et remise unique de session non confirmés.");
    requireEvidence(await clearedStorage(), "État temporaire SSO encore présent après connexion.");
    captureAllowed = true;

    result.stage = "admin_read";
    const [adminRead] = await Promise.all([traffic.waitFor(resources), admin.click()]);
    requireEvidence(adminRead.status() === 200 && resourceProjection(await adminRead.json(), true),
      "Le contrôle ne fournit pas les deux ressources à l’administration initiale.");
    await heading("Ressources autorisées.").waitFor();
    await scope(`${displayName} · Administration · ${tenant}`).waitFor();
    await resource(adminResource).waitFor();
    await resource(personalResource).waitFor();
    await page.getByText("Flux autorisé connecté", { exact: true }).waitFor();

    result.stage = "scheduled_refresh";
    const oldStreams = new Set(traffic.streamRequests);
    requireEvidence(oldStreams.size > 0 && !traffic.responses.some(({ path }) => path === refresh),
      "Le renouvellement doit démarrer après l’observation du contexte d’administration et de son flux.");
    const [, updated] = await Promise.all([
      traffic.waitFor(refresh, "POST", 35000).then((response) => {
        requireEvidence(response.status() === 200, "Le renouvellement planifié de la session a échoué.");
      }),
      traffic.waitFor(identity, "GET", 35000),
    ]);
    requireEvidence(updated.status() === 200, "La lecture d’identité après renouvellement a échoué.");

    result.stage = "admin_purged";
    requireEvidence(identityProjection(await updated.json(), config.issuer, false), "Le contrôle propose encore un droit inattendu après renouvellement.");
    await heading("Choisir un contexte.").waitFor();
    await scope(displayName).waitFor();
    await personal.waitFor();
    await resource(adminResource).waitFor({ state: "detached" });
    requireEvidence(await admin.count() === 0 && await heading("Administration · " + tenant).count() === 0 &&
      await page.getByRole("button", { name: /^(Reprendre |Superviser administration)/ }).count() === 0 &&
      await page.locator(".human-resource-panel, dialog").count() === 0,
    "Le contexte d’administration, un accès de reprise ou ses données restent dans le DOM.");
    requireEvidence(traffic.noReadsAfterRenewal(), "Une lecture a été lancée avant le choix du nouveau contexte.");
    requireEvidence(await clearedStorage(), "Une donnée SSO subsiste dans le stockage Web après renouvellement.");

    result.stage = "personal_read";
    const [personalRead] = await Promise.all([traffic.waitFor(resources), personal.click()]);
    requireEvidence(personalRead.status() === 200 && resourceProjection(await personalRead.json(), false),
      "La lecture personnelle n’a pas conservé uniquement sa ressource autorisée.");
    await heading("Ressources autorisées.").waitFor();
    await scope(`${displayName} · Personnel · ${tenant}`).waitFor();
    await resource(personalResource).waitFor();
    requireEvidence(await resource(adminResource).count() === 0 &&
      traffic.responses.filter(({ path }) => path === refresh).length === 1 &&
      traffic.responses.filter(({ path }) => path === tokens).length === 1,
    "La ressource d’administration est revenue ou la connexion a été redémarrée.");
    requireEvidence([...traffic.streamRequests].every((request) => !oldStreams.has(request)),
      "Le flux du contexte d’administration est encore ouvert.");
    requireEvidence(violations.length === 0, "Le navigateur a tenté une requête hors du parcours autorisé.");
    return { ...result, status: "PASS", stage: "complete", adminPurge: true, personalRead: true, oldStreamClosed: true };
  } catch (error) {
    // Never capture the fixture issuer, callback, completion page or pre-redemption storage.
    const artifacts = captureAllowed && page?.url() === config.dashboard.href ?
      await saveBrowserFailure(page, { directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR }) : null;
    const evidenceError = error instanceof EvidenceError ? error : null;
    const diagnostic = violations.length ? "browser_route_blocked" : evidenceError?.diagnostic;
    return { ...result, status: "FAIL", ...(diagnostic ? { diagnostic } : {}),
      reason: violations.length ? "Requête hors du parcours autorisé." : evidenceError?.message ?? "L’étape n’a pas abouti.",
      ...(artifacts ? { artifacts } : {}) };
  } finally {
    await context.close().catch(() => {});
  }
}

async function main() {
  const result = { mode, stage: "configuration" };
  let browser;
  let report;
  try {
    const config = readRefreshConfig(process.env);
    requireEvidence(!process.env.DEBUG && !process.env.PWDEBUG && !process.env.NODE_DEBUG,
      "Les journaux réseau et Playwright doivent rester désactivés pour cette recette.");
    if (process.argv.includes("--check-config")) {
      report = { ...result, status: "PASS", scope: "configuration_only" };
    } else {
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
      report = await run(browser, config, result);
    }
  } catch (error) {
    report = { ...result, status: result.stage === "chromium" ? "BLOCKED" : "FAIL",
      reason: error instanceof EvidenceError ? error.message : "La recette n’a pas pu démarrer." };
  } finally {
    await browser?.close().catch(() => {});
  }
  console.log(JSON.stringify(report));
  process.exitCode = report.status === "PASS" ? 0 : report.status === "BLOCKED" ? 2 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
