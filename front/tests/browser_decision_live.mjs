// Read-only Chromium checks for decisions on a private Rust loopback stack.
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { closeBrowserResources, installBrowserGuard } from "./browser_live.mjs";
import { saveBrowserFailure } from "./browser_failure.mjs";

const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const sha256 = /^[0-9a-f]{64}$/;
const tenantId = /^[A-Za-z0-9_-]{1,128}$/;
const outcomeText = { allow: "Autorisé", deny: "Refusé", review: "À confirmer" };
const qwenModel = "qwen2.5-0.5b-q4km";
const typesafeRequestedModel = "jev-latest";
const typesafeReturnedModel = "jev-fixture-version";
const typesafeReplayModes = new Set(["typesafe-real", "typesafe-replay-fixture"]);
const typesafeAdviceRules = { allow: "exact_allowlist", deny: "jev_deny", review: "jev_review_fail_closed" };
const evaluatorLabel = { rust_rules: "Rust déterministe", local_openai: "Modèle local", typesafe: "Jev TypeSafe" };
const adviceLabel = { allow: "Autoriser", deny: "Refuser", review: "Demander une revue", unknown: "Indéterminé" };
const effectiveLocalAdvice = { allow: "review", deny: "deny", review: "review", unknown: "review" };
const evaluatorStatus = {
  evaluated: "Évaluée",
  skipped_local_policy: "Non sollicitée par la politique locale",
  unavailable_or_invalid: "Indisponible ou invalide",
};
const identityFields = ["id", "hash", "request_id"];
class EvidenceError extends Error {}
class BrowserUnavailable extends Error {}
function check(condition, message) {
  if (!condition) throw new EvidenceError(message);
}
const matches = (value, pattern) => typeof value === "string" && value.trim() === value && pattern.test(value);

function identityProof(entry) {
  return entry && Number.isSafeInteger(entry.seq) && entry.seq > 0 && matches(entry.id, uuid) &&
    matches(entry.request_id, uuid) && matches(entry.session_id, uuid) && matches(entry.hash, sha256);
}

function boundedUsage(usage) {
  const fields = ["prompt_tokens", "completion_tokens", "total_tokens"];
  return fields.every((field) => Number.isSafeInteger(usage?.[field]) && usage[field] > 0) &&
    usage.completion_tokens <= 256 && usage.prompt_tokens + usage.completion_tokens === usage.total_tokens;
}

function strictVeto(entry) {
  return entry.outcome === "deny" && entry.rule === "strict_default_deny" && entry.backend_status === "skipped_local_policy";
}

function validRetainedTypesafe(entry) {
  if (!matches(entry.configuration_sha256, sha256)) return false;
  if (entry.backend_status === "unavailable_or_invalid") {
    return entry.outcome === "deny" && entry.rule === "jev_unavailable_or_invalid" &&
      entry.provider_outcome === null && entry.returned_model === null;
  }
  return entry.backend_status === "evaluated" && typeof entry.provider_outcome === "string" &&
    Object.hasOwn(typesafeAdviceRules, entry.provider_outcome) &&
    entry.rule === typesafeAdviceRules[entry.provider_outcome] && entry.outcome === (entry.provider_outcome === "allow" ? "allow" : "deny") &&
    matches(entry.returned_model, /^[A-Za-z0-9][A-Za-z0-9._:/@-]{0,127}$/);
}

function validLocalFailure(entry) {
  const diagnostic = entry.transport_diagnostic;
  const validDiagnostic = diagnostic && typeof diagnostic === "object" && !Array.isArray(diagnostic) &&
    diagnostic.hop === "configured_endpoint" && ["send", "response_body"].includes(diagnostic.phase) &&
    ["timeout", "connection_error", "http_status", "transport_error"].includes(diagnostic.kind) &&
    Object.keys(diagnostic).every((key) => ["hop", "phase", "kind", "http_status"].includes(key)) &&
    (!Object.hasOwn(diagnostic, "http_status") || Number.isInteger(diagnostic.http_status) &&
      diagnostic.http_status >= 100 && diagnostic.http_status <= 599);
  return entry.backend_status === "unavailable_or_invalid" && entry.outcome === "deny" &&
    entry.rule === "decision_backend_unavailable_or_invalid" && entry.provider_outcome === null &&
    entry.advisory_outcome === null && entry.returned_model === null && !Object.hasOwn(entry, "usage") &&
    matches(entry.configuration_sha256, sha256) && Number.isInteger(entry.model_timeout_seconds) &&
    entry.model_timeout_seconds >= 1 && entry.model_timeout_seconds <= 12 &&
    (entry.backend_error === "transport_unavailable" ? validDiagnostic :
      ["invalid_configuration", "request_too_large", "response_too_large", "invalid_response"].includes(entry.backend_error) &&
      diagnostic === null);
}

