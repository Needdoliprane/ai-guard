// Read-only browser evidence for one completed real CLI session.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { open, readFile, stat } from "node:fs/promises";
import { createRequire } from "node:module";
import { isAbsolute, join } from "node:path";
import { pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { closeBrowserResources, installBrowserGuard } from "./browser_live.mjs";
import { saveBrowserFailure } from "./browser_failure.mjs";

const require = createRequire(import.meta.url);
const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const nilUuid = "00000000-0000-0000-0000-000000000000";
const sha256 = /^[0-9a-f]{64}$/;
const maxExportBytes = 32 * 1024 * 1024;
const models = { codex: "gpt-6-luna", claude_code: "claude-haiku-5-5" };
class Blocked extends Error {}
class EvidenceError extends Error {
  constructor(message, httpStatus) {
    super(message);
    this.httpStatus = httpStatus;
  }
}

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Blocked(`${name} est requis.`);
  return value;
}

function localOrigin(name) {
  const value = required(name);
  try {
    const url = new URL(value);
    if (["http:", "https:"].includes(url.protocol) &&
      ["127.0.0.1", "[::1]", "localhost"].includes(url.hostname) &&
      url.pathname === "/" && !url.username && !url.password && !url.search && !url.hash)
      return url.origin;
  } catch { /* Signal the same configuration error for every invalid value. */ }
  throw new Blocked(`${name} doit désigner une origine loopback sans chemin ni credentials.`);
}

