import assert from "node:assert/strict";
import { test } from "node:test";
import * as control from "../src/control.ts";
import { ApiClient } from "../src/api.ts";

const id = "00000000-0000-4000-8000-000000000001";
const worker = {
  worker_id: id, state: "in_doubt", quota_held: true, revision: 2,
  launch: { version: 1, session_id: id, runtime_kind: "docker", authority_id: "11111111-1111-4111-8111-111111111111",
    provider: "generic", profile: "strict", cpu_millis: 2000, memory_bytes: 1073741824,
    pids_limit: 256, timeout_seconds: 60, network: "none" },
  attempt: { attempt_id: id, stable_name: "known", volume_name: "known", container_id: null },
  last_observation: null, observed_started_at: null, allocation_inconsistent: false,
  unknown_reason: null, cleanup_removed: null, cleanup_error: null,
};
const unknown = () => ({ status: "authority_unknown", broker: null, runtime_kind: null,
  runtime_matches: null, in_current_grant: null });
const registered = () => ({ status: "registered", runtime_kind: "docker", runtime_matches: true,
  in_current_grant: false, broker: { broker_id: id, current_incarnation_id: id, state: "stale",
    grant_revision: "9007199254740993", heartbeat_revision: "1",
    last_received_at: "2026-11-06T01:00:00.000Z", lease_expires_at: "2026-11-06T01:02:00.000Z" } });
const page = () => ({ version: 1, source: "tenant_worker_broker_registry", consistency: "snapshot_per_page",
  registry_status: "available",
  evaluated_at: "2026-11-06T01:03:00.000Z", runtime_inventory: "unknown", next_cursor: null,
  items: [{ worker, launch_broker: registered(), observation_broker: unknown() }] });
const parse = (value) => {
  assert.equal(typeof control.parseFederationWorkerPage, "function", "federation has a separate strict read parser");
  return control.parseFederationWorkerPage(value);
};

test("federation keeps historical ownership and server freshness without inventing a live host", () => {
  const parsed = parse(page());
  assert.equal(parsed.evaluated_at, "2026-11-06T01:03:00.000Z");
  assert.equal(parsed.items[0].worker_id, id);
  assert.equal(parsed.items[0].federation.launch_broker.broker.state, "stale");
  assert.equal(parsed.items[0].federation.launch_broker.broker.grant_revision, "9007199254740993");
  assert.equal(parsed.items[0].federation.launch_broker.in_current_grant, false);
  assert.equal(parsed.items[0].federation.observation_broker.status, "authority_unknown");
  const value = page();
  value.items[0].worker.launch.command = { argv: ["private"] };
  value.items[0].launch_broker.broker.credential_id = "private";
  const limited = parse(value);
  assert.equal("command" in limited.items[0].launch, false);
  assert.equal("credential_id" in limited.items[0].federation.launch_broker.broker, false);
});

test("federation refuses missing, contradictory, or unbounded association metadata", () => {
  for (const mutate of [
    (v) => { v.version = 2; }, (v) => { v.runtime_inventory = "complete"; },
    (v) => { delete v.evaluated_at; }, (v) => { v.items[0].launch_broker = null; },
    (v) => { v.items[0].launch_broker.broker = null; },
    (v) => { v.items[0].launch_broker.broker.state = "healthy"; },
    (v) => { v.items[0].launch_broker.runtime_matches = false; },
    (v) => { v.items[0].launch_broker.broker.grant_revision = 1; },
    (v) => { v.items[0].launch_broker.broker.heartbeat_revision = "9223372036854775808"; },
    (v) => { v.items[0].observation_broker = registered(); },
    (v) => { v.next_cursor = "../tenant-b"; },
  ]) {
    const value = page(); mutate(value);
    assert.throws(() => parse(value), /contrat/i);
  }
  const value = page();
  value.items[0].launch_broker = { ...unknown(), status: "unregistered" };
  assert.equal(parse(value).items[0].federation.launch_broker.status, "unregistered");
  const items = (length) => Array.from({ length }, (_, index) => {
    const item = structuredClone(value.items[0]);
    item.worker.worker_id = `00000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`;
    return item;
  });
  // Distinct sorted IDs keep order/uniqueness valid on both sides of the cap.
  assert.equal(parse({ ...value, items: items(100) }).items.length, 100);
  assert.throws(() => parse({ ...value, items: items(101) }), /contrat/i);
});

test("the technical dashboard permits only the bounded federation read route", async () => {
  const seen = [];
  const client = new ApiClient("viewer", async (path, options) => {
    seen.push({ path, method: options.method }); return Response.json(page());
  });
  await client.get(`/api/v1/federation/workers?limit=50&cursor=${id}`);
  assert.deepEqual(seen, [{ path: `/api/v1/federation/workers?limit=50&cursor=${id}`, method: "GET" }]);
  for (const path of ["/api/v1/federation/workers?tenant_id=other", "/api/v1/federation/workers?limit=101",
    "/api/v1/federation/workers?cursor=../other", "/api/v1/federation/workers?limit=1&limit=2",
    "/api/v1/federation/workers/grants", "/api/federation/workers"]) {
    await assert.rejects(client.get(path), /lecture/i);
  }
  assert.equal(seen.length, 1);
  client.close();
});

test("legacy workers retain an explicit structural registry limit without hiding a failed read", () => {
  const value = page();
  value.registry_status = "migration_required";
  value.items[0].launch_broker = { ...unknown(), status: "registry_unavailable" };
  value.items[0].observation_broker = { ...unknown(), status: "registry_unavailable" };
  const parsed = parse(value);
  assert.equal(parsed.items[0].worker_id, id);
  assert.equal(parsed.registry_status, "migration_required");
  assert.equal(parsed.items[0].federation.launch_broker.status, "registry_unavailable");
  for (const mutate of [
    (v) => { delete v.registry_status; },
    (v) => { v.registry_status = "unavailable"; },
    (v) => { v.registry_status = "available"; },
    (v) => { v.items[0].launch_broker = registered(); },
  ]) {
    const bad = { ...value, items: [{ ...value.items[0] }] }; mutate(bad);
    assert.throws(() => parse(bad), /contrat/i);
  }
});