export function validateDecisionProof(proof) {
  const judgeMode = proof?.judge_mode ?? "rust";
  const localFixture = judgeMode === "local-fixture";
  const typesafeReplay = typesafeReplayModes.has(judgeMode);
  check(proof?.version === 1 && matches(proof.tenant_id, tenantId) &&
    matches(proof.foreign_tenant_id, tenantId) && proof.tenant_id !== proof.foreign_tenant_id &&
    (typesafeReplay || ["rust", "local-fixture", "typesafe-fixture", "local-real"].includes(judgeMode)) && Array.isArray(proof.decisions) &&
    proof.decisions.length === (localFixture || judgeMode === "typesafe-fixture" ? 3 : 2) &&
    identityProof(proof.foreign_decision), "Le format de la preuve ou les deux tenants sont invalides.");
  check(proof.decisions.every((entry) => identityProof(entry) && Object.hasOwn(outcomeText, entry.outcome) &&
    matches(entry.rule, /^[a-z0-9_]{1,128}$/)), "Une décision attendue est incomplète.");
  if (judgeMode === "local-real") {
    const [evaluation, veto] = proof.decisions;
    const evaluated = evaluation.outcome === "deny" && evaluation.backend_status === "evaluated" &&
      Object.hasOwn(effectiveLocalAdvice, evaluation.provider_outcome) &&
      evaluation.advisory_outcome === effectiveLocalAdvice[evaluation.provider_outcome] &&
      evaluation.rule === (evaluation.advisory_outcome === "deny" ? "local_openai_deny" : "local_openai_review_fail_closed") &&
      matches(evaluation.configuration_sha256, sha256) && boundedUsage(evaluation.usage);
    check((evaluated || validLocalFailure(evaluation)) && evaluation.seq < veto.seq &&
      strictVeto(veto) && strictVeto(proof.foreign_decision) && proof.audit_head === veto.hash,
    "La décision Qwen et les deux refus Rust sans appel modèle doivent être attestés.");
  } else if (typesafeReplay) {
    const [evaluation, veto] = proof.decisions;
    check(validRetainedTypesafe(evaluation) && evaluation.seq < veto.seq && proof.audit_head === veto.hash &&
      strictVeto(veto) && strictVeto(proof.foreign_decision),
    "Une décision TypeSafe retenue et deux refus Rust après réouverture sont requis.");
  } else if (localFixture) {
    check(proof.decisions.every(({ outcome, backend_status }) => outcome === "deny" && Object.hasOwn(evaluatorStatus, backend_status)) &&
      new Set(proof.decisions.map(({ backend_status }) => backend_status)).size === 3,
    "Trois refus distincts sont attendus après avis local, veto Rust et panne du modèle.");
  } else if (judgeMode === "typesafe-fixture") {
    check(proof.decisions.some(({ outcome, backend_status }) => outcome === "allow" && backend_status === "evaluated") &&
      proof.decisions.some(({ rule, backend_status }) => rule === "jev_deny" && backend_status === "evaluated") &&
      proof.decisions.some(({ rule, backend_status }) => rule === "strict_default_deny" && backend_status === "skipped_local_policy"),
    "L’accord Jev, le veto Jev et le refus local sans évaluation doivent être attestés.");
  } else {
    check(proof.decisions.filter(({ outcome }) => outcome === "allow").length === 1,
      "Une décision autorisée et un refus ou une revue sont attendus.");
  }
  check(new Set(proof.decisions.map(({ seq }) => seq)).size === proof.decisions.length,
    "Les décisions du tenant A doivent avoir des séquences distinctes.");
  const all = [...proof.decisions, proof.foreign_decision];
  check(identityFields.every((field) => new Set(all.map((entry) => entry[field])).size === all.length) &&
    proof.decisions.every(({ session_id }) => session_id !== proof.foreign_decision.session_id),
    "Les identifiants de décision et les empreintes doivent être distincts entre les demandes de test.");
  return proof;
}