async function readJson(origin, path, viewer) {
  const headers = { Accept: "application/json" };
  if (viewer) headers.Authorization = `Bearer ${viewer}`;
  const response = await fetch(`${origin}${path}`, {
    headers, credentials: "omit", redirect: "error", cache: "no-store",
    signal: AbortSignal.timeout(15000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new EvidenceError(`Lecture ${path.split("?")[0]} en erreur HTTP ${response.status}.`, response.status);
  }
  return response.json();
}

async function sessionEvents(gateway, sandbox, viewer) {
  const events = [];
  const ids = new Set();
  let before = null;
  for (let index = 0; index < 20; index++) {
    const query = new URLSearchParams({ sandbox_id: sandbox, limit: "100" });
    if (before !== null) query.set("before", String(before));
    const page = await readJson(gateway, `/api/events?${query}`, viewer);
    assert.ok(Array.isArray(page.items), "Page d’audit invalide.");
    for (const event of page.items) {
      assert.ok(event.session_id === sandbox && event.sandbox_id === sandbox &&
        Number.isSafeInteger(event.seq) && event.seq > 0 && uuid.test(event.id ?? "") && event.id !== nilUuid &&
        !ids.has(event.id) && sha256.test(event.hash ?? ""),
      "Événement sans identité de session ou empreinte vérifiable.");
      assert.ok(!events.length || event.seq < events.at(-1).seq, "Séquence du journal non décroissante.");
      ids.add(event.id);
      events.push(event);
    }
    if (page.next_cursor === null) return events.reverse();
    assert.ok(Number.isSafeInteger(page.next_cursor) && page.next_cursor === page.items.at(-1)?.seq &&
      (before === null || page.next_cursor < before), "Curseur d’audit invalide.");
    before = page.next_cursor;
  }
  throw new EvidenceError("Le journal de la session dépasse la limite de lecture de la recette.");
}

export function providerProof(events, { transport, expectation, expectedFile, expectedFileSha256, expectWorkspaceExport }) {
  const requested = events.find((event) => event.kind === "command.requested");
  const decision = events.find((event) => event.kind === "security.decision" && event.decision?.outcome === "allow");
  const timedOut = expectation === "timeout";
  const terminalKind = timedOut ? "command.failed" : "command.completed";
  const terminal = events.find((event) => event.kind === terminalKind);
  const file = expectedFile ? events.find((event) => event.kind === "file.created" && event.data?.path === expectedFile) : null;
  const exported = timedOut ? null : events.find((event) => event.kind === "workspace.export.completed");
  assert.ok(requested && decision && terminal, "Demande, décision autorisée et résultat terminal de cette session sont requis.");
  if (expectedFile) assert.ok(file, "La création du fichier de recette doit être auditée.");
  if (expectWorkspaceExport) assert.ok(exported, "L’export du workspace doit être audité.");
  assert.ok(requested.seq < decision.seq && decision.seq < terminal.seq &&
    (!file || terminal.seq < file.seq) && (!exported || (file ?? terminal).seq < exported.seq),
  "Ordre causal des traces inattendu.");
  assert.ok(requested.data?.request_id && requested.data.request_id === decision.data?.request_id &&
    requested.correlation_id && [decision, terminal, file, exported].filter(Boolean).every((event) =>
      event.correlation_id === requested.correlation_id), "Lien demande, décision et session manquant.");
  assert.ok([terminal, file, exported].filter(Boolean).every((event) => event.provenance === "observed"),
    "Provenance observée requise pour les effets vérifiés.");
  assert.ok(terminal.data?.output_captured === false && uuid.test(terminal.data.worker_id ?? "") &&
    terminal.data.worker_id !== nilUuid && Number.isFinite(Date.parse(terminal.data.runtime_started_at)) &&
    Number.isFinite(Date.parse(terminal.data?.runtime_finished_at)) &&
    Date.parse(terminal.data.runtime_started_at) <= Date.parse(terminal.data.runtime_finished_at),
  "Dates de début et de fin runtime observées requises.");
  if (timedOut) {
    assert.ok(Number.isSafeInteger(terminal.result?.exit_code) && terminal.result.exit_code !== 0 &&
      terminal.result.timed_out === true &&
      terminal.result.transport_stop_reason === "authorized worker duration elapsed",
    "L’échec terminal doit attester l’expiration de la durée autorisée.");
  } else {
    assert.ok(terminal.result?.exit_code === 0 && terminal.result.timed_out === false,
      "Résultat terminal réussi requis.");
  }
  if (file) {
    assert.ok(sha256.test(expectedFileSha256 ?? "") && file.data?.after?.sha256 === expectedFileSha256,
      "L’empreinte auditée diffère de celle du fichier exporté.");
  }
  if (exported) {
    const count = exported.data?.files;
    assert.ok(Number.isSafeInteger(count) && count >= (transport === "batch" ? 1 : 0) &&
      exported.data.worker_id === terminal.data.worker_id && sha256.test(exported.data.manifest_digest ?? ""),
    "L’export doit porter le même worker, le compte de fichiers et l’empreinte du manifeste.");
  }
  const resolution = decision.data?.provider_resolution;
  assert.ok(resolution && ["codex", "claude_code"].includes(resolution.client) &&
    ["api", "subscription"].includes(resolution.auth_mode) &&
    resolution.model === models[resolution.client] &&
    isDeepStrictEqual(resolution, requested.data?.provider_resolution),
  "La résolution fournisseur doit être conservée dans la demande et dans l’autorisation.");
  assert.ok(resolution.transport === (transport === "tty" ? "interactive_tty" : "batch"),
    "Le transport audité diffère du mode de la recette.");
  if (transport === "tty") {
    const executionMode = "interactive_tty_v1";
    assert.ok(requested.data.execution_mode === executionMode && decision.data.execution_mode === executionMode &&
      terminal.result.execution_mode === executionMode &&
      (timedOut || terminal.result.transport_stop_reason === null),
    "La demande, l’autorisation et le résultat doivent attester la même session TTY terminée.");
  }
  for (const [name, recorded] of [
    ["AI_GUARD_TEST_CLIENT", resolution.client],
    ["AI_GUARD_TEST_AUTH_MODE", resolution.auth_mode],
    ["AI_GUARD_TEST_MODEL", resolution.model],
    ["AI_GUARD_TEST_RUNTIME", requested.data?.runtime],
  ]) {
    if (process.env[name]) assert.ok(process.env[name] === recorded, `${name} diffère de la valeur auditée.`);
  }
  return { requested, decision, terminal, file, exported, resolution };
}

export function validateBrokerProof(proof) {
  const identifiers = ["operation_id", "session_id", "export_command_id", "export_request_id"];
  const digests = ["container_id", "scan_manifest_sha256", "export_manifest_sha256", "file_sha256"];
  const seqs = ["result_audit_seq", "scan_audit_seq", "cleanup_audit_seq"];
  const fields = ["version", ...identifiers, ...digests, ...seqs, "export_file_count", "file_name",
    "client", "auth_mode", "client_profile", "model_requested", "runtime", "worker_image_id"];
  assert.ok(proof && typeof proof === "object" && Object.keys(proof).length === fields.length &&
    fields.every((field) => Object.hasOwn(proof, field)) && proof.version === 1,
  "Contrat de preuve du broker incompatible.");
  assert.ok(identifiers.every((field) => typeof proof[field] === "string" && proof[field].length === 36 && uuid.test(proof[field]) &&
    proof[field] !== nilUuid) && proof.export_command_id === proof.export_request_id &&
    digests.every((field) => typeof proof[field] === "string" && proof[field].length === 64 && sha256.test(proof[field])) &&
    typeof proof.worker_image_id === "string" && /^sha256:[0-9a-f]{64}$/.test(proof.worker_image_id) &&
    proof.worker_image_id.length === 71, "Identités et empreintes du reçu broker invalides.");
  assert.ok(seqs.every((field) => Number.isSafeInteger(proof[field]) && proof[field] > 0 &&
    proof[field] < Number.MAX_SAFE_INTEGER) && proof.result_audit_seq < proof.cleanup_audit_seq &&
    proof.cleanup_audit_seq < proof.scan_audit_seq, "Séquences des preuves broker invalides.");
  assert.ok(Number.isSafeInteger(proof.export_file_count) && proof.export_file_count > 0 &&
    typeof proof.file_name === "string" && proof.file_name.length <= 4096 && !/[\\\u0000-\u001f]/u.test(proof.file_name) &&
    proof.file_name.split("/").every((part) => part && part !== "." && part !== "..") &&
    Object.hasOwn(models, proof.client) && proof.model_requested === models[proof.client] &&
    ["api", "subscription"].includes(proof.auth_mode) && ["docker", "podman"].includes(proof.runtime) &&
    typeof proof.client_profile === "string" && proof.client_profile.trim() === proof.client_profile &&
    proof.client_profile.length > 0 && proof.client_profile.length <= 128, "Sélection fournisseur ou fichier de recette invalide.");
  return proof;
}

async function readBrokerProof(path) {
  if (!isAbsolute(path)) throw new Blocked("Le chemin de la preuve broker doit être absolu.");
  let file;
  try {
    file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const facts = await file.stat();
    if (!facts.isFile() || facts.uid !== process.getuid() || (facts.mode & 0o077) || facts.size > 16384)
      throw new Blocked("La preuve broker doit être un fichier privé, régulier et borné de la recette.");
    return validateBrokerProof(JSON.parse(await file.readFile("utf8")));
  } finally {
    await file?.close();
  }
}

export function brokerProviderProof(events, cleanup, worker, expected) {
  const at = (seq, kind) => events.find((event) => event.seq === seq && event.kind === kind);
  const terminal = at(expected.result_audit_seq, "broker.batch.result");
  const scan = at(expected.scan_audit_seq, "broker.batch.scan");
  const exports = events.filter((event) => event.kind === "broker.batch.export" &&
    event.data?.request_id === expected.export_request_id);
  const policies = events.filter((event) => event.kind === "workspace.export.policy" &&
    event.data?.request_id === expected.export_request_id);
  const [exported] = exports;
  const [exportPolicy] = policies;
  const decision = at(worker?.receipt?.audit_seq, "security.decision");
  assert.ok(terminal && scan && exports.length === 1 && policies.length === 1 && decision && cleanup &&
    [terminal, scan, exported, exportPolicy, decision, cleanup].every((event) =>
      event.session_id === expected.session_id && event.provenance === "observed"),
  "Les preuves auditables de cette session broker sont incomplètes.");
  assert.ok(decision.seq < terminal.seq && scan.seq < exportPolicy.seq &&
    exportPolicy.seq < exported.seq && terminal.correlation_id &&
    [scan, exportPolicy, exported, decision].every((event) => event.correlation_id === terminal.correlation_id),
  "L’ordre et la corrélation durable des faits broker diffèrent du reçu.");
  const data = terminal.data;
  const observation = data?.observation;
  const process = observation?.process;
  assert.ok(terminal.source === "runner" && data?.version === 1 && uuid.test(data.worker_id ?? "") &&
    data.worker_id !== nilUuid && uuid.test(data.attempt_id ?? "") && data.attempt_id !== nilUuid &&
    observation?.id === expected.container_id && observation.image_id === expected.worker_image_id &&
    (process?.status === "exited" || expected.runtime === "podman" && process?.status === "stopped") &&
    process.running === false &&
    process.exit_code === 0 && process.oom_killed === false &&
    Number.isFinite(Date.parse(process.started_at)) && Number.isFinite(Date.parse(process.finished_at)) &&
    Date.parse(process.started_at) <= Date.parse(process.finished_at) &&
    (!data.transport || (data.transport.stdout?.text === null && data.transport.stderr?.text === null)),
  "Le résultat broker ne prouve pas la fin normale de ce conteneur et l’absence de capture.");
  const launch = worker?.launch;
  const inspected = worker?.last_observation;
  const inspection = inspected?.inspection;
  const resolution = launch?.provider_resolution;
  assert.ok(worker?.worker_id === data.worker_id && worker.admission_operation_id === expected.operation_id &&
    worker.attempt?.attempt_id === data.attempt_id &&
    launch?.session_id === expected.session_id && worker.attempt.container_id === expected.container_id &&
    worker.state === "terminal" && worker.quota_held === false && worker.cleanup_removed === true &&
    worker.cleanup_error === null && launch.capture_output === false && launch.runtime_kind === expected.runtime &&
    launch.image_id === expected.worker_image_id && uuid.test(launch.authority_id ?? "") && launch.authority_id !== nilUuid &&
    inspection?.authority_id === launch.authority_id && inspection.runtime_kind === expected.runtime &&
    inspection.container_id === expected.container_id && inspection.image_id === expected.worker_image_id &&
    sha256.test(worker.spec_hash ?? "") && inspection.spec_hash === worker.spec_hash &&
    ["exited", "dead"].includes(inspected.status) && inspected.running === false &&
    inspected.exit_code === 0 && inspected.oom_killed === false,
  "L’instance, la provenance et le nettoyage du worker divergent du résultat broker.");
  assert.ok(decision.source === "gateway" && decision.decision?.outcome === "allow" &&
    worker.receipt?.version === 1 && worker.receipt.audit_hash === decision.hash &&
    uuid.test(worker.request_id ?? "") && worker.request_id !== nilUuid && worker.receipt.request_id === worker.request_id &&
    decision.data?.request_id === worker.request_id && sha256.test(worker.receipt.request_hash ?? "") &&
    decision.data.request_hash === worker.receipt.request_hash &&
    resolution?.client === expected.client && resolution.auth_mode === expected.auth_mode &&
    resolution.profile_id === expected.client_profile && resolution.model === expected.model_requested &&
    resolution.transport === "batch" && isDeepStrictEqual(resolution, decision.data.provider_resolution),
  "L’autorisation durable ne lie pas ce worker et la même résolution fournisseur.");
  const cleaned = cleanup.data?.worker;
  assert.ok(cleanup.seq === expected.cleanup_audit_seq && cleanup.kind === "control.worker.transition" &&
    cleanup.source === "gateway" && cleanup.sandbox_id == null && uuid.test(cleanup.id ?? "") &&
    cleanup.id !== nilUuid && sha256.test(cleanup.hash ?? "") &&
    cleanup.data?.version === 1 && cleanup.data.action === "cleanup_result" && cleaned?.worker_id === data.worker_id &&
    cleaned.attempt?.attempt_id === data.attempt_id && cleaned.launch?.session_id === expected.session_id &&
    cleaned.cleanup_removed === true && cleaned.cleanup_error === null,
  "Le reçu de nettoyage ne correspond pas à cette tentative.");
  const change = scan.data?.changes?.[0];
  assert.ok(scan.source === "runner" && scan.data?.version === 1 && scan.data.worker_id === data.worker_id &&
    scan.data.manifest_digest === expected.scan_manifest_sha256 && scan.data.changed_files === 1 &&
    scan.data.truncated === false && Array.isArray(scan.data.changes) && scan.data.changes.length === 1 &&
    change?.path === expected.file_name && change.change === "created" && change.before === null &&
    change.after?.sha256 === expected.file_sha256, "Le scan ne confirme pas la création du fichier et son empreinte.");
  assert.ok(exported.source === "runner" && exported.data?.version === 1 &&
    exported.data.destination_ref === expected.export_request_id &&
    exported.data.manifest_digest === expected.export_manifest_sha256 && exported.data.files === expected.export_file_count &&
    exportPolicy.source === "runner" && exportPolicy.data?.version === 1 &&
    exportPolicy.data.manifest_digest === expected.export_manifest_sha256,
  "La publication du workspace ne correspond pas à son reçu et à la politique d’export.");
  return { decision, terminal, scan, cleanup, exported, resolution, worker };
}

async function readBrokerAudit(gateway, viewer, events, expected) {
  const terminal = events.find((event) => event.seq === expected.result_audit_seq && event.kind === "broker.batch.result");
  const id = terminal?.data?.worker_id;
  assert.ok(typeof id === "string" && id.length === 36 && uuid.test(id) && id !== nilUuid,
    "Le résultat broker doit identifier un worker durable.");
  const seq = expected.cleanup_audit_seq;
  const [worker, cleanup] = await Promise.all([
    readJson(gateway, "/api/v1/workers/" + id, viewer),
    readJson(gateway, "/api/events?after=" + (seq - 1) + "&before=" + (seq + 1) + "&limit=2", viewer),
  ]);
  assert.ok(Array.isArray(cleanup?.items) && cleanup.items.length === 1 && cleanup.next_cursor === null,
    "Le reçu broker doit désigner une seule trace de nettoyage.");
  return brokerProviderProof(events, cleanup.items[0], worker, expected);
}

export async function verifyTimeoutLabels(page, dialog, result) {
  assert.ok(result?.timed_out === true && typeof result.transport_stop_reason === "string" &&
    result.transport_stop_reason.trim(), "Le délai et le motif doivent être attestés par l’événement audité.");
  for (const [label, value] of [
    ["État de la commande", "Délai dépassé"],
    ["Motif de l’arrêt", result.transport_stop_reason],
  ]) {
    const field = dialog.locator(".key-value").filter({
      has: page.getByText(label, { exact: true }),
    }).locator("dd");
    await field.waitFor();
    assert.ok(await field.textContent() === value, `Champ Commande absent ou différent de l’audit : ${label}.`);
  }
}

async function verifyBrokerWorkspace(page, dialog, event, expected) {
  const scan = event.kind === "broker.batch.scan";
  await dialog.getByRole("heading", { name: scan ? "Modifications observées" : "Export publié", exact: true }).waitFor();
  const fields = scan ? [["Nombre de modifications", "1"], ["Empreinte du manifeste", expected.scan_manifest_sha256]]
    : [["Fichiers publiés", String(expected.export_file_count)], ["Empreinte du manifeste", expected.export_manifest_sha256],
      ["Demande d’export", expected.export_request_id]];
  for (const [label, value] of fields) {
    const field = dialog.locator(".key-value").filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
    assert.ok(await field.textContent() === value, "Le détail Fichiers diffère de son audit sur le champ " + label + ".");
  }
  if (scan) {
    const table = dialog.getByRole("table");
    assert.ok(isDeepStrictEqual(await table.getByRole("columnheader").allTextContents(),
      ["Chemin", "Changement", "Empreinte avant", "Empreinte après"]), "Colonnes attendues du tableau des modifications.");
    const row = table.getByRole("row").filter({ has: page.getByRole("cell", { name: expected.file_name, exact: true }) });
    assert.ok(isDeepStrictEqual(await row.getByRole("cell").allTextContents(),
      [expected.file_name, "Créé", "—", expected.file_sha256]), "Le détail ne montre pas ce fichier créé et son empreinte.");
  }
}

async function verifyBrokerResult(page, dialog, event) {
  await dialog.getByRole("tab", { name: "Commande", exact: true }).click();
  const observed = event.data.observation.process;
  const transport = event.data.transport;
  const capture = (channel) => channel?.text === null ? "Texte non conservé" :
    typeof channel?.text === "string" ? "Texte conservé" : "Inconnue";
  const fields = [
    ["Code de sortie observé", String(observed.exit_code)],
    ["Arrêt mémoire (OOM)", observed.oom_killed === true ? "Oui" : observed.oom_killed === false ? "Non" : "Inconnu"],
    ["Code de sortie du transport", transport?.exit_code == null ? "Inconnu" : String(transport.exit_code)],
    ["Durée du transport", transport?.duration_ms_decimal == null ? "Inconnu" : transport.duration_ms_decimal + " ms"],
    ["Capture de la sortie standard", capture(transport?.stdout)],
    ["Capture de la sortie d’erreur", capture(transport?.stderr)],
  ];
  for (const [label, value] of fields) {
    const field = dialog.locator(".key-value").filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
    assert.ok(await field.count() === 1 && await field.textContent() === value,
      "Le détail Commande diffère du résultat broker sur le champ " + label + ".");
  }
}

async function openRawEvent(page, event, { timedOut = false, expectTimeoutLabels = false, workspace = null, brokerResult = false } = {}) {
  const button = page.getByRole("button", { name: `Voir l’événement ${event.seq}`, exact: true });
  for (let index = 0; index < 20; index++) {
    if (await button.count()) {
      await button.click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("heading", { name: event.kind, exact: true }).waitFor();
      if (workspace) await verifyBrokerWorkspace(page, dialog, event, workspace);
      if (brokerResult) await verifyBrokerResult(page, dialog, event);
      if (timedOut) {
        stage = "code de sortie dans le détail Commande";
        await dialog.getByRole("tab", { name: "Commande", exact: true }).click();
        const exitCode = dialog.locator(".key-value").filter({
          has: page.getByText("Code de sortie", { exact: true }),
        });
        await exitCode.getByText(String(event.result.exit_code), { exact: true }).waitFor();
        if (expectTimeoutLabels) {
          stage = "délai et motif lisibles dans le détail Commande";
          await verifyTimeoutLabels(page, dialog, event.result);
          checks.push(stage);
        }
        stage = "données brutes du résultat terminal";
      }
      await dialog.getByRole("tab", { name: "Données brutes", exact: true }).click();
      const shown = JSON.parse(await dialog.locator("pre").textContent());
      assert.ok(isDeepStrictEqual(shown, event), "Détail affiché différent de la trace d’audit attendue.");
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "hidden" });
      return;
    }
    const next = page.getByRole("button", { name: "Charger les précédents", exact: true });
    assert.ok(await next.count(), "Trace attendue absente des pages du journal affiché.");
    await next.click();
    await page.locator('.event-slot[aria-busy="false"]').waitFor();
    assert.equal(await page.getByText("Journal indisponible", { exact: true }).count(), 0);
  }
  throw new EvidenceError("Trace au-delà de la limite de pagination du navigateur.");
}

