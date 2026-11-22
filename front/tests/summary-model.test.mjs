import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHumanSummary, humanSummaryUrl, countOf } from "../src/summary-model.ts";

const context = "c".repeat(64);
const version = "a".repeat(64);
const expected = { contextId: context, authorizationVersion: version };
const broker = "00000000-0000-4000-8000-0000000000b1";
const worker = "00000000-0000-4000-8000-0000000000a1";
const summary = () => ({
  version: 1, context_id: context, authorization_version: version, mode: "administration",
  evaluated_at: "2026-11-21T12:00:00.000000000Z",
  period: { window: "24h", from: "2026-11-20T12:00:00.000000000Z", to: "2026-11-21T12:00:00.000000000Z" },
  consistency: "transaction_snapshot",
  sections: {
    resources: { status: "ready", coverage: "authorized_selection", data: { by_kind: { session: "4", agent: "0", sandbox: "1", project: "0" } } },
    decisions: { status: "ready", coverage: "verified_journal", data: { by_outcome: { allow: "12", deny: "3", review: "1" }, complete: true, oldest_counted_at: null } },
    workers: { status: "ready", coverage: "complete_registry", data: {
      by_state: { running: "2", in_doubt: "1" }, quota_held: "3", groups_complete: true, runtime_inventory: "unknown",
      groups: [
        { association: "registered", runtime_kind: "docker", broker: { broker_id: broker, state: "stale",
          last_received_at: "2026-11-21T11:00:00Z", lease_expires_at: "2026-11-21T11:05:00Z" }, by_state: { running: "2" }, quota_held: "2" },
        { association: "unregistered", runtime_kind: "podman", broker: null, by_state: { in_doubt: "1" }, quota_held: "1" },
      ] } },
    brokers: { status: "ready", coverage: "complete_registry", data: { by_state: { unseen: "0", current: "1", stale: "1", expired: "0", revoked: "1", clock_uncertain: "0" } } },
    quota: { status: "ready", coverage: "complete_registry", data: { max_active_workers: "100", max_cpu_millis: "200000",
      max_memory_bytes: "107374182400", held_workers: "3", held_cpu_millis: "3000", held_memory_bytes: "3221225472" } },
    usage: { status: "ready", coverage: "complete_registry", data: { allocations: "2", duration_ns: "340282366920938463463374607431768211455",
      cpu_millicore_nanoseconds: "0", memory_byte_nanoseconds: "18446744073709551616", method_version: 1, billable: false } },
    attention: { status: "ready", coverage: "complete_registry", data: { by_kind: { worker_in_doubt: "1", broker_lease_stale: "1" },
      items: [{ kind: "broker_lease_stale", subject_kind: "broker", subject_id: broker, at: "2026-11-21T11:00:00Z" },
        { kind: "worker_in_doubt", subject_kind: "worker", subject_id: worker, at: null }], has_more: false } },
  },
  read_receipt: { request_id: "00000000-0000-4000-8000-000000000001", audit_event_id: "00000000-0000-4000-8000-000000000002", received_at: "2026-11-21T12:00:00Z" },
});
const mutate = (change) => { const value = summary(); change(value); return value; };

test("la synthèse conserve les comptes du contrôle en chaînes décimales exactes", () => {
  const parsed = parseHumanSummary(summary(), expected);
  assert.equal(parsed.sections.usage.data.duration_ns, "340282366920938463463374607431768211455");
  assert.equal(parsed.sections.usage.data.memory_byte_nanoseconds, "18446744073709551616");
  assert.equal(parsed.sections.workers.data.groups[0].broker.state, "stale");
  assert.equal(parsed.sections.workers.data.groups[1].broker, null);
  assert.equal(countOf(parsed.sections.workers.data.by_state, "terminal"), null, "an absent state is not shown as zero");
  assert.equal(countOf(parsed.sections.brokers.data.by_state, "expired"), "0", "an exact map may hold zero");
});

test("une section non accordée ou indisponible garde sa nature et sa raison", () => {
  const parsed = parseHumanSummary(mutate((value) => {
    value.sections.workers = { status: "not_granted" };
    value.sections.usage = { status: "unavailable", code: "capacity_exceeded" };
  }), expected);
  assert.deepEqual(parsed.sections.workers, { status: "not_granted" });
  assert.deepEqual(parsed.sections.usage, { status: "unavailable", code: "capacity_exceeded" });
});

test("un décompte de décisions borné nomme la plus ancienne réception comptée", () => {
  const parsed = parseHumanSummary(mutate((value) => {
    value.sections.decisions.data = { by_outcome: { allow: "20000", deny: "0", review: "0" }, complete: false, oldest_counted_at: "2026-11-21T08:00:00Z" };
  }), expected);
  assert.equal(parsed.sections.decisions.data.complete, false);
  assert.equal(parsed.sections.decisions.data.oldest_counted_at, "2026-11-21T08:00:00Z");
});

test("une synthèse incompatible est refusée au lieu de devenir une activité nulle", () => {
  for (const change of [
    (value) => { value.context_id = "d".repeat(64); },
    (value) => { value.extra = true; },
    (value) => { value.sections.workers.data.quota_held = 3; },
    (value) => { value.sections.workers.data.quota_held = "03"; },
    (value) => { value.sections.usage.data.billable = true; },
    (value) => { value.sections.usage.data.duration_ns = "1.5"; },
    (value) => { value.sections.resources.data.by_kind = { session: "1" }; },
    (value) => { value.sections.workers.data.by_state = { exploded: "1" }; },
    (value) => { value.sections.workers.data.runtime_inventory = "complete"; },
    (value) => { value.sections.workers.data.groups[0].broker = null; },
    (value) => { value.sections.decisions.data.complete = true; value.sections.decisions.data.oldest_counted_at = "2026-11-21T08:00:00Z"; },
    (value) => { value.sections.attention.data.items[0].kind = "worker_in_doubt"; },
    (value) => { value.sections.brokers = { status: "ready", coverage: "complete_registry" }; },
    (value) => { value.sections.quota = { status: "unavailable", code: "later" }; },
    (value) => { value.period = { window: "all", from: "2026-11-20T12:00:00Z", to: value.evaluated_at }; },
    (value) => { value.period.to = "2026-11-21T12:00:01Z"; },
    (value) => { delete value.sections.attention; },
  ]) assert.throws(() => parseHumanSummary(mutate(change), expected), /Contrat/);
});

test("la période choisie est la seule entrée de la requête", () => {
  assert.equal(humanSummaryUrl("7d"), "/api/v1/human/summary?window=7d");
});

test("les unités affichées restent exactes au-delà de la précision flottante", async () => {
  const { grouped, gibibytes, perSecond } = await import("../src/summary-model.ts");
  assert.equal(grouped("340282366920938463463374607431768211455"), "340 282 366 920 938 463 463 374 607 431 768 211 455");
  assert.equal(grouped("999"), "999");
  assert.equal(gibibytes("107374182400"), "100,0 Gio");
  assert.equal(gibibytes("1610612735"), "1,4 Gio", "truncated, never rounded up");
  assert.equal(perSecond("18446744073709551616"), "18 446 744 073");
  assert.equal(perSecond("999999999"), "0");
});