function dashboardOrigin(value, explicitPort) {
  let url;
  try { url = new URL(value); } catch { /* Report only a static diagnostic. */ }
  check(url && url.protocol === "http:" && ["127.0.0.1", "[::1]"].includes(url.hostname) &&
    url.href === url.origin + "/", "AI_GUARD_TEST_DASHBOARD doit être une origine HTTP sur loopback littérale.");
  check(!explicitPort || url.hostname === "127.0.0.1" && !!url.port && [url.origin, url.href].includes(value),
    "Ce parcours exige l’origine IPv4 loopback avec son port explicite.");
  return url.origin;
}

function eventMatches(event, expected) {
  return event.seq === expected.seq && event.id === expected.id && event.hash === expected.hash &&
    event.session_id === expected.session_id && event.data?.request_id === expected.request_id &&
    event.decision?.outcome === expected.outcome && event.decision.rule === expected.rule;
}

function observedEvaluation(event, expected, mode) {
  const jev = event.data?.jev;
  const judged = expected.backend_status ?? (expected.outcome === "allow" ? "evaluated" : "skipped_local_policy");
  const typesafeReplay = typesafeReplayModes.has(mode);
  const backend = typesafeReplay && judged !== "skipped_local_policy" ? "typesafe" :
    { "local-fixture": "local_openai", "local-real": "local_openai", "typesafe-fixture": "typesafe" }[mode] ?? "rust_rules";
  let engine = "deterministic";
  if (backend === "local_openai" && judged !== "skipped_local_policy") engine = "local_openai";
  if (backend === "typesafe" && expected.outcome === "deny" && judged !== "skipped_local_policy") engine = "jev";
  if (event.source !== "gateway" || event.provenance !== "observed" || event.decision?.engine !== engine ||
      typeof event.decision.reason !== "string" || !event.decision.reason.trim() || !matches(event.prev_hash, sha256) ||
      jev?.backend !== backend || jev.status !== judged || jev.selection_source !== "cli") return false;
  if (backend === "rust_rules") return true;
  if (backend === "typesafe") {
    if (jev.requested_model !== typesafeRequestedModel) return false;
    if (typesafeReplay) {
      const transport = jev.configuration?.transport;
      const endpointMatches = mode === "typesafe-real" ? transport?.endpoint === "https://api.typesafe.ai/v1/systemone" :
        matches(transport?.endpoint, /^http:\/\/127\.0\.0\.1:[0-9]{1,5}\/v1\/systemone$/);
      if (!endpointMatches || transport.credential_present !== true || transport.automatic_retry !== false ||
        transport.allow_loopback_http !== (mode !== "typesafe-real") || jev.config_sha256 !== expected.configuration_sha256 ||
        jev.model !== expected.returned_model) return false;
      if (judged === "unavailable_or_invalid") return jev.outcome === undefined;
      return judged === "evaluated" && jev.outcome === expected.provider_outcome &&
        Number.isFinite(jev.confidence) && jev.confidence >= 0.85 && jev.confidence <= 1 &&
        jev.confidence_provenance === "provider_declared_not_calibrated";
    }
    if (judged === "skipped_local_policy") return jev.model === null && jev.outcome === undefined;
    return judged === "evaluated" && jev.model === typesafeReturnedModel && jev.outcome === expected.outcome &&
      jev.usage_provenance === "provider_declared" && jev.confidence_provenance === "provider_declared_not_calibrated";
  }
  if (jev.requested_model !== qwenModel) return false;
  if (judged === "evaluated") {
    if (jev.model !== qwenModel || jev.qualification !== "not_qualified_to_authorize" ||
      jev.coverage !== "top_level_advisory_only") return false;
    if (mode !== "local-real") return jev.provider_outcome === "allow" && jev.outcome === "review";
    return jev.provider_outcome === expected.provider_outcome && jev.outcome === expected.advisory_outcome &&
      jev.config_sha256 === expected.configuration_sha256 && jev.confidence === null && jev.usage_provenance === "provider_declared" &&
      isDeepStrictEqual(jev.usage, expected.usage) && jev.configuration?.max_tokens === 256 &&
      jev.configuration.requested_model === qwenModel && jev.configuration.stream === false;
  }
  if (mode === "local-real" && judged === "unavailable_or_invalid") {
    const configuration = jev.configuration;
    const transport = configuration?.transport;
    return validLocalFailure(expected) && jev.model === null && jev.usage === null && jev.confidence === null &&
      jev.provider_outcome === undefined && jev.outcome === undefined && jev.error === expected.backend_error &&
      isDeepStrictEqual(jev.diagnostic ?? null, expected.transport_diagnostic) &&
      jev.config_sha256 === expected.configuration_sha256 && configuration?.max_tokens === 256 &&
      configuration.requested_model === qwenModel && configuration.stream === false &&
      configuration.response_format?.type === "json_object" &&
      matches(transport?.endpoint, /^http:\/\/127\.0\.0\.1:[0-9]{1,5}\/v1\/chat\/completions$/) &&
      transport.timeout_seconds === expected.model_timeout_seconds && transport.automatic_retry === false &&
      transport.credential_present === false && transport.allow_loopback_http === true;
  }
  return jev.model === null && jev.provider_outcome === undefined &&
    (judged !== "unavailable_or_invalid" || jev.diagnostic?.http_status === 503);
}

