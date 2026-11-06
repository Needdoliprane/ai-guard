// Optional RAW response handoff from human_workers_api, inside the same private QA.
// This file is never imported by the frontend and does not manufacture wire DTOs.
import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { parseHumanWorkerPage } from "../src/human-model.ts";

const directory = process.env.AIGUARD_HUMAN_WORKERS_WIRE_DIR;
async function actualResponse(name) {
  assert.ok(isAbsolute(directory), "the wire directory is the absolute QA-owned path");
  let bytes;
  try { bytes = await readFile(join(directory, name)); }
  catch (error) { assert.fail(`RAW human response ${name} was not captured by its Rust API parent: ${error.code}`); }
  assert.ok(bytes.byteLength <= 256 * 1024, "actual human frame fits its exact wire budget");
  const raw = bytes.toString("utf8");
  const body = JSON.parse(raw);
  assert.equal(body.error, undefined, `actual human ${name} frame is a server refusal: ${body.error?.code ?? "none"}`);
  assert.equal(body.source, "tenant_worker_broker_registry");
  assert.ok(Array.isArray(body.items) && body.items.length === 1);
  return { raw, body };
}

test("RAW nil-session page preserves the canonical legacy UUID through the product parser", { skip: !directory }, async () => {
  const { body } = await actualResponse("nil-session.json");
  assert.equal(body.items[0].session_id, "00000000-0000-0000-0000-000000000000");
  const parsed = parseHumanWorkerPage(body);
  assert.equal(parsed.items[0].session_id, "00000000-0000-0000-0000-000000000000");
  assert.equal(parsed.items[0].session_provenance, "declared");
});

test("RAW InDoubt page preserves typed date uncertainty through the product parser", { skip: !directory }, async () => {
  const { raw, body } = await actualResponse("invalid-runtime-date.json");
  const item = body.items[0];
  assert.equal(item.state, "in_doubt");
  assert.equal(item.quota_held, true);
  assert.equal(item.last_observation.started_at, null);
  assert.equal(item.last_observation.started_at_status, "unknown");
  assert.equal(item.last_observation.finished_at_status, "known");
  assert.equal(raw.includes("unavailable"), false, "raw arbitrary runtime date text does not enter the human DTO");
  const parsed = parseHumanWorkerPage(body);
  assert.equal(parsed.items[0].last_observation.started_at_status, "unknown");
  assert.equal(parsed.items[0].last_observation.finished_at_status, "known");
});

test("RAW legacy i64 exit code reaches the product parser as the exact decimal string", { skip: !directory }, async () => {
  const { raw, body } = await actualResponse("large-exit-code.json");
  assert.ok(raw.includes("9007199254740993"), "the RAW Rust response must contain the exact accepted i64 counterexample");
  const received = body.items[0].last_observation.exit_code;
  // This assertion deliberately precedes parser/status-field assertions. It
  // exposes the current number transport/JS rounding, not a different new field.
  assert.equal(typeof received, "string", "an accepted i64 above Number's safe range requires decimal-string transport");
  assert.equal(received, "9007199254740993");
  assert.equal(BigInt(received), 9_007_199_254_740_993n);
  const parsed = parseHumanWorkerPage(body);
  assert.equal(parsed.items[0].last_observation.exit_code, "9007199254740993");
  assert.equal(BigInt(parsed.items[0].last_observation.exit_code), 9_007_199_254_740_993n);
});
