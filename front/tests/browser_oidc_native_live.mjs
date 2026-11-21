// Native CLI + Keycloak discovery DEMO. The caller owns the prepared loopback services.
// Required: AI_GUARD_TEST_CLI, AI_GUARD_TEST_GATEWAY, AI_GUARD_TEST_OIDC_ISSUER.
// Playwright from the local install or CODEX_PRIMARY_RUNTIME_NODE_MODULES.
// --check-gateway-restart waits for a single gateway_restarted line from the caller after the gateway is healthy again.
import { spawn } from "node:child_process";
import { access, mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { constants } from "node:fs";
import { get } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { createInterface } from "node:readline";

const demoHost = "keycloak.ai-guard.test";
const realm = "/realms/ai-guard-demo";
const protocol = realm + "/protocol/openid-connect";
const callbackPath = "/oidc/callback";
const account = "user-1";
const clientId = "ai-guard-demo-cli";
const tenant = "ai-guard-demo";
const uuid = /^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/;
const redirects = new Set([301, 302, 303, 307, 308]);
const providerOutcomes = ["accepted", "failed", "unavailable", "unsupported", "not_attempted"];
const check = (condition) => { if (!condition) throw new Error("Native OIDC oracle failed."); };

function httpUrl(value) {
  if (typeof value !== "string" || value.length > 8192) return null;
  try {
    const url = new URL(value);
    return url.protocol === "http:" && !url.username && !url.password && !url.hash ? url : null;
  } catch { return null; }
}

function configFrom(env) {
  const gateway = httpUrl(env.AI_GUARD_TEST_GATEWAY);
  const issuer = httpUrl(env.AI_GUARD_TEST_OIDC_ISSUER);
  const binary = env.AI_GUARD_TEST_CLI;
  check(gateway?.hostname === "127.0.0.1" && gateway.port &&
    [gateway.origin, gateway.origin + "/"].includes(env.AI_GUARD_TEST_GATEWAY));
  check(issuer?.port && ["127.0.0.1", demoHost].includes(issuer.hostname) &&
    env.AI_GUARD_TEST_OIDC_ISSUER === issuer.origin + realm && issuer.origin !== gateway.origin);
  check(typeof binary === "string" && isAbsolute(binary));
  return { gateway, issuer, binary };
}

function authorizationFrom(raw, issuer) {
  const url = httpUrl(raw);
  check(url?.origin === issuer.origin && url.pathname === protocol + "/auth");
  const params = url.searchParams;
  check(new Set(params.keys()).size === [...params.keys()].length);
  const expected = new Set(["response_type", "client_id", "state", "code_challenge", "code_challenge_method", "redirect_uri", "scope", "nonce"]);
  check([...params.keys()].every((name) => expected.has(name)));
  check(params.get("client_id") === clientId && params.get("response_type") === "code" && params.get("code_challenge_method") === "S256" &&
    /^[A-Za-z0-9_-]{43}$/.test(params.get("code_challenge") ?? "") && params.get("scope")?.split(" ").includes("openid") &&
    /^[A-Za-z0-9_-]{16,128}$/.test(params.get("state") ?? "") && /^[A-Za-z0-9_-]{16,128}$/.test(params.get("nonce") ?? ""));
  const callback = httpUrl(params.get("redirect_uri"));
  check(callback?.hostname === "127.0.0.1" && callback.port && callback.origin !== issuer.origin &&
    params.get("redirect_uri") === callback.origin + callbackPath);
  return { url, callback, state: params.get("state") };
}

function matchingCallback(url, issuer, auth) {
  const query = url.searchParams;
  const fields = new Set(["state", "code", "session_state", "iss"]);
  return url.origin === auth.callback.origin && url.pathname === callbackPath &&
    query.get("state") === auth.state && Boolean(query.get("code")) &&
    new Set(query.keys()).size === [...query.keys()].length &&
    [...query.keys()].every((name) => fields.has(name)) &&
    (!query.has("iss") || query.get("iss") === issuer.href);
}

function allowedBrowserRequest(raw, method, { issuer, gateway }, auth) {
  const url = httpUrl(raw);
  if (!url || url.origin === gateway.origin) return false;
  if (url.origin === auth.callback.origin) return method === "GET" && matchingCallback(url, issuer, auth);
  if (url.origin !== issuer.origin) return false;
  if (method === "POST") return url.pathname === realm + "/login-actions/authenticate";
  return method === "GET" && (url.href === auth.url.href || url.pathname.startsWith("/resources/"));
}

async function browserGuard(context, page, config, auth) {
  let blocked = 0;
  await context.routeWebSocket("**/*", async (socket) => { blocked++; await socket.close(); });
  await context.route("**/*", async (route) => {
    const request = route.request();
    let frame;
    try { frame = request.frame(); } catch { /* a worker has no frame */ }
    if (frame === page.mainFrame() && allowedBrowserRequest(request.url(), request.method(), config, auth)) {
      await route.continue();
    } else {
      blocked++;
      await route.abort("blockedbyclient");
    }
  });
  // Playwright's route is not replayed for redirect hops. Inspect Location before Chromium may follow it.
  const cdp = await context.newCDPSession(page);
  cdp.on("Fetch.requestPaused", async (event) => {
    try {
      if (redirects.has(event.responseStatusCode)) {
        const location = event.responseHeaders?.filter(({ name }) => name.toLowerCase() === "location") ?? [];
        const { url, method } = event.request;
        const converted = event.responseStatusCode === 303 || ([301, 302].includes(event.responseStatusCode) && method === "POST");
        const nextMethod = converted ? "GET" : method;
        const allowed = location.length === 1 && new URL(url).origin === config.issuer.origin &&
          allowedBrowserRequest(new URL(location[0].value, url).href, nextMethod, config, auth);
        if (!allowed) {
          blocked++;
          await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "BlockedByClient" });
          return;
        }
      }
      await cdp.send("Fetch.continueRequest", { requestId: event.requestId });
    } catch {
      if (!page.isClosed() && !event.responseErrorReason) blocked++;
      await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "BlockedByClient" }).catch(() => {});
    }
  });
  await cdp.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Response" }] });
  return () => blocked;
}