async function checkWorkerProvenance(page, gateway, viewer, session, workerId, brokerWorker = null) {
  const worker = brokerWorker ?? await readJson(gateway, `/api/v1/workers/${workerId}`, viewer);
  assert.ok(worker?.worker_id === workerId && worker.launch?.session_id === session,
    "Le registre Workers doit identifier la même session et la même instance.");
  const inspection = worker.last_observation?.inspection;
  const expected = [
    ["Autorité déclarée du lancement", worker.launch?.authority_id],
    ["Empreinte de l’image de lancement", worker.launch?.image_id],
    ["Empreinte du plan de lancement", worker.spec_hash],
    ["Autorité de la dernière inspection enregistrée", inspection?.authority_id],
    ["Runtime de l’inspection enregistrée", inspection?.runtime_kind],
    ["Conteneur inspecté", inspection?.container_id],
    ["Empreinte de l’image inspectée", inspection?.image_id],
    ["Empreinte du plan liée à l’inspection", inspection?.spec_hash],
  ];
  if (brokerWorker) expected.push(["Tentative", worker.attempt.attempt_id], ["Conteneur", worker.attempt.container_id],
    ["Session", worker.launch.session_id], ["Code de sortie", String(worker.last_observation.exit_code)], ["Retrait constaté", "Oui"]);
  assert.ok(expected.every(([, value]) => typeof value === "string" && value),
    "Le registre doit fournir la provenance du lancement et de la dernière inspection.");
  await page.getByRole("link", { name: "Workers", exact: true }).click();
  const row = page.locator(".control-view tbody tr").filter({ hasText: workerId });
  await row.getByRole("button", { name: "Détails", exact: true }).click();
  const dialog = page.getByRole("dialog");
  if (brokerWorker) {
    await dialog.getByText("Terminal observé", { exact: true }).waitFor();
    await dialog.getByText("Quota libéré", { exact: true }).waitFor();
  }
  for (const [label, value] of expected) {
    const field = dialog.locator(".key-value").filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
    try {
      await field.waitFor();
      if (await field.textContent() !== value) throw new Error("worker field differs");
    } catch {
      throw new EvidenceError(`Champ Workers absent ou différent de l’API : ${label}.`);
    }
  }
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
}

