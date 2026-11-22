// Real Keycloak discovery Web path. Use a freshly prepared, workstation-local DEMO only.
import { once } from "node:events";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { saveBrowserFailure } from "./browser_failure.mjs";

const demoHost = "keycloak.ai-guard.test";
const realm = "/realms/ai-guard-demo";
const authorizationPath = realm + "/protocol/openid-connect/auth";
const callback = "/auth/oidc/callback";
const complete = "/auth/oidc/complete";
const oidcStatus = "/auth/oidc/status";
const identity = "/api/v1/human/identity";
const resources = "/api/v1/human/resources";
const accounts = ["admin-ai-guard", "user-1", "user-2", "admin"];
const tenant = "ai-guard-demo";
const dashboardHosts = ["127.0.0.1", "[::1]", "localhost"];
const hostResolverRules = "EXCLUDE 127.0.0.1, EXCLUDE ::1, MAP keycloak.ai-guard.test 127.0.0.1, MAP localhost 127.0.0.1, MAP * ~NOTFOUND";
const redirectStatuses = new Set([301, 302, 303, 307, 308]);
const dashboardPosts = new Set(["/auth/oidc/start", "/auth/oidc/tokens", "/auth/oidc/refresh", "/auth/oidc/logout"]);

class EvidenceError extends Error {}
function requireEvidence(condition, message) {
  if (!condition) throw new EvidenceError(message);
}

function approvedUrl(raw, allowTls) {
  try {
    const url = new URL(raw);
    return !url.username && !url.password && !url.hash &&
      (url.protocol === "http:" || allowTls && url.protocol === "https:") ? url : null;
  } catch {
    return null;
  }
}

export function readDiscoveryConfig(env) {
  const allowTls = env.AI_GUARD_TEST_OIDC_ALLOW_TLS === "1";
  const dashboard = approvedUrl(env.AI_GUARD_TEST_DASHBOARD, allowTls);
  const issuer = approvedUrl(env.AI_GUARD_TEST_OIDC_ISSUER, allowTls);
  requireEvidence(dashboard && dashboardHosts.includes(dashboard.hostname) && dashboard.href === dashboard.origin + "/",
    "AI_GUARD_TEST_DASHBOARD doit être une origine loopback exacte.");
  requireEvidence(issuer && issuer.href === issuer.origin + realm &&
    (dashboardHosts.includes(issuer.hostname) || issuer.hostname === demoHost) && issuer.origin !== dashboard.origin,
    "AI_GUARD_TEST_OIDC_ISSUER doit désigner exactement le realm DEMO sur loopback ou son alias local.");
  const selected = env.AI_GUARD_TEST_OIDC_ACCOUNTS?.split(",") ?? accounts;
  requireEvidence(selected.length > 0 && new Set(selected).size === selected.length &&
    selected.every((account) => accounts.includes(account)), "AI_GUARD_TEST_OIDC_ACCOUNTS contient un compte inconnu ou répété.");
  return { dashboard, issuer, allowTls, selected };
}

export function discoveryRequestAllowed(raw, method, config) {
  const url = approvedUrl(raw, config.allowTls);
  if (!url) return false;
  if (url.origin === config.dashboard.origin) {
    if (method === "POST") return !url.search && dashboardPosts.has(url.pathname);
    if (method !== "GET") return false;
    if (url.pathname === callback) return true;
    if (url.pathname.startsWith("/api/v1/human/")) return true;
    if (url.pathname === "/favicon.svg") return url.search === "" || url.search === "?v=ovpt-20261030";
    return !url.search && (["/", complete, oidcStatus, "/api/v1/status",
      "/api/v1/gateway/status"].includes(url.pathname) || /^\/assets\/[\w.-]+$/.test(url.pathname));
  }
  if (url.origin !== config.issuer.origin) return false;
  if (method === "POST") return url.pathname === realm + "/login-actions/authenticate";
  return method === "GET" && (url.pathname.startsWith(realm + "/") || url.pathname.startsWith("/resources/"));
}

export function validAuthorization(raw, config) {
  const url = approvedUrl(raw, config.allowTls);
  if (!url || url.origin !== config.issuer.origin || url.pathname !== authorizationPath) return false;
  const params = url.searchParams;
  const single = (name) => params.getAll(name).length === 1 && params.get(name);
  return single("client_id") === "ai-guard-demo-web" && single("response_type") === "code" &&
    single("redirect_uri") === config.dashboard.origin + callback && single("code_challenge_method") === "S256" &&
    /^[A-Za-z0-9_-]{43}$/.test(single("code_challenge") || "") && Boolean(single("state"));
}