function profileMatches(profile, issuer) {
  const provider = profile?.provider;
  const endpoint = (path) => issuer.origin + protocol + "/" + path;
  return profile?.version === 1 && profile.enabled === true && provider?.version === 1 && provider.issuer === issuer.href &&
    provider.cli_client_id === clientId && provider.api_audience === "ai-guard-demo-api" &&
    provider.transport?.kind === "isolated_demo_http" && provider.transport.exact_issuer === issuer.href &&
    provider.cli_callback_path === callbackPath && provider.authorization_endpoint === endpoint("auth") &&
    provider.token_endpoint === endpoint("token") && provider.jwks_uri === endpoint("certs") &&
    provider.end_session_endpoint === endpoint("logout") && profile.ca_bundle_pem == null;
}

async function readProfile(gateway) {
  return new Promise((resolve, reject) => {
    const endpoint = new URL("/auth/oidc/cli-profile", gateway);
    const request = get(endpoint, { agent: false, signal: AbortSignal.timeout(5000), headers: { accept: "application/json" } }, async (response) => {
      try {
        check(response.statusCode === 200 && response.headers["content-type"]?.includes("application/json"));
        const chunks = [];
        let bytes = 0;
        for await (const chunk of response) {
          bytes += chunk.length;
          check(bytes <= 32 * 1024);
          chunks.push(chunk);
        }
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch (error) { response.destroy(); reject(error); }
    });
    request.once("error", reject);
  });
}

async function checkHost(issuer) {
  if (issuer.hostname !== demoHost) return;
  const contents = await readFile("/etc/hosts", "utf8");
  const matches = contents.split(/\r?\n/).map((line) => line.split("#")[0].trim().split(/\s+/))
    .filter(([, ...names]) => names.some((name) => name.toLowerCase() === demoHost));
  check(matches.length > 0 && matches.every(([ip]) => ip === "127.0.0.1"));
}

async function deadline(promise, milliseconds = 15000) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("Step deadline.")), milliseconds); })]);
  } finally { clearTimeout(timer); }
}

async function waitForGatewayRestart(milliseconds) {
  const input = process.stdin;
  const expected = Buffer.from("gateway_restarted\n");
  let received = 0;
  let receive;
  let incomplete;
  try {
    await deadline(new Promise((resolve, reject) => {
      incomplete = () => reject(new Error("Missing or invalid gateway restart signal."));
      receive = (chunk) => {
        const next = received + chunk.length;
        if (next > expected.length || !chunk.equals(expected.subarray(received, next))) {
          incomplete();
          return;
        }
        received = next;
        if (received === expected.length) resolve();
      };
      input.once("end", incomplete);
      input.once("error", incomplete);
      input.on("data", receive);
    }), milliseconds);
  } finally {
    input.off("data", receive);
    input.off("end", incomplete);
    input.off("error", incomplete);
    input.pause();
  }
}