let browser;
let context;
let guard;
let viewer;
let page;
let stage = "configuration";
const checks = [];

async function addFailureArtifacts(result) {
  if (result.artifacts) return;
  const artifacts = await saveBrowserFailure(page, {
    directory: process.env.AI_GUARD_TEST_ARTIFACT_DIR, secrets: [viewer],
  });
  if (artifacts) result.artifacts = artifacts;
}

async function main() {
  const flags = process.argv.slice(2);
  const preflight = flags.length === 1 && flags[0] === "--preflight";
  if (flags.length && !preflight) throw new Blocked("Seul --preflight est accepté.");
  const broker = !preflight && process.env.AI_GUARD_TEST_BROKER_PROOF !== undefined
    ? await readBrokerProof(required("AI_GUARD_TEST_BROKER_PROOF")) : null;
  const transport = process.env.AI_GUARD_TEST_TRANSPORT ?? "batch";
  if (!["batch", "tty"].includes(transport))
    throw new Blocked("AI_GUARD_TEST_TRANSPORT doit valoir batch ou tty.");
  const expectation = process.env.AI_GUARD_TEST_EXPECTATION ?? "success";
  if (expectation !== "success" && !(expectation === "timeout" && transport === "tty"))
    throw new Blocked("AI_GUARD_TEST_EXPECTATION doit valoir success ou timeout, ce dernier en TTY.");
  if (broker && (transport !== "batch" || expectation !== "success"))
    throw new Blocked("La preuve broker requiert un batch terminé avec succès.");
  const timedOut = expectation === "timeout";
  const exportFlag = process.env.AI_GUARD_TEST_EXPECT_WORKSPACE_EXPORT ?? "0";
  if (!["0", "1"].includes(exportFlag) || (timedOut && exportFlag === "1"))
    throw new Blocked("AI_GUARD_TEST_EXPECT_WORKSPACE_EXPORT doit valoir 0 ou 1 et rester à 0 en mode timeout.");
  const labelsFlag = process.env.AI_GUARD_TEST_EXPECT_TIMEOUT_LABELS ?? "0";
  if (!["0", "1"].includes(labelsFlag) || (!timedOut && labelsFlag === "1"))
    throw new Blocked("AI_GUARD_TEST_EXPECT_TIMEOUT_LABELS doit valoir 0 ou 1 et ne s’active qu’en mode timeout.");
  const expectTimeoutLabels = labelsFlag === "1";
  const workerFlag = process.env.AI_GUARD_TEST_WORKER_PROVENANCE ?? "0";
  if (!["0", "1"].includes(workerFlag))
    throw new Blocked("AI_GUARD_TEST_WORKER_PROVENANCE doit valoir 0 ou 1.");
  const expectWorkerProvenance = workerFlag === "1" || broker !== null;
  const expectWorkspaceExport = transport === "batch" || exportFlag === "1";
  const gateway = localOrigin("AI_GUARD_TEST_GATEWAY");
  const dashboard = localOrigin("AI_GUARD_TEST_DASHBOARD");
  let session;
  let expectedFile;
  let expectedFileSha256;
  if (!preflight) {
    session = broker?.session_id ?? required("AI_GUARD_TEST_SESSION_ID");
    if (!uuid.test(session) || session === nilUuid) throw new Blocked("Un UUID de session est requis.");
    if (transport === "batch" && !broker) {
      expectedFile = required("AI_GUARD_TEST_EXPECTED_FILE");
      if (expectedFile.includes("\\") || expectedFile.includes("\0") ||
        expectedFile.split("/").some((part) => !part || part === "." || part === ".."))
        throw new Blocked("Un chemin relatif de fichier de recette est requis.");
      expectedFileSha256 = required("AI_GUARD_TEST_EXPECTED_FILE_SHA256");
      if (!sha256.test(expectedFileSha256)) throw new Blocked("L’empreinte SHA-256 du fichier exporté est requise.");
    }
  }
  const config = required("AI_GUARD_TEST_CONFIG");
  try { viewer = (await readFile(join(config, "secrets", "viewer"), "utf8")).trim(); }
  catch { throw new Blocked("Jeton viewer requis dans AI_GUARD_TEST_CONFIG/secrets/viewer."); }
  if (!viewer) throw new Blocked("Le jeton viewer de la recette est vide.");
  stage = "services HTTP";
  try {
    for (const [origin, service] of [[gateway, "gateway"], [dashboard, "dashboard"]])
      assert.equal((await readJson(origin, "/healthz")).service, service);
  } catch { throw new Blocked("Les services Rust doivent être joignables sur leurs origines explicites."); }
  checks.push(stage);
  let events;
  let proof;
  if (broker) {
    stage = "audit durable de la session broker";
    events = await sessionEvents(gateway, session, viewer);
    proof = await readBrokerAudit(gateway, viewer, events, broker);
    checks.push(stage);
  }

  stage = "Playwright et Chromium";
  let chromium;
  try {
    const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES;
    try { ({ chromium } = require("playwright")); }
    catch { ({ chromium } = require(join(modules, "playwright"))); }
    browser = await chromium.launch({
      headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined,
    });
  } catch { throw new Blocked("Playwright et un exécutable Chromium utilisable sont requis."); }
  context = await browser.newContext({ serviceWorkers: "block", acceptDownloads: true });
  context.setDefaultTimeout(20000);
  try {
    guard = await installBrowserGuard(context, [dashboard], [viewer]);
    page = await context.newPage();
    await guard.preparePage(page);
  } catch { throw new Blocked("Le garde réseau du parcours requiert un Chromium et un Playwright compatibles."); }
  const pageErrors = [];
  page.on("pageerror", () => pageErrors.push("exception navigateur"));
  await page.goto(dashboard, { waitUntil: "domcontentloaded" });
  await page.getByLabel("Jeton de lecture", { exact: true }).waitFor();
  assert.equal(pageErrors.length, 0, "Exception JavaScript à l’ouverture du tableau de bord.");
  checks.push(stage);
  stage = "identité viewer";
  const [identity, proxiedIdentity] = await Promise.all(
    [gateway, dashboard].map((origin) => readJson(origin, "/api/v1/identity", viewer)));
  assert.ok(identity?.role === "viewer" && typeof identity.tenant_id === "string", "Identité viewer attendue.");
  assert.ok(isDeepStrictEqual(proxiedIdentity, identity), "Le dashboard et la gateway doivent attester la même identité.");
  if (preflight && expectWorkerProvenance) {
    stage = "provenance du worker";
    // Read through the authenticated proxy before opening the live views and their concurrent reads.
    let workers;
    try {
      workers = await readJson(dashboard, "/api/v1/federation/workers?limit=1", viewer);
    } catch (error) {
      if (error instanceof EvidenceError && Number.isInteger(error.httpStatus)) {
        throw new Blocked(`Lecture du registre Workers indisponible (HTTP ${error.httpStatus}).`);
      }
      if (error instanceof SyntaxError) throw new Blocked("Réponse JSON invalide pour le registre Workers.");
      throw new Blocked("Erreur de transport à la lecture du registre Workers.");
    }
    if (workers?.source !== "tenant_worker_broker_registry" || !Array.isArray(workers.items)) {
      throw new Blocked("Format du registre Workers invalide (source ou items).");
    }
    stage = "identité viewer";
  }
  await page.getByLabel("Jeton de lecture", { exact: true }).fill(viewer);
  await page.getByRole("button", { name: "Ouvrir la supervision", exact: true }).click();
  await page.getByRole("heading", { name: "Vue d’ensemble", exact: true }).waitFor();
  await page.locator(".topbar").getByText(`Client : ${identity.tenant_id}`, { exact: true }).waitFor();
  checks.push(stage);
  if (preflight) {
    if (expectWorkerProvenance) checks.push("provenance du worker");
    return { mode: "preflight", browser: browser.version() };
  }

  if (!broker) {
    stage = "journal de la session";
    events = await sessionEvents(gateway, session, viewer);
    proof = providerProof(events, { transport, expectation, expectedFile, expectedFileSha256, expectWorkspaceExport });
    checks.push(stage);
  }

  stage = "sélection de la sandbox";
  const sandbox = page.getByRole("combobox", { name: "Toutes les sandboxes", exact: true });
  await sandbox.selectOption(session);
  stage = "application du filtre de la session";
  const [filtered] = await Promise.all([page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.origin === dashboard && url.pathname === "/api/events" && url.searchParams.get("sandbox_id") === session;
  }), page.getByRole("button", { name: "Filtrer", exact: true }).click()]);
  assert.equal(filtered.status(), 200, "Filtre de session relayé par le tableau de bord.");
  await page.locator('.event-slot[aria-busy="false"]').waitFor();
  stage = "détail de la décision";
  if (broker) await page.getByRole("link", { name: "Sécurité", exact: true }).click();
  else if (timedOut) await page.getByRole("link", { name: "Actions", exact: true }).click();
  await openRawEvent(page, proof.decision);
  stage = "ouverture du résultat terminal";
  if (broker) await page.getByRole("link", { name: "Actions", exact: true }).click();
  await openRawEvent(page, proof.terminal, { timedOut, expectTimeoutLabels, brokerResult: broker !== null });
  checks.push("décision et résultat dans le journal");

  if (broker) {
    stage = "scan et export supervisés dans la vue Fichiers";
    await page.getByRole("link", { name: "Fichiers", exact: true }).click();
    await openRawEvent(page, proof.scan, { workspace: broker });
    await openRawEvent(page, proof.exported, { workspace: broker });
    checks.push(stage);
  } else if (proof.file || proof.exported) {
    stage = proof.file ? "fichier et export dans la vue Fichiers" : "export dans la vue Fichiers";
    await page.getByRole("link", { name: "Fichiers", exact: true }).click();
    if (proof.file) await openRawEvent(page, proof.file);
    if (proof.exported) await openRawEvent(page, proof.exported);
    checks.push(stage);
  }

  stage = "téléchargement JSONL de la session";
  const [download, exportResponse] = await Promise.all([
    page.waitForEvent("download"),
    page.waitForResponse((response) => {
      const url = new URL(response.url());
      return url.origin === dashboard && url.pathname === "/api/export" &&
        url.searchParams.get("sandbox_id") === session && url.searchParams.get("format") === "jsonl";
    }),
    page.getByRole("button", { name: "Exporter JSONL", exact: true }).click(),
  ]);
  assert.equal(exportResponse.status(), 200, "Export de session relayé par le tableau de bord.");
  const path = await download.path();
  assert.ok(download.suggestedFilename().endsWith(".jsonl") && (await stat(path)).size <= maxExportBytes,
    "Téléchargement JSONL absent ou trop volumineux.");
  const bytes = await readFile(path);
  assert.ok(![viewer, encodeURIComponent(viewer)].some((token) => bytes.includes(Buffer.from(token))),
    "Le téléchargement contient le jeton de lecture.");
  const entries = bytes.toString("utf8").trim().split("\n").map((line) => JSON.parse(line));
  assert.ok(isDeepStrictEqual(entries, events), "Le téléchargement diffère du journal complet de la session.");
  assert.equal(pageErrors.length, 0, "Exception JavaScript dans le parcours.");
  checks.push(stage);
  const workerId = proof.terminal.data.worker_id;
  if (expectWorkerProvenance) {
    stage = "provenance du worker";
    await checkWorkerProvenance(page, gateway, viewer, session, workerId, proof.worker);
    assert.equal(pageErrors.length, 0, "Exception JavaScript dans la vue Workers.");
    checks.push(stage);
  }
  if (broker) {
    return { mode: "broker_session_trace", executionPath: "broker_batch", transport, browser: browser.version(),
      operationId: broker.operation_id, session, workerId, authorityId: proof.worker.launch.authority_id,
      containerId: broker.container_id, workerImageId: broker.worker_image_id, client: proof.resolution.client,
      authMode: proof.resolution.auth_mode, clientProfile: proof.resolution.profile_id,
      requestedModel: proof.resolution.model, runtime: proof.worker.launch.runtime_kind,
      exportRequestId: broker.export_request_id, fileSha256: broker.file_sha256,
      scanManifestSha256: broker.scan_manifest_sha256, exportManifestSha256: broker.export_manifest_sha256,
      decisionEventId: proof.decision.id, terminalEventId: proof.terminal.id, scanEventId: proof.scan.id,
      cleanupEventId: proof.cleanup.id, exportEventId: proof.exported.id,
      decisionAuditSeq: proof.decision.seq, resultAuditSeq: proof.terminal.seq, scanAuditSeq: proof.scan.seq,
      cleanupAuditSeq: proof.cleanup.seq, exportAuditSeq: proof.exported.seq,
      auditExportSha256: createHash("sha256").update(bytes).digest("hex"), count: events.length,
      workerProvenance: "displayed_fields_match_registry" };
  }
  return {
    mode: timedOut ? "session_timeout_trace" : "session_trace",
    transport, browser: browser.version(), session, count: events.length,
    ...(timedOut ? { workerOutcome: "timeout", timeoutLabelsVerified: expectTimeoutLabels,
      terminalKind: proof.terminal.kind,
      terminalExitCode: proof.terminal.result.exit_code,
      transportStopReason: proof.terminal.result.transport_stop_reason } : {}),
    terminalEventId: proof.terminal.id, decisionEventId: proof.decision.id,
    fileEventId: proof.file?.id ?? null, exportEventId: proof.exported?.id ?? null,
    fileSha256: proof.file?.data.after.sha256 ?? null,
    exportManifestSha256: proof.exported?.data.manifest_digest ?? null,
    auditExportSha256: createHash("sha256").update(bytes).digest("hex"),
    client: proof.resolution.client, authMode: proof.resolution.auth_mode,
    requestedModel: proof.resolution.model ?? null, runtime: proof.requested.data.runtime,
    workerId,
    ...(expectWorkerProvenance ? { workerProvenance: "displayed_fields_match_registry" } : {}),
  };
}

