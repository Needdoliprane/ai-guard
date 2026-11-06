import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHumanIdentity, parseHumanWorkerPage, humanWorkersUrl, HumanContractError } from "../src/human-model.ts";
import { HumanApiClient, HumanSuperseded } from "../src/human-api.ts";
import { worker, workerPage, workerIdentity, workerObservation, errorResponse } from "./human-fixtures.mjs";

test("Workers conserve sa capacité explicite et sa projection fermée sans contenu technique", () => {
  assert.deepEqual(parseHumanIdentity(workerIdentity()).contexts[0].capabilities, ["read_worker_registry"]);
  assert.equal(parseHumanWorkerPage(workerPage()).items[0].revision, "9223372036854775807");
  assert.equal(parseHumanWorkerPage(workerPage()).items[0].session_provenance, "declared");
  for (const extra of [{ launch: {} }, { credential_binding_id: "private" }, { argv: ["private"] }, { owner: "inferred-human" }, { session_provenance: "observed" }])
    assert.throws(() => parseHumanWorkerPage(workerPage([worker(extra)])), HumanContractError);
  for (const extra of [{ items: null }, { runtime_inventory: "complete" }, { consistency: "snapshot" }, { has_more: true }, { total: 100 }])
    assert.throws(() => parseHumanWorkerPage(workerPage(undefined, extra)), HumanContractError);
});

test("Workers valide la provenance des associations et des observations", () => {
  const association = { status: "registered", runtime_kind: "docker", runtime_matches: true, in_current_grant: false,
    broker: { broker_id: "00000000-0000-4000-8000-000000000001", current_incarnation_id: "00000000-0000-4000-8000-000000000002", state: "stale",
      grant_revision: "1", heartbeat_revision: "0", last_received_at: null, lease_expires_at: null } };
  assert.equal(parseHumanWorkerPage(workerPage([worker({ launch_broker: association })])).items[0].launch_broker.in_current_grant, false);
  for (const bad of [{ ...association, credential_id: "private" }, { ...association, runtime_matches: null }, { status: "unregistered", broker: association.broker, runtime_kind: null, runtime_matches: null, in_current_grant: null }])
    assert.throws(() => parseHumanWorkerPage(workerPage([worker({ launch_broker: bad })])), HumanContractError);
  assert.throws(() => parseHumanWorkerPage(workerPage([worker({ last_observation: { status: "running" } })])), HumanContractError);
});

test("les UUID Workers historiques acceptent nil sans élargir les identités brokers ou reçus", () => {
  const nil = "00000000-0000-0000-0000-000000000000";
  const item = worker({ worker_id: nil, request_id: nil, attempt_id: nil, session_id: nil });
  assert.equal(parseHumanWorkerPage(workerPage([item])).items[0].worker_id, nil);
  assert.throws(() => parseHumanWorkerPage(workerPage([worker({ worker_id: "00000000-0000-4000-8000-00000000000A" })])), HumanContractError);
  const association = { status: "registered", runtime_kind: "docker", runtime_matches: true, in_current_grant: true,
    broker: { broker_id: nil, current_incarnation_id: "00000000-0000-4000-8000-000000000002", state: "unseen", grant_revision: "1", heartbeat_revision: "0", last_received_at: null, lease_expires_at: null } };
  assert.throws(() => parseHumanWorkerPage(workerPage([worker({ launch_broker: association })])), HumanContractError);
  assert.throws(() => parseHumanWorkerPage(workerPage(undefined, { read_receipt: { request_id: nil, audit_event_id: nil, received_at: "2026-11-01T12:00:00Z" } })), HumanContractError);
});