function readIdentity(identity, issuer) {
  check(identity.version === 1 && identity.display_name === account + " — DEMO" && identity.authentication?.method === "oidc_access_token" &&
    identity.principal?.kind === "human_oidc" && identity.principal.issuer === issuer.href &&
    typeof identity.principal.subject === "string" && identity.principal.subject.length > 0 &&
    identity.contexts_complete === true && Array.isArray(identity.contexts) && identity.contexts.length === 1);
  const selected = identity.contexts[0];
  check(/^[a-f0-9]{64}$/.test(selected.id) && selected.mode === "personal" && selected.tenant_id === tenant &&
    [...selected.capabilities].sort().join(",") === "export,read_content,read_metadata");
  return selected.id;
}

async function cleanupChild(child, exited) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.stdin.end();
  if (await deadline(exited, 2500).then(() => true, () => false)) return;
  child.kill("SIGTERM");
  if (await deadline(exited, 1000).then(() => true, () => false)) return;
  child.kill("SIGKILL");
  await deadline(exited, 1000).catch(() => {});
}

const report = { status: "FAIL", stage: "config", mode: "native_cli_keycloak_demo", account };
let child;
let exited;
let browser;
let workspace;
let lines;
try {
  check(!process.env.DEBUG && !process.env.PWDEBUG && !process.env.NODE_DEBUG);
  const config = configFrom(process.env);
  const options = process.argv.slice(2);
  check(options.length === 0 || options.length === 1 && ["--check-config", "--check-gateway-restart"].includes(options[0]));
  const checkRestart = options[0] === "--check-gateway-restart";
  if (checkRestart) report.mode = "native_cli_keycloak_demo_gateway_restart";
  if (options[0] === "--check-config") {
    report.status = "NOT_RUN";
    report.stage = "config_validated";
  } else {
    report.stage = "local_preflight";
    await access(config.binary, constants.X_OK);
    await checkHost(config.issuer);
    check(profileMatches(await readProfile(config.gateway), config.issuer));

    report.stage = "browser_launch";
    const require = createRequire(import.meta.url);
    const modulePath = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
    let chromium;
    try { ({ chromium } = require("playwright")); }
    catch { check(modulePath); ({ chromium } = require(join(modulePath, "playwright"))); }
    browser = await chromium.launch({ headless: true, chromiumSandbox: true, timeout: 12000,
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
      args: ["--no-proxy-server", "--host-resolver-rules=EXCLUDE 127.0.0.1, MAP keycloak.ai-guard.test 127.0.0.1, MAP * ~NOTFOUND"] });
    const context = await browser.newContext({ serviceWorkers: "block", acceptDownloads: false });
    const page = await context.newPage();
    page.setDefaultTimeout(12000);

    report.stage = "cli_authorization";
    workspace = await mkdtemp(join(tmpdir(), "ai-guard-native-browser-"));
    child = spawn(config.binary, ["human", "sso", "--gateway", config.gateway.origin, "--browser", "manual"], {
      cwd: workspace, env: { HOME: workspace, TMPDIR: workspace, PATH: "/usr/bin:/bin" }, stdio: ["pipe", "pipe", "ignore"],
    });
    exited = new Promise((resolve) => {
      child.once("error", () => resolve(null));
      child.once("close", (code) => resolve(code));
    });
    child.stdin.on("error", () => {});
    lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
    const stream = lines[Symbol.asyncIterator]();
    const read = async () => {
      const line = await deadline(stream.next());
      check(!line.done && Buffer.byteLength(line.value) <= 512 * 1024);
      const result = JSON.parse(line.value);
      check(result?.version === 1);
      return result;
    };
    const enter = (command) => child.stdin.write(command + "\n");
    const call = async (command) => { enter(command); return read(); };
    const start = await read();
    check(start.status === "browser_authorization_required");
    const auth = authorizationFrom(start.authorization_url, config.issuer);
    check(auth.callback.origin !== config.gateway.origin);

    report.stage = "keycloak_form";
    const blocked = await browserGuard(context, page, config, auth);
    const login = await page.goto(auth.url.href, { waitUntil: "domcontentloaded" });
    check(login?.status() === 200 && new URL(page.url()).origin === config.issuer.origin);
    await page.locator("#username").fill(account);
    await page.locator("#password").fill(account);
    report.stage = "cli_callback";
    const [callback] = await Promise.all([
      page.waitForResponse((response) => {
        const url = httpUrl(response.url());
        return url && response.request().method() === "GET" && matchingCallback(url, config.issuer, auth);
      }),
      page.locator("#kc-login").click(),
    ]);
    check(callback.status() === 200 && (await callback.text()).trim() === "Authorization received. Return to the ai-guard terminal." && blocked() === 0);
    await context.close();
    report.callback_http = 200;

    report.stage = "cli_session_ready";
    const ready = await read();
    check(ready.status === "session_ready" && ready.generation === 1 && ready.storage === "foreground_memory_only" && uuid.test(ready.authentication_id));
    report.authentication_id = ready.authentication_id;
    report.stage = "cli_identity";
    const identity = await call("identity");
    const contextId = readIdentity(identity, config.issuer);
    const selected = await call("context " + contextId);
    check(selected.status === "context_selected" && selected.context?.id === contextId);

    report.stage = "cli_resources";
    const resources = await call("resources --limit 5");
    check(resources.context_id === contextId && resources.coverage === "authorized_selection" &&
      Array.isArray(resources.items) && resources.items.length <= 5 && typeof resources.has_more === "boolean" &&
      uuid.test(resources.read_receipt?.audit_event_id) && uuid.test(resources.read_receipt?.request_id) &&
      Number.isFinite(Date.parse(resources.read_receipt?.received_at)));
    report.resource_count = resources.items.length;
    report.read_receipt_audit_event_id = resources.read_receipt.audit_event_id;

    if (checkRestart) {
      report.stage = "await_gateway_restart";
      const expiresAt = Date.parse(ready.expires_at);
      // Reserve one full CLI read deadline and a five-second margin before expiry.
      const waitMs = Math.min(45000, expiresAt - Date.now() - 20000);
      check(waitMs > 0);
      console.log(JSON.stringify({ mode: report.mode, status: "WAITING", stage: report.stage, wait_ms: waitMs }));
      await waitForGatewayRestart(waitMs);
      report.stage = "post_restart_identity";
      const refused = await call("identity");
      check(Date.now() < expiresAt && refused.status === "command_refused" &&
        refused.reason === "human access refused: HTTP 401, credential_invalid, retry_action=authenticate; check the private human credential with the operator" &&
        Object.keys(refused).every((key) => ["version", "status", "reason", "next_action"].includes(key)));
      report.post_restart_identity_http = 401;
      report.post_restart_identity_code = "credential_invalid";
      report.post_restart_contexts_returned = false;
      report.admission_deadline_remaining = true;
    } else {
      report.stage = "cli_refresh";
      const refreshed = await call("refresh");
      check(refreshed.status === "session_refreshed" && refreshed.generation === 2 && refreshed.authentication_id === ready.authentication_id);
      report.stage = "cli_refreshed_identity";
      const again = await call("identity");
      const renewedContext = readIdentity(again, config.issuer);
      check(again.principal.subject === identity.principal.subject && renewedContext !== contextId);
      report.generations = [1, 2];
    }

    report.stage = "cli_logout";
    const logout = await call("logout");
    check(logout.status === "session_closed" &&
      (logout.local_session === "closed" || checkRestart && logout.local_session === "unknown") &&
      (logout.audit === "recorded" || checkRestart && logout.audit === "unavailable") &&
      providerOutcomes.includes(logout.provider_logout) && providerOutcomes.includes(logout.control_provider_logout));
    report.local_session = logout.local_session;
    report.logout_audit = logout.audit;
    report.provider_logout = logout.provider_logout;
    report.control_provider_logout = logout.control_provider_logout;
    check(await deadline(exited, 10000) === 0);
    if (checkRestart) report.cli_process_exited = true;
    report.stage = "cli_ephemeral_storage";
    const names = await readdir(workspace);
    check(names.length === 0);
    report.credential_files_created = false;
    if (!checkRestart) {
      report.stage = "provider_logout";
      check(logout.provider_logout === "accepted" && logout.control_provider_logout === "not_attempted");
    }
    report.status = "PASS";
    report.stage = "complete";
  }
} catch {
  process.exitCode = 1;
} finally {
  await browser?.close().catch(() => {});
  await cleanupChild(child, exited);
  lines?.close();
  if (workspace) await rm(workspace, { recursive: true, force: true }).catch(() => {});
  console.log(JSON.stringify(report));
}
