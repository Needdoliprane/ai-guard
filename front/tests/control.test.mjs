import assert from "node:assert/strict";
import { test } from "node:test";
import {
  parseWorkerPage, parseAllocationPage, parseQuota, formatExactDecimal,
  allocationDuration, relatedWorkers, relatedAgents, relatedSandboxes, workerRelation, controlPageUrl,
} from "../src/control.ts";

const id = "00000000-0000-4000-8000-000000000001";
const worker = {
  worker_id: id, state: "in_doubt", quota_held: true, revision: 2,
  launch: { version: 1, session_id: id, runtime_kind: "docker", provider: "generic",
    profile: "strict", cpu_millis: 2000, memory_bytes: 1073741824,
    pids_limit: 256, timeout_seconds: 60, network: "none" },
  attempt: { attempt_id: id, stable_name: "worker-known", volume_name: "volume-known", container_id: null },
  last_observation: null, unknown_reason: "Inspection indisponible",
  observed_started_at: null, allocation_inconsistent: false,
};

test("une page worker incomplète n’est pas transformée en liste vide", () => {
  assert.equal(parseWorkerPage({ items: [worker], next_cursor: id }).items[0].state, "in_doubt");
  for (const invalid of [null, {}, { items: null }, { items: [null], next_cursor: null },
    { items: [{ ...worker, launch: null }], next_cursor: null },
    { items: [], next_cursor: "../autre" }])
    assert.throws(() => parseWorkerPage(invalid), /contrat/i);
});

test("le modèle de lecture worker ne conserve pas les commandes ou métadonnées de credentials", () => {
  const parsed = parseWorkerPage({ items: [{ ...worker, receipt: { request_hash: "interne" },
    launch: { ...worker.launch, command: { argv: ["private-argument"] },
      credential_names: ["PRIVATE_KEY"], credential_binding_id: id } }], next_cursor: null }).items[0];
  assert.equal("command" in parsed.launch, false);
  assert.equal("credential_names" in parsed.launch, false);
  assert.equal("receipt" in parsed, false);
  assert.equal(parsed.launch.session_id, id);
});

test("les autorités de lancement et d’inspection sont projetées séparément sans normalisation", () => {
  const utf8Boundary = "é".repeat(128);
  const observed = { ...worker, launch: { ...worker.launch, authority_id: "  authority-a  " },
    last_observation: { observed_at: "2026-11-05T10:00:00Z", status: "running", running: true,
      started_at: "2026-11-05T09:59:00Z", finished_at: null, exit_code: null,
      oom_killed: false, restart_count: 0,
      inspection: { authority_id: "authority-b", runtime_kind: "podman", container_id: "ignored" } } };
  const parsed = parseWorkerPage({ items: [observed], next_cursor: null }).items[0];
  assert.equal(parsed.launch.authority_id, "  authority-a  ");
  assert.deepEqual(parsed.last_observation.inspection, {
    authority_id: "authority-b", runtime_kind: "podman",
  });
  assert.equal("container_id" in parsed.last_observation.inspection, false);

  const unknown = parseWorkerPage({ items: [{ ...worker,
    launch: { ...worker.launch, authority_id: null },
    last_observation: { ...observed.last_observation,
      inspection: { runtime_kind: "docker" } } }], next_cursor: null }).items[0];
  assert.equal(unknown.launch.authority_id, null);
  assert.equal(unknown.last_observation.inspection.authority_id, null);
  const omitted = { ...worker, launch: { ...worker.launch },
    last_observation: { ...observed.last_observation, inspection: { runtime_kind: "docker" } } };
  delete omitted.launch.authority_id;
  const omittedParsed = parseWorkerPage({ items: [omitted], next_cursor: null }).items[0];
  assert.equal(omittedParsed.launch.authority_id, null);
  assert.equal(omittedParsed.last_observation.inspection.authority_id, null);

  const boundary = parseWorkerPage({ items: [{ ...worker,
    launch: { ...worker.launch, authority_id: utf8Boundary },
    last_observation: { ...observed.last_observation,
      inspection: { authority_id: utf8Boundary, runtime_kind: "docker" } } }], next_cursor: null }).items[0];
  assert.equal(boundary.launch.authority_id, utf8Boundary);
  assert.equal(boundary.last_observation.inspection.authority_id, utf8Boundary);

  for (const bad of ["", 1, "a".repeat(257), "é".repeat(129), "bad\u0000authority"]) {
    assert.throws(() => parseWorkerPage({ items: [{ ...worker,
      launch: { ...worker.launch, authority_id: bad } }], next_cursor: null }), /contrat/i);
    assert.throws(() => parseWorkerPage({ items: [{ ...observed,
      last_observation: { ...observed.last_observation,
        inspection: { authority_id: bad, runtime_kind: "docker" } } }], next_cursor: null }), /contrat/i);
  }
  assert.equal(parsed.launch.authority_id, "  authority-a  ", "les valeurs ne sont pas normalisées");
});

