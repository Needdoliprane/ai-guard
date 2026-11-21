import assert from "node:assert/strict";
import { test } from "node:test";
import { brokerProviderProof, validateBrokerProof } from "./browser_provider_live.mjs";

import { brokerBatchFixture as fixture, digest, fixtureProof, id } from "./browser_broker_provider_fixture.mjs";

test("la preuve privée v1 du batch refuse les champs inconnus et les séquences impossibles", () => {
  assert.equal(validateBrokerProof(fixtureProof), fixtureProof);
  assert.throws(() => validateBrokerProof({ ...fixtureProof, secret: "fixture" }), /Contrat/);
  assert.throws(() => validateBrokerProof({ ...fixtureProof, cleanup_audit_seq: 6 }), /Séquences/);
  assert.throws(() => validateBrokerProof({ ...fixtureProof, file_name: "../result.txt" }), /fichier/);
  assert.throws(() => validateBrokerProof({ ...fixtureProof, export_request_id: id(42) }), /Identités/);
});

test("un résultat synthétique lie le reçu allow, le worker, le scan et le reçu d’export à leurs sources", () => {
  const { events, cleanup, worker, expected } = fixture();
  const proof = brokerProviderProof(events, cleanup, worker, expected);
  assert.deepEqual([proof.decision.seq, proof.terminal.seq, proof.cleanup.seq, proof.scan.seq, proof.exported.seq], [1, 3, 4, 5, 7]);
  assert.notEqual(proof.scan.data.manifest_digest, proof.exported.data.manifest_digest);
  assert.ok(!events.some(({ kind }) => ["command.completed", "file.created", "workspace.export.completed"].includes(kind)));
});

test("une décision, un nettoyage, un fichier ou un export divergents refusent la chaîne synthétique", () => {
  for (const [change, message] of [
    [(f) => { f.worker.receipt.audit_hash = digest("2"); }, /autorisation/],
    [(f) => { f.cleanup.data.worker.attempt.attempt_id = id(12); }, /nettoyage/],
    [(f) => { f.events[2].data.changes[0].after.sha256 = digest("3"); }, /scan/],
    [(f) => { f.events[4].data.manifest_digest = f.expected.scan_manifest_sha256; }, /publication/],
    [(f) => { f.worker.last_observation.inspection.container_id = digest("4"); }, /instance/],
    [(f) => { f.worker.admission_operation_id = id(20); }, /instance/],
  ]) {
    const f = fixture();
    change(f);
    assert.throws(() => brokerProviderProof(f.events, f.cleanup, f.worker, f.expected), message);
  }
});