test("les dates Workers distinguent valeurs connues, absentes et inconnues par champ", () => {
  const observation = workerObservation({ observed_at: "1970-01-01T00:00:00.000000000Z", started_at: null, started_at_status: "unknown", finished_at: "1970-01-01T00:00:00.000000000Z" });
  const parsed = parseHumanWorkerPage(workerPage([worker({ state: "in_doubt", last_observation: observation })])).items[0];
  assert.equal(parsed.last_observation.started_at_status, "unknown");
  assert.equal(parsed.last_observation.finished_at_status, "known");
  assert.equal(parsed.observed_started_at_status, "absent");
  const unrepresentable = parseHumanWorkerPage(workerPage([worker({ last_observation: workerObservation({ observed_at: null, observed_at_status: "unknown" }) })])).items[0];
  assert.equal(unrepresentable.last_observation.observed_at, null);
  assert.equal(unrepresentable.last_observation.observed_at_status, "unknown");
  for (const bad of [{ started_at: null, started_at_status: "known" }, { started_at: "unavailable", started_at_status: "unknown" },
    { started_at: null, started_at_status: "observed" }, { finished_at: "2026-02-30T10:00:00.000000000Z", finished_at_status: "known" },
    { observed_at_status: "unknown" }, { observed_at: null, observed_at_status: "absent" }, { observed_at: "2026-10-29T10:00:02Z" }])
    assert.throws(() => parseHumanWorkerPage(workerPage([worker({ last_observation: workerObservation(bad) })])), HumanContractError);
});

test("les codes de sortie i64 restent des décimaux exacts avec bornes BigInt", () => {
  for (const exit_code of ["0", "-1", "9007199254740993", "9223372036854775807", "-9223372036854775808"]) {
    const received = parseHumanWorkerPage(workerPage([worker({ last_observation: workerObservation({ exit_code }) })])).items[0].last_observation.exit_code;
    assert.equal(received, exit_code); assert.equal(BigInt(received), BigInt(exit_code));
  }
  for (const exit_code of [0, 9007199254740993, "-0", "+1", "01", "9223372036854775808", "-9223372036854775809"])
    assert.throws(() => parseHumanWorkerPage(workerPage([worker({ last_observation: workerObservation({ exit_code }) })])), HumanContractError);
});

test("le client Workers seul emploie GET humain avec contexte sans appel technique ni SSE", async () => {
  const requests = [];
  const client = new HumanApiClient("fixture-private-worker-human", { onAuthentication() {}, onContextInvalidated() {} }, async (path, options) => {
    requests.push({ path, options }); return Response.json(path.endsWith("/identity") ? workerIdentity() : workerPage());
  });
  try {
    await client.identity(); client.selectContext("context-workers"); await client.workers();
    assert.deepEqual(requests.map(({ path }) => path), ["/api/v1/human/identity", "/api/v1/human/workers?limit=25"]);
    assert.equal(requests[1].options.headers["X-AiGuard-Context"], "context-workers");
    assert.equal(requests[1].options.credentials, "omit"); assert.equal(requests[1].options.cache, "no-store");
    const url = new URL(humanWorkersUrl("worker-next-cursor"), "http://localhost");
    assert.deepEqual([...url.searchParams.keys()].sort(), ["cursor", "limit"]);
  } finally { client.close(); }
});

test("une réponse Workers suspendue ne survit pas au retrait du contexte", async () => {
  let release; let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  const client = new HumanApiClient("fixture-private-worker-human", { onAuthentication() {}, onContextInvalidated() {} }, async (path) => {
    if (path.endsWith("/identity")) return Response.json(workerIdentity());
    entered(); await new Promise((resolve) => { release = resolve; }); return Response.json(workerPage());
  });
  try {
    await client.identity(); client.selectContext("context-workers"); const pending = client.workers();
    await started; client.clearContext(); release(); await assert.rejects(pending, HumanSuperseded);
    assert.equal(client.context, null);
  } finally { client.close(); }
});

test("le budget durable Workers arrête les retries et une révocation purge le contexte", async () => {
  let phase = "budget"; let calls = 0; let invalidations = 0;
  const client = new HumanApiClient("fixture-private-worker-human", { onAuthentication() {}, onContextInvalidated() { invalidations++; } }, async (path) => {
    calls++; if (path.endsWith("/identity")) return Response.json(workerIdentity());
    return phase === "budget" ? errorResponse(429, "audit_read_budget_exhausted", "contact_operator") : errorResponse(403, "context_revoked", "select_context");
  });
  try {
    await client.identity(); client.selectContext("context-workers");
    await assert.rejects(client.workers(), (error) => error.code === "audit_read_budget_exhausted");
    const stopped = calls; await assert.rejects(client.workers(), (error) => error.code === "audit_read_budget_exhausted"); assert.equal(calls, stopped);
    phase = "revoked"; client.allowReadAfterOperatorAction();
    await assert.rejects(client.workers(), (error) => error.code === "context_revoked");
    assert.equal(client.context, null); assert.equal(invalidations, 1); assert.equal(client.readBudgetStopped, true);
  } finally { client.close(); }
});