async function run() {
  let result;
  try {
    result = { status: "PASS", ...await main() };
  } catch (error) {
    const blocked = error instanceof Blocked;
    // Syntax errors can include response bodies. Keep them out of the JSON report.
    const message = blocked || error instanceof EvidenceError || error instanceof assert.AssertionError
      ? error.message
      : error instanceof SyntaxError
        ? "Données JSON invalides."
        : error.name === "TimeoutError"
          ? "Le contrôle de cette étape a expiré."
          : "Erreur de transport ou d’exécution de la recette.";
    result = { status: blocked ? "BLOCKED" : "FAIL", stage, message };
    await addFailureArtifacts(result);
    process.exitCode = blocked ? 2 : 1;
  } finally {
    if (guard?.violations.length) await addFailureArtifacts(result);
    const cleanup = await closeBrowserResources(context, browser);
    if (cleanup.length || guard?.violations.length) {
      result = { ...result, status: "FAIL", stage, message: "Échec de la fermeture ou du garde réseau du navigateur." };
      process.exitCode = 1;
    }
  }
  let report = JSON.stringify({ ...result, checks, node: process.version });
  for (const secret of viewer ? [viewer, encodeURIComponent(viewer), JSON.stringify(viewer).slice(1, -1)] : [])
    report = report.split(secret).join("[REDACTED]");
  process.stdout.write(`${report}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  await run();
