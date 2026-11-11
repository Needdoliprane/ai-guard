// Opt-in: exact responses of the real Rust gateway, captured by tests/supervision_stack_api.rs in the same
// private QA run, decoded by the product parsers and model. Without the directory these tests are skipped
// (NOT_RUN, never PASS). Nothing here is imported by the application.
import assert from "node:assert/strict";
import { test } from "node:test";
import { lstatSync, readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { parseHumanEventPage, parseHumanFederationWorkerPage, parseResourcePage } from "../src/human-model.ts";
import { defaultFleetFilter, filterWorkers, joinFleet, sessionResource, sortWorkers, workerActivity, workerFlags } from "../src/supervision-model.ts";

const directory = process.env.AIGUARD_SUPERVISION_WIRE_DIR;
const read = (name) => {
  assert.ok(isAbsolute(directory), "the wire directory is the absolute QA-owned path");
  const parent = lstatSync(directory);
  assert.ok(parent.isDirectory() && !parent.isSymbolicLink()); assert.equal(parent.mode & 0o777, 0o700);
  const path = join(directory, name); const metadata = lstatSync(path);
  assert.ok(metadata.isFile() && !metadata.isSymbolicLink(), `${name} was captured by its Rust parent`);
  assert.equal(metadata.mode & 0o777, 0o600); assert.ok(metadata.size > 0 && metadata.size <= 2 * 256 * 1024 + 4096);
  return JSON.parse(readFileSync(path, "utf8"));
};

test("the real composed pages decode, join and filter without loss or invention", { skip: !directory }, () => {
  const first = parseHumanFederationWorkerPage(read("federation-first.json"));
  const next = parseHumanFederationWorkerPage(read("federation-next.json"));
  assert.ok(first.workers.items.length >= 1 && first.workers.items.length <= 100);
  assert.equal(first.workers.has_more, true); assert.equal(next.workers.has_more, false);
  assert.equal(first.brokers.items.length, 3);
  const workers = [...first.workers.items, ...next.workers.items];
  assert.equal(new Set(workers.map((item) => item.worker_id)).size, workers.length, "no row repeats across the two real pages");
  assert.equal(workers.length, 120);
  const groups = joinFleet(first.brokers.items, workers);
  assert.deepEqual(groups.filter((group) => group.kind === "broker").map((group) => group.key), first.brokers.items.map((item) => item.broker_id));
  assert.ok(groups.some((group) => group.kind === "unregistered" && group.workers.length > 0), "the unregistered authority keeps its workers");
  assert.equal(groups.some((group) => group.kind === "unlisted_broker"), false, "every referenced broker is on the real broker page");
  assert.equal(new Set(groups.flatMap((group) => group.workers.map((item) => item.worker_id))).size, workers.length, "the join loses no real row");
  const states = new Set(workers.map((item) => item.state));
  for (const state of ["reserved", "running", "terminal", "in_doubt", "cancelled"]) assert.ok(states.has(state));
  const counts = { open: 0, uncertain: 0, ended: 0 }; for (const item of workers) counts[workerActivity(item.state)]++;
  assert.equal(counts.open + counts.uncertain + counts.ended, workers.length);
  assert.ok(counts.open > 0 && counts.uncertain > 0 && counts.ended > 0);
  assert.equal(filterWorkers(workers, { ...defaultFleetFilter(), activity: "uncertain" }).length, counts.uncertain);
  assert.ok(workers.some((item) => workerFlags(item).includes("oom_killed")), "a real observed OOM is reported as such");
  assert.ok(workers.some((item) => workerFlags(item).includes("runtime_mismatch")), "a real runtime mismatch is reported as such");
  const sorted = sortWorkers(workers, "activity");
  assert.equal(sorted.length, workers.length); assert.equal(workerActivity(sorted[0].state), "open"); assert.equal(workerActivity(sorted.at(-1).state), "ended");
  const observed = sorted.filter((item) => workerActivity(item.state) === "open" && item.last_observation?.observed_at_status === "known").map((item) => item.last_observation.observed_at);
  assert.deepEqual(observed, [...observed].sort().reverse(), "canonical real dates sort newest first as text");
});

test("a real empty registry decodes as a successful empty read with unknown runtime inventory", { skip: !directory }, () => {
  const empty = parseHumanFederationWorkerPage(read("federation-empty.json"));
  assert.deepEqual([empty.workers.items.length, empty.brokers.items.length], [0, 0]);
  assert.equal(empty.workers.has_more, false); assert.equal(empty.runtime_inventory, "unknown");
  assert.deepEqual(joinFleet(empty.brokers.items, empty.workers.items), []);
});

test("real resources and session events decode and link by exact session identifier", { skip: !directory }, () => {
  const resources = parseResourcePage(read("resources-admin.json"));
  const events = parseHumanEventPage(read("events-session.json"));
  assert.ok(resources.items.length >= 10); assert.ok(events.items.length > 0);
  for (const kind of ["session", "agent", "sandbox", "project"]) assert.ok(resources.items.some((item) => item.resource.kind === kind));
  const session = events.items[0].metadata.resource;
  assert.equal(session.kind, "session");
  assert.ok(events.items.every((item) => item.metadata.resource.resource_id === session.resource_id && item.content === null));
  assert.ok(sessionResource(resources.items, session.resource_id), "the real event resource is one of the real granted sessions");
  // The fleet pages and this resource page come from one data set; the two files above from another.
  const fleetResources = parseResourcePage(read("resources-fleet.json"));
  const workers = [...parseHumanFederationWorkerPage(read("federation-first.json")).workers.items, ...parseHumanFederationWorkerPage(read("federation-next.json")).workers.items];
  const linked = workers.filter((item) => sessionResource(fleetResources.items, item.session_id));
  assert.equal(linked.length, fleetResources.items.filter((item) => item.resource.kind === "session").length, "each real granted session is declared by exactly one real worker");
  assert.equal(workers.filter((item) => sessionResource(resources.items, item.session_id)).length, 0, "identifiers of another data set never link");
});