test("les unités entières de l’allocation restent exactes au-delà de Number.MAX_SAFE_INTEGER", () => {
  const value = "9007199254740993000000001";
  assert.equal(formatExactDecimal(value, 9), "9\u202f007\u202f199\u202f254\u202f740\u202f993,000000001");
  assert.equal(formatExactDecimal("0", 9), "0");
  assert.equal(formatExactDecimal(null, 9), "Inconnu");
  assert.equal(formatExactDecimal("invalid", 9), "Inconnu");
  assert.equal(allocationDuration(null), "Inconnue · aucune observation terminale");
  const allocation = { attempt_id: id, worker_id: id, terminal_observation_id: id,
    started_at: "2026-10-29T12:00:00Z", finished_at: "2026-10-29T12:00:01Z",
    duration_ns: "1000000000", cpu_millicore_nanoseconds: value,
    memory_byte_nanoseconds: value, method_version: 1, billable: false,
    source: { authority_id: "qa-authority", runtime_kind: "docker", container_id: "runtime-known", spec_hash: "a".repeat(64), provenance: "observed" } };
  assert.equal(allocationDuration(allocation), "1 s");
  assert.equal(parseAllocationPage({ items: [allocation], next_cursor: null }).items[0].cpu_millicore_nanoseconds, value);
  for (const bad of [{ ...allocation, duration_ns: 1000000000 },
    { ...allocation, billable: true }, { ...allocation, method_version: 2 },
    { ...allocation, source: null }, { ...allocation, source: { ...allocation.source, provenance: "declared" } }])
    assert.throws(() => parseAllocationPage({ items: [bad], next_cursor: null }), /contrat/i);
});

test("une allocation runtime incohérente conserve son signal au lieu de déduire une durée", () => {
  const parsed = parseWorkerPage({ items: [{ ...worker, allocation_inconsistent: true,
    observed_started_at: "2026-10-29T12:00:00Z" }], next_cursor: null }).items[0];
  assert.equal(parsed.allocation_inconsistent, true);
  assert.equal(parsed.observed_started_at, "2026-10-29T12:00:00Z");
});

test("un quota absent reste inconnu et un quota nul reste une vraie limite", () => {
  const quota = { policy: { version: 1, max_active_workers: 0, max_cpu_millis: 0,
    max_memory_bytes: 0, max_timeout_seconds: 60 }, held_workers: 0,
    held_cpu_millis: 0, held_memory_bytes: 0 };
  assert.equal(parseQuota(quota).policy.max_active_workers, 0);
  assert.throws(() => parseQuota({ ...quota, held_workers: null }), /contrat/i);
});

test("le lien agent-worker utilise uniquement la session ou l’identité runtime exacte", () => {
  assert.deepEqual(relatedWorkers({ id: "agent", session_id: id }, [worker]).map((item) => item.worker_id), [id]);
  assert.deepEqual(relatedWorkers({ id: "agent", sandbox_id: "worker-known" }, [worker]), [worker]);
  assert.deepEqual(relatedWorkers({ id: "agent", name: "worker-known", provider: "generic" }, [worker]), []);
  assert.deepEqual(relatedWorkers({ id: "agent", session_id: "autre" }, [worker]), []);
});

test("les liens worker-sandbox-agent distinguent chaque identifiant explicite des noms ressemblants", () => {
  const observed = { ...worker, attempt: { ...worker.attempt, container_id: "runtime-container" } };
  const sandbox = { id: "runtime-container", engine: "docker" };
  const sessionSandbox = { id: "other-container", session_id: id };
  const runtimeAgent = { id: "agent", sandbox_id: "runtime-container", provenance: "declared" };
  const sessionAgent = { id: "session-agent", session_id: id };
  assert.deepEqual(relatedSandboxes(observed, [sandbox, sessionSandbox,
    { id: "runtime", image: "runtime-container" }, { id: "agent", engine: "generic" }]), [sandbox, sessionSandbox]);
  assert.deepEqual(relatedAgents(observed, [runtimeAgent, sessionAgent,
    { id: "runtime-container", name: "worker-known", provider: "generic" }]), [runtimeAgent, sessionAgent]);
  assert.deepEqual(workerRelation({ session_id: id, sandbox_id: "runtime-container" }, observed),
    ["Session identique", "Identité runtime identique"]);
  assert.deepEqual(workerRelation({ sandbox_id: "runtime" }, observed), []);
  assert.deepEqual(workerRelation({ session_id: "", sandbox_id: "" }, observed), []);
  assert.equal(runtimeAgent.provenance, "declared", "Une jointure ne requalifie pas un agent déclaré en observation");
});

test("la pagination ne transporte qu’un curseur UUID et une taille bornée", () => {
  const url = new URL(controlPageUrl("workers", id), "http://localhost");
  assert.equal(url.pathname, "/api/v1/federation/workers");
  assert.equal(url.searchParams.get("cursor"), id);
  assert.equal(url.searchParams.get("limit"), "50");
  assert.throws(() => controlPageUrl("workers", "../autre"), /curseur/i);
});
