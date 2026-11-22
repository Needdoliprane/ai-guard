import assert from "node:assert/strict";
import { test } from "node:test";
import { integrityCoverage, parseRetentionLedger, retentionNotice } from "../src/retention.ts";

const segment = {
  segment_index: 2,
  archive_id: "0b8d4c1e-4f0a-4b8e-9d21-6f9f2b6b1a10",
  manifest_sha256: "a".repeat(64),
  first_seq: 120,
  last_seq: 240,
  reclaimed_events: 90,
  reclaimed_bytes: 51200,
  retention_event_seq: 251,
};
const ledger = {
  capability: true,
  segments: 2,
  archived_through_seq: 240,
  reclaimed_events: 170,
  reclaimed_bytes: 98304,
  last: segment,
};

test("an archived prefix is named, never shown as an absence of activity", () => {
  const parsed = parseRetentionLedger(ledger);
  assert.ok(parsed);
  const notice = retentionNotice(parsed);
  assert.match(notice, /jusqu’à l’événement n° 240/);
  assert.match(notice, /2 segments, 170 événements retirés/);
  assert.match(notice, /0b8d4c1e…/);
  assert.match(integrityCoverage(252, 82, parsed), /252 positions de chaîne vérifiées.*dont 82 encore dans la base vivante/);
});

test("a store without retention keeps the plain integrity statement", () => {
  const parsed = parseRetentionLedger({ ...ledger, segments: 0, archived_through_seq: 0, reclaimed_events: 0, reclaimed_bytes: 0, last: null });
  assert.ok(parsed);
  assert.equal(retentionNotice(parsed), null);
  assert.equal(integrityCoverage(12, 12, parsed), "12 événements vérifiés par le collecteur.");
});

test("a malformed or inconsistent ledger stays unknown", () => {
  for (const value of [
    null,
    {},
    { ...ledger, segments: -1 },
    { ...ledger, last: null },
    { ...ledger, archived_through_seq: 239 },
    { ...ledger, capability: false },
    { ...ledger, last: { ...segment, archive_id: "not-a-uuid" } },
    { ...ledger, last: { ...segment, retention_event_seq: 240 } },
  ]) {
    assert.equal(parseRetentionLedger(value), null, JSON.stringify(value));
  }
  assert.equal(retentionNotice(null), null);
  assert.equal(integrityCoverage(undefined, undefined, null), "Nombre d’événements vérifiés inconnu.");
  assert.match(integrityCoverage(30, null, parseRetentionLedger(ledger)), /dont un nombre inconnu/);
});