export function validateAuditEvents(events, proof) {
  check(Array.isArray(events) && events.every((event) => event && typeof event === "object"),
    "Le journal authentifié est invalide.");
  const foreign = proof.foreign_decision;
  check(!events.some((event) => event.id === foreign.id || event.hash === foreign.hash ||
    event.session_id === foreign.session_id || event.data?.request_id === foreign.request_id),
    "Une décision du tenant B est visible sous l’identité du tenant A.");
  const decisions = events.filter((event) => event.kind === "security.decision");
  check(decisions.length === proof.decisions.length, "Le journal du tenant A ne contient pas exactement les décisions prévues.");
  return proof.decisions.map((expected) => {
    const event = decisions.find((candidate) => eventMatches(candidate, expected));
    check(event && observedEvaluation(event, expected, proof.judge_mode),
      "La décision attendue est absente ou diffère de son audit observé.");
    return event;
  });
}

async function readJson(origin, path, viewer) {
  const response = await fetch(origin + path, {
    headers: { Accept: "application/json", ...(viewer ? { Authorization: "Bearer " + viewer } : {}) },
    credentials: "omit", redirect: "error", cache: "no-store", signal: AbortSignal.timeout(10000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new EvidenceError("Lecture HTTP en échec sur " + path.split("?")[0] + " (" + response.status + ").");
  }
  return response.json();
}

async function inspectDecision(page, event) {
  await page.getByRole("button", { name: "Voir l’événement " + event.seq, exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("heading", { name: "security.decision", exact: true }).waitFor();
  const field = (label) => dialog.locator(".key-value")
    .filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
  const metadata = event.data.jev;
  const localModel = metadata.backend === "local_openai";
  const hasModel = localModel || metadata.backend === "typesafe";
  const settled = metadata.status === "evaluated";
  const advice = hasModel && settled ? adviceLabel[localModel ? metadata.provider_outcome : metadata.outcome] : undefined;
  const evaluation = [["Évaluateur", evaluatorLabel[metadata.backend]],
    ["État de l’évaluation", evaluatorStatus[metadata.status]], ["Sélection de l’évaluateur", "Ligne de commande"]];
  if (hasModel) evaluation.push(["Modèle demandé", metadata.requested_model]);
  if (hasModel && settled) evaluation.push(["Modèle retourné", metadata.model]);
  if (advice) evaluation.push(["Avis du modèle", advice]);
  for (const [name, expected] of evaluation) {
    const value = field(name);
    check(await value.count() === 1 && await value.textContent() === expected,
      "Les métadonnées de l’évaluation diffèrent de l’audit sur le champ " + name + ".");
  }
  if (localModel)
    await dialog.getByText("Le modèle local intervient à titre consultatif. L’autorisation d’exécuter relève de la politique Rust.",
      { exact: true }).waitFor();
  if (!hasModel || !settled) {
    check(await field("Modèle retourné").count() === 0, "Le détail affiche un modèle qui n’a pas été retourné.");
  }
  if (!advice) check(await field("Avis du modèle").count() === 0, "Le détail affiche un avis qui n’est pas rendu par cette évaluation.");
  for (const [name, expected] of [["Moteur", event.decision.engine], ["Règle", event.decision.rule],
    ["Source", event.source], ["Identifiant", event.id], ["Session", event.session_id], ["Empreinte", event.hash]]) {
    check(await field(name).textContent() === expected, "Le détail diffère de l’audit sur le champ " + name + ".");
  }
  await dialog.getByText(event.decision.reason, { exact: true }).waitFor();
  await dialog.getByText(outcomeText[event.decision.outcome], { exact: true }).waitFor();
  await dialog.getByRole("tab", { name: "Données brutes", exact: true }).click();
  const raw = JSON.parse(await dialog.getByRole("tabpanel").locator("pre").textContent());
  check(isDeepStrictEqual(raw, event), "Les données brutes du détail diffèrent de l’audit.");
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
}

async function readExport(page) {
  const [download] = await Promise.all([
    page.waitForEvent("download"), page.getByRole("button", { name: "Exporter JSONL", exact: true }).click(),
  ]);
  const stream = await download.createReadStream();
  const chunks = [];
  let bytes = 0;
  for await (const chunk of stream) {
    bytes += chunk.length;
    check(bytes <= 1024 * 1024, "L’export du jeu de décision dépasse sa borne de vérification.");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

async function main() {
  let mode = "rust_decision_audit_web";
  let result;
  let stage = "preuve";
  let viewer;
  let browser;
  let context;
  let page;
  let violations = [];
  const browserHttpErrors = [];
  try {
    check(process.env.AI_GUARD_TEST_DECISION_PROOF, "AI_GUARD_TEST_DECISION_PROOF est requis.");
    const proof = validateDecisionProof(JSON.parse(await readFile(process.env.AI_GUARD_TEST_DECISION_PROOF, "utf8")));
    if (proof.judge_mode === "local-fixture") mode = "local_model_fixture_decision_audit_web";
    if (proof.judge_mode === "typesafe-fixture") mode = "typesafe_fixture_decision_audit_web";
    const localReal = proof.judge_mode === "local-real";
    const localFailure = localReal && proof.decisions[0].backend_status === "unavailable_or_invalid";
    if (localReal) mode = localFailure ? "local_real_model_fail_closed_audit_web" : "local_real_model_decision_audit_web";
    const typesafeReplay = typesafeReplayModes.has(proof.judge_mode);
    if (typesafeReplay) mode = proof.judge_mode === "typesafe-real" ?
      "typesafe_real_decision_audit_web" : "typesafe_replay_fixture_decision_audit_web";
    const sameHead = localReal || typesafeReplay;
    if (process.argv.includes("--check-proof")) {
      return { mode, status: "PASS", scope: "proof_shape_only" };
    }
    stage = "configuration";
    check(!process.env.DEBUG && !process.env.PWDEBUG && !process.env.NODE_DEBUG, "Désactivez les logs de débogage réseau et Playwright.");
    const origin = dashboardOrigin(process.env.AI_GUARD_TEST_DASHBOARD, sameHead);
    check(process.env.AI_GUARD_TEST_CONFIG, "AI_GUARD_TEST_CONFIG est requis.");
    viewer = (await readFile(join(process.env.AI_GUARD_TEST_CONFIG, "secrets", "viewer"), "utf8")).trim();
    check(viewer, "Le fichier viewer est vide.");

    stage = "identité, intégrité et décision du dashboard";
    const [health, identity, integrity, data] = await Promise.all([
      readJson(origin, "/healthz"), readJson(origin, "/api/v1/identity", viewer),
      readJson(origin, "/api/v1/integrity", viewer),
      readJson(origin, "/api/v1/events?limit=100&order=asc", viewer),
    ]);
    check(health?.service === "dashboard" && health.status === "ok", "Le dashboard Rust est indisponible.");
    check(identity?.tenant_id === proof.tenant_id && identity.role === "viewer", "Le rôle viewer du tenant A n’est pas authentifié.");
    check(integrity?.valid === true && matches(integrity.head_hash, sha256) &&
      Number.isSafeInteger(integrity.event_count) && integrity.event_count >= proof.decisions.length,
      "La chaîne d’audit du tenant A est invalide.");
    check(data?.next_cursor === null && data.items?.length === integrity.event_count, "Le journal lu est incomplet.");
    if (sameHead) check(integrity.head_hash === proof.audit_head, "La tête d’audit du dashboard diffère du reçu vérifié CLI.");
    const events = validateAuditEvents(data.items, proof);

    stage = "lancement Chromium";
    const require = createRequire(import.meta.url);
    try {
      let playwright;
      try { playwright = require("playwright"); }
      catch {
        const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
        if (!modules) throw new BrowserUnavailable("Playwright est requis pour ce parcours.");
        playwright = require(join(modules, "playwright"));
      }
      browser = await playwright.chromium.launch({ headless: true,
        executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined });
    } catch (error) {
      if (error instanceof BrowserUnavailable) throw error;
      throw new BrowserUnavailable("Playwright ou Chromium est indisponible pour ce parcours.");
    }
    context = await browser.newContext({ serviceWorkers: "block", acceptDownloads: true });
    context.setDefaultTimeout(12000);
    const guard = await installBrowserGuard(context, [origin], [viewer]);
    violations = guard.violations;
    page = await context.newPage();
    await guard.preparePage(page);
    let pageErrors = 0;
    page.on("pageerror", () => { pageErrors++; });
    page.on("response", (response) => {
      const url = new URL(response.url());
      if (response.status() >= 400 && url.origin === origin &&
        /^\/api\/(?:v1\/)?(?:identity|events|overview|agents|sandboxes|integrity|stream|export)$/.test(url.pathname) &&
        browserHttpErrors.length < 12)
        browserHttpErrors.push({ path: url.pathname, status: response.status() });
    });
    stage = "connexion viewer";
    await page.goto(origin);
    await page.getByLabel("Jeton de lecture").fill(viewer);
    await page.getByRole("button", { name: "Ouvrir la supervision", exact: true }).click();
    await page.getByText("Client : " + proof.tenant_id, { exact: true }).first().waitFor();
    stage = "lecture du journal Sécurité";
    await page.getByRole("link", { name: "Sécurité", exact: true }).click();
    const unavailable = page.getByRole("heading", { name: "Journal indisponible", exact: true });
    const firstDecision = page.getByRole("button", { name: "Voir l’événement " + events[0].seq, exact: true });
    await firstDecision.or(unavailable).first().waitFor();
    check(!await unavailable.isVisible(), "Le dashboard indique que le journal est indisponible.");
    stage = "détail des décisions Sécurité";
    for (const event of events) await inspectDecision(page, event);
    stage = "export JSONL du tenant A";
    const exported = await readExport(page);
    validateAuditEvents(exported, proof);
    check(isDeepStrictEqual(exported, data.items), "Le JSONL complet diffère du journal authentifié.");
    check(pageErrors === 0, "Une erreur JavaScript est survenue.");
    check(!violations.length, "Le navigateur a tenté une requête hors du parcours autorisé.");
    result = { mode, status: "PASS", tenant_id: proof.tenant_id, audit_verified: true, export_verified: true,
      foreign_decision_absent: true, decisions: proof.decisions.map(({ seq, id, hash, outcome, rule }) => ({ seq, id, hash, outcome, rule })),
      ...(sameHead ? { audit_head: integrity.head_hash, provider_outcome: proof.decisions[0].provider_outcome } : {}),
      ...(localFailure ? { backend_status: "unavailable_or_invalid" } : {}),
      ...(typesafeReplay ? { returned_model: proof.decisions[0].returned_model } : {}) };
  } catch (error) {
    const artifacts = await saveBrowserFailure(page, {
      directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR, secrets: [viewer],
    });
    result = { mode, status: error instanceof BrowserUnavailable ? "BLOCKED" : "FAIL", stage,
      reason: error instanceof EvidenceError || error instanceof BrowserUnavailable ? error.message : "L’étape n’a pas abouti.",
      ...(browserHttpErrors.length ? { browser_http_errors: browserHttpErrors } : {}),
      ...(artifacts ? { artifacts } : {}) };
  } finally {
    const cleanup = await closeBrowserResources(context, browser);
    if ((cleanup.length || violations.length) && result?.status !== "FAIL")
      result = { mode, status: "FAIL", stage, reason: "Interception réseau ou fermeture du navigateur en erreur." };
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const result = await main();
  process.stdout.write(JSON.stringify(result) + "\n");
  process.exitCode = result.status === "PASS" ? 0 : result.status === "BLOCKED" ? 2 : 1;
}