export function discoveryRedirectAllowed(from, location, method, status, config) {
  if (!location || !redirectStatuses.has(status)) return false;
  const source = approvedUrl(from, config.allowTls);
  if (!source) return false;
  let target;
  try { target = new URL(location, source); } catch { return false; }
  const nextMethod = status === 303 || ((status === 301 || status === 302) && method === "POST") ? "GET" : method;
  if (!discoveryRequestAllowed(target.href, nextMethod, config)) return false;
  if (source.origin === target.origin) return true;
  if (nextMethod !== "GET") return false;
  return (source.origin === config.issuer.origin && target.origin === config.dashboard.origin && target.pathname === callback) ||
    (source.origin === config.dashboard.origin && validAuthorization(target.href, config));
}

async function guardBrowser(context, page, config) {
  const violations = [];
  requireEvidence(typeof context.routeWebSocket === "function", "Playwright avec interception WebSocket est requis.");
  await context.routeWebSocket("**/*", async (socket) => {
    violations.push("websocket");
    await socket.close();
  });
  await context.route("**/*", async (route) => {
    const request = route.request();
    let requestPage;
    try { requestPage = request.frame().page(); } catch { /* Worker without a page. */ }
    if (requestPage === page && discoveryRequestAllowed(request.url(), request.method(), config)) {
      await route.continue();
    } else {
      violations.push("requête");
      await route.abort("blockedbyclient");
    }
  });
  // Playwright routes only the first hop. Check redirect targets before Chromium follows them.
  const cdp = await context.newCDPSession(page);
  cdp.on("Fetch.requestPaused", async (event) => {
    try {
      if (redirectStatuses.has(event.responseStatusCode)) {
        const location = event.responseHeaders?.find(({ name }) => name.toLowerCase() === "location")?.value;
        if (!discoveryRedirectAllowed(event.request.url, location, event.request.method, event.responseStatusCode, config)) {
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
  return violations;
}

async function checkBrowserTransport(launchBrowser) {
  const checks = [];
  for (const host of dashboardHosts) {
    const forbidden = "/outside-demo-recipe";
    let forbiddenRequests = 0;
    const server = createServer((request, response) => {
      if (request.url === forbidden) forbiddenRequests++;
      response.setHeader("Content-Type", "text/html; charset=utf-8");
      response.end('<!doctype html><title>Loopback transport</title><link rel="icon" href="/favicon.svg">');
    });
    let browser;
    let phase = "écoute";
    try {
      server.listen(0, host === "[::1]" ? "::1" : "127.0.0.1");
      await once(server, "listening");
      const config = readDiscoveryConfig({
        AI_GUARD_TEST_DASHBOARD: `http://${host}:${server.address().port}`,
        AI_GUARD_TEST_OIDC_ISSUER: `http://${demoHost}:18080${realm}`,
      });
      phase = "démarrage Chromium";
      browser = await launchBrowser();
      const context = await browser.newContext({ serviceWorkers: "block" });
      const page = await context.newPage();
      phase = "interception";
      const violations = await guardBrowser(context, page, config);
      phase = "navigation autorisée";
      const response = await page.goto(config.dashboard.href, { timeout: 5000 });
      requireEvidence(response?.status() === 200 && violations.length === 0,
        "L’adresse loopback autorisée doit répondre HTTP 200.");
      const before = violations.length;
      phase = "refus hors recette";
      const blocked = await page.goto(new URL(forbidden, config.dashboard).href, { timeout: 5000 })
        .then(() => false, () => true);
      requireEvidence(blocked && violations[before] === "requête" && forbiddenRequests === 0,
        "La route hors recette doit être arrêtée avant le serveur local.");
      checks.push({ host, allowed_http: response.status(), disallowed_requests_reaching_server: forbiddenRequests });
    } catch (error) {
      if (error instanceof EvidenceError) throw error;
      throw new EvidenceError(`Prévol ${host} interrompu à l’étape « ${phase} ».`);
    } finally {
      await browser?.close();
      if (server.listening) {
        server.closeAllConnections();
        await new Promise((resolve) => server.close(resolve));
      }
    }
  }
  return checks;
}

async function checkClearedStorage(page) {
  const cleared = await page.evaluate(() => localStorage.length === 0 && sessionStorage.length === 0);
  requireEvidence(cleared, "L’état Web temporaire de la connexion subsiste sur le dashboard.");
}

export function observeDiscoveryStatus({ dashboardHref, statusPath }) {
  if (window !== window.top || location.href !== dashboardHref) return;
  const statusUrl = new URL(statusPath, dashboardHref).href;
  const fetch = window.fetch;
  let observation;
  Object.defineProperty(window, "__aiGuardDiscoveryStatus", { get: () => observation });
  window.fetch = (input, options) => {
    const pending = fetch(input, options);
    if (input === statusPath && (options?.method ?? "GET") === "GET" && location.href === dashboardHref) {
      // Clone before the application consumes the body. Return its original fetch promise unchanged.
      void pending.then(async (response) => {
        if (response.url !== statusUrl || response.status !== 200) return;
        try {
          const value = await response.clone().json();
          observation = Object.freeze({
            http: response.status,
            version: Number.isSafeInteger(value?.version) ? value.version : null,
            enabled: typeof value?.enabled === "boolean" ? value.enabled : null,
            issuer: typeof value?.issuer === "string" && value.issuer.length <= 2048 ? value.issuer : null,
            provider_label: typeof value?.provider_label === "string" && value.provider_label.length <= 128 ? value.provider_label : null,
          });
        } catch {
          observation = Object.freeze({ http: response.status });
        }
      }).catch(() => {});
    }
    return pending;
  };
}

async function runAccount(browser, config, account) {
  const context = await browser.newContext({ locale: "en-US", serviceWorkers: "block" });
  let page;
  let stage = "interception navigateur";
  let violations = [];
  let tokenDeliveries = 0;
  let dashboardReturnRequested = false;
  const responses = new Map();
  const initialHttp = {};
  try {
    await context.addInitScript(observeDiscoveryStatus, { dashboardHref: config.dashboard.href, statusPath: oidcStatus });
    page = await context.newPage();
    page.setDefaultTimeout(12000);
    violations = await guardBrowser(context, page, config);
    if (account === "admin") {
      page.on("request", (request) => {
        const { origin, pathname } = new URL(request.url());
        if (origin === config.dashboard.origin && [callback, complete, "/auth/oidc/tokens"].includes(pathname)) {
          dashboardReturnRequested = true;
        }
      });
    }
    page.on("response", (response) => {
      const { origin, pathname } = new URL(response.url());
      if (origin === config.dashboard.origin) {
        responses.set(response.request().method() + " " + pathname, response.status());
        if (pathname === "/auth/oidc/tokens" && response.ok()) tokenDeliveries++;
      }
    });
    stage = "chargement du dashboard et observation du SSO";
    const [publicStatus] = await Promise.all([
      page.waitForResponse((response) => response.url() === config.dashboard.origin + oidcStatus &&
        response.request().method() === "GET", { timeout: 15000 }).catch((error) => {
        throw new EvidenceError(error?.name === "TimeoutError" ? "Aucune réponse GET d’état SSO dans le délai." :
          "L’observation de la réponse d’état SSO a été interrompue.");
      }).then((response) => {
        initialHttp.sso_status_http = response.status();
        return response;
      }),
      page.goto(config.dashboard.href, { waitUntil: "domcontentloaded", timeout: 12000 }).catch((error) => {
        throw new EvidenceError(error?.name === "TimeoutError" ? "Le document du dashboard n’a pas chargé dans le délai." :
          "La navigation vers le dashboard a échoué.");
      }).then((response) => {
        if (response) initialHttp.dashboard_document_http = response.status();
        requireEvidence(response?.status() === 200 && response.url() === config.dashboard.href,
          "Le document de l’origine exacte du dashboard doit répondre HTTP 200.");
      }),
    ]);
    stage = "validation de l’état du SSO";
    requireEvidence(publicStatus.status() === 200, "L’état SSO du dashboard doit répondre HTTP 200.");
    const observed = await page.waitForFunction(() => window.__aiGuardDiscoveryStatus, undefined, { timeout: 8000 }).catch(() => {
      throw new EvidenceError("La réponse d’état SSO n’a pas été lue dans la page.");
    });
    const summary = await observed.jsonValue();
    await observed.dispose();
    requireEvidence(summary?.http === publicStatus.status() && Object.hasOwn(summary, "version"),
      "La réponse d’état SSO lue dans la page doit contenir un JSON lisible.");
    requireEvidence(summary.version === 1 && summary.enabled === true && summary.issuer === config.issuer.href &&
      summary.provider_label === "Keycloak DEMO", "Le SSO actif et l’issuer exact du profil Keycloak DEMO sont requis.");
    stage = "bouton de connexion Keycloak DEMO";
    const signIn = page.getByRole("button", { name: "Se connecter avec Keycloak DEMO", exact: true });
    await signIn.waitFor().catch(() => {
      throw new EvidenceError("Le bouton de connexion Keycloak DEMO doit être visible après validation de l’état SSO.");
    });
    stage = "autorisation Code + PKCE S256";
    await signIn.click();
    await page.waitForURL((url) => validAuthorization(url.href, config));
    stage = "formulaire Keycloak du realm DEMO";
    await page.locator("#username").fill(account);
    await page.locator("#password").fill(account);
    await page.locator("#kc-login").click();
    if (account === "admin") {
      stage = "destination du refus Keycloak";
      await page.waitForURL((url) => url.origin === config.issuer.origin && url.pathname === realm + "/login-actions/authenticate");
      stage = "refus Keycloak du compte master";
      // The base theme puts the ID on the span. Keycloak v2 puts it on the username error wrapper.
      const errorMessage = page.locator("span#input-error, #input-error-username > span");
      await errorMessage.waitFor();
      requireEvidence((await errorMessage.textContent())?.trim() === "Invalid username or password.",
        "Le message de refus Keycloak diffère du texte attendu.");
      const refused = new URL(page.url());
      requireEvidence(refused.origin === config.issuer.origin && refused.pathname === realm + "/login-actions/authenticate" &&
        !dashboardReturnRequested, "Le refus du compte master doit rester dans le realm DEMO sans callback ni remise.");
      requireEvidence(!violations.length, "Le navigateur a tenté une requête hors du parcours autorisé.");
      return { account, status: "PASS", evidence: "realm_demo_login_refused" };
    }
    stage = "retour OIDC et identité nominative";
    await page.getByRole("heading", { name: "Choisir un contexte.", exact: true }).waitFor();
    await page.waitForURL(config.dashboard.href);
    await page.getByText("COMPTE OIDC / CONSULTATION NOMINATIVE", { exact: true }).waitFor();
    await page.getByText(account + " — DEMO", { exact: true }).and(page.locator(".authenticated-scope")).waitFor();
    requireEvidence(responses.get("GET " + callback) === 303 &&
      responses.get("GET " + complete) === 200 && tokenDeliveries === 1 &&
      responses.get("GET " + identity) === 200, "Callback, remise unique du résultat et identité authentifiée doivent être observés.");
    await checkClearedStorage(page);
    stage = "contextes et capacités serveur";
    const administrator = account === "admin-ai-guard";
    const cards = page.locator(".human-context-card");
    const expectedModes = administrator ? ["Administration", "Personnel"] : ["Personnel"];
    const headings = (await cards.getByRole("heading", { level: 2 }).allTextContents()).sort();
    requireEvidence(headings.length === expectedModes.length &&
      headings.every((value, index) => value === expectedModes[index] + " · " + tenant),
      "Les contextes affichés diffèrent des permissions du compte DEMO.");
    requireEvidence(await page.getByText("Liste des contextes partielle", { exact: true }).count() === 0,
      "Une liste de contextes partielle ne permet pas de vérifier les permissions DEMO.");
    for (const mode of expectedModes) {
      const contextCard = cards.filter({ has: page.getByRole("heading", { name: mode + " · " + tenant, exact: true }) });
      const capabilityText = await contextCard.getByText(/^Capacités serveur : /).textContent();
      const capabilities = capabilityText?.replace("Capacités serveur : ", "").split(", ").sort() ?? [];
      const expected = ["read_metadata", "read_content", "export"];
      if (mode === "Administration") expected.push("read_tenant_metadata");
      requireEvidence(capabilities.join(",") === expected.sort().join(","),
        "Les capacités affichées diffèrent des permissions du contexte DEMO.");
    }
    const selectedMode = administrator ? "administration" : "personnel";
    const card = cards.filter({ has: page.getByRole("button", { name: "Ouvrir " + selectedMode + " · " + tenant, exact: true }) });
    stage = "lecture autorisée des ressources";
    const [read] = await Promise.all([
      page.waitForResponse((response) => {
        const url = new URL(response.url());
        return url.origin === config.dashboard.origin && url.pathname === resources;
      }),
      card.getByRole("button", { name: "Ouvrir " + selectedMode + " · " + tenant, exact: true }).click(),
    ]);
    await page.getByRole("heading", { name: "Ressources autorisées.", exact: true }).waitFor();
    requireEvidence(read.status() === 200, "La lecture des ressources n’a pas réussi dans le contexte proposé.");
    await page.getByText("Lecture réussie", { exact: true }).first().waitFor();
    stage = "déconnexion locale";
    const [logout] = await Promise.all([
      page.waitForResponse((response) => response.url() === config.dashboard.origin + "/auth/oidc/logout" &&
        response.request().method() === "POST"),
      page.getByRole("button", { name: "Se déconnecter", exact: true }).click(),
    ]);
    await signIn.waitFor();
    requireEvidence(logout.status() === 200 && await page.locator(".authenticated-scope").count() === 0,
      "La déconnexion locale ou sa réponse n’a pas été observée.");
    const logoutStatus = await logout.json();
    requireEvidence(logoutStatus?.local_session === "closed" && logoutStatus.audit === "recorded" &&
      ["accepted", "failed", "unavailable", "unsupported", "not_attempted"].includes(logoutStatus.provider_logout),
      "La fermeture d’admission locale et son audit ne sont pas confirmés.");
    await checkClearedStorage(page);
    requireEvidence(!violations.length, "Le navigateur a tenté une requête hors du parcours autorisé.");
    return { account, status: "PASS", evidence: "oidc_identity_and_authorized_read", context: selectedMode,
      local_logout: "closed", logout_audit: "recorded", provider_logout: logoutStatus.provider_logout };
  } catch (error) {
    // Callback and IdP pages can embed ephemeral codes and state in their DOM. Capture only the cleaned dashboard.
    const artifacts = page?.url() === config.dashboard.href ? await saveBrowserFailure(page, {
      directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR, secrets: accounts,
    }) : null;
    return { account, status: "FAIL", stage,
      reason: violations.length ? "Le navigateur a tenté une requête hors du parcours autorisé." :
        error instanceof EvidenceError ? error.message : "L’étape n’a pas abouti.",
      ...initialHttp,
      ...(artifacts ? { artifacts } : {}) };
  } finally {
    await context.close();
  }
}

async function main() {
  let stage = "configuration";
  const transportOnly = process.argv.includes("--check-transport");
  const result = { mode: transportOnly ? "keycloak_discovery_transport_preflight" : "keycloak_discovery_oidc_web" };
  try {
    requireEvidence(!transportOnly || !process.argv.includes("--check-config"), "Sélectionnez un seul mode de prévol.");
    const config = transportOnly ? null : readDiscoveryConfig(process.env);
    if (process.argv.includes("--check-config")) {
      console.log(JSON.stringify({ ...result, status: "PASS", scope: "configuration_only", accounts: config.selected }));
      return;
    }
    requireEvidence(!process.env.DEBUG && !process.env.PWDEBUG && !process.env.NODE_DEBUG,
      "Désactivez les journaux de débogage réseau et Playwright pour garder l’échange OIDC privé.");
    stage = "lancement Chromium";
    const require = createRequire(import.meta.url);
    let playwright;
    try { playwright = require("playwright"); }
    catch {
      const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
      requireEvidence(modules, "Playwright est requis pour ce parcours.");
      playwright = require(join(modules, "playwright"));
    }
    const launchBrowser = () => playwright.chromium.launch({ headless: true, chromiumSandbox: true,
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
      args: ["--no-proxy-server", "--disable-background-networking", `--host-resolver-rules=${hostResolverRules}`] });
    if (transportOnly) {
      stage = "prévol loopback";
      const checks = await checkBrowserTransport(launchBrowser);
      console.log(JSON.stringify({ ...result, status: "PASS", scope: "loopback_http_only", checks }));
      return;
    }
    stage = "parcours des comptes";
    const checks = [];
    for (const account of config.selected) {
      const browser = await launchBrowser();
      try { checks.push(await runAccount(browser, config, account)); }
      finally { await browser.close(); }
    }
    const passed = checks.every((check) => check.status === "PASS");
    console.log(JSON.stringify({ ...result, status: passed ? "PASS" : "FAIL", checks }));
    process.exitCode = passed ? 0 : 1;
  } catch (error) {
    console.error(JSON.stringify({ ...result, status: "FAIL", stage,
      reason: error instanceof EvidenceError ? error.message : "L’étape n’a pas abouti." }));
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
