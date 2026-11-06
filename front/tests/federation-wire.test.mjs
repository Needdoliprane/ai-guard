// Opt-in real gateway/Store wire handoff from federation_workers_api in the same QA.
import assert from "node:assert/strict";
import { test } from "node:test";
import { lstatSync, readFileSync } from "node:fs";
import { join, isAbsolute } from "node:path";
import { parseFederationWorkerPage } from "../src/control.ts";

const directory = process.env.AIGUARD_FEDERATION_WIRE_DIR;
test("the frontend consumes actual modern A/B and legacy Rust HTTP responses", { skip: !directory }, () => {
  assert.ok(isAbsolute(directory));
  const parent = lstatSync(directory);
  assert.ok(parent.isDirectory() && !parent.isSymbolicLink());
  assert.equal(parent.mode & 0o777, 0o700);
  const read = (name) => {
    const path = join(directory, name);
    const metadata = lstatSync(path);
    assert.ok(metadata.isFile() && !metadata.isSymbolicLink());
    assert.equal(metadata.mode & 0o777, 0o600);
    assert.ok(metadata.size > 0 && metadata.size <= 512 * 1024);
    return parseFederationWorkerPage(JSON.parse(readFileSync(path, "utf8")));
  };
  const a = read("modern-a.json");
  const b = read("modern-b.json");
  const legacy = read("legacy.json");
  assert.equal(a.registry_status, "available");
  assert.equal(b.registry_status, "available");
  assert.equal(legacy.registry_status, "migration_required");
  assert.equal(a.items.length, 2);
  assert.equal(b.items.length, 1);
  assert.equal(legacy.items.length, 1);
  assert.ok(a.items.every((item) => !b.items.some((other) => other.worker_id === item.worker_id)));
  const registered = (page) => page.items.find((item) => item.federation.launch_broker.status === "registered");
  assert.ok(registered(a));
  assert.ok(registered(b));
  assert.notEqual(registered(a).federation.launch_broker.broker.current_incarnation_id,
    registered(b).federation.launch_broker.broker.current_incarnation_id);
  assert.equal(legacy.items[0].federation.launch_broker.status, "registry_unavailable");
  assert.equal(legacy.items[0].federation.observation_broker.status, "registry_unavailable");
});
