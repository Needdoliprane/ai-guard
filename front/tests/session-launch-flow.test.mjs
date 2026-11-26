// Life of one prepare→launch journey: stable idempotency, distinct refusals, unknown-outcome replay. Pure.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiError } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { SessionLaunchJourney, launchContextLost, launchFailure } from "../src/session-launch-flow.ts";
import { uuid, secondUuid } from "./human-fixtures.mjs";

const hash = "a".repeat(64);
const future = new Date(Date.now() + 600000).toISOString();
const prepareReceipt = (operationId) => ({ version: 1, operation_id: operationId, preparation_id: "prep-1",
  launch_target_id: secondUuid, target_revision: 7, input_hash: hash, expires_at: future, audit_seq: 12, audit_hash: "b".repeat(64), replayed: false });
const launchReceipt = (operationId) => ({ version: 1, operation_id: operationId, launch_id: "launch-1", session_id: "session-1",
  launch_target_id: secondUuid, target_revision: 7, preparation_operation_id: uuid, preparation_id: "prep-1", input_hash: hash,
  grant_revision: 3, grant_audit_seq: 13, grant_audit_hash: "b".repeat(64), audit_seq: 14, audit_hash: "b".repeat(64),
  conversation: { version: 1, tenant_id: "tenant-a", source_id: "source-1", session_id: "session-1", launch_target_id: secondUuid,
    target_revision: 7, broker_id: null, broker_incarnation: null, worker_id: null, attempt_id: null, runtime_instance: null,
    input_epoch: 0, provider: null, protocol: null, expires_at: future, status: "prepared" }, replayed: false });
const counter = () => { let n = 0; return () => `00000000-0000-4000-8000-${(++n).toString(16).padStart(12, "0")}`; };
const apiError = (status, code = "permission_denied") => new HumanApiError(status, code, `refus ${status}`, "retry");

test("la classification distingue conflit, refus, introuvable, invalide et inconnu", () => {
  assert.equal(launchFailure(apiError(409, "conflict")), "conflict");
  assert.equal(launchFailure(apiError(403)), "refused");
  assert.equal(launchFailure(apiError(401, "authentication_required")), "refused");
  assert.equal(launchFailure(apiError(404, "not_found")), "not_found");
  assert.equal(launchFailure(apiError(400, "invalid_request")), "invalid");
  assert.equal(launchFailure(apiError(503, "identity_unavailable")), "unknown");
  assert.equal(launchFailure(new Error("réseau")), "unknown");
  assert.equal(launchFailure(new HumanContractError()), "invalid");
  assert.equal(launchContextLost(apiError(403, "context_revoked")), true);
  assert.equal(launchContextLost(apiError(403, "permission_denied")), false);
});

test("préparer retient un reçu durable et ne consomme aucune réservation", async () => {
  const journey = new SessionLaunchJourney(counter());
  const seen = [];
  const phase = await journey.prepare("intent-a", async (id) => { seen.push(id); return prepareReceipt(id); });
  assert.equal(phase, "prepared"); assert.equal(journey.preparation.operation_id, seen[0]);
  assert.equal(journey.receipt, null, "la préparation ne lance rien");
  assert.equal(journey.prepareOperationId, null, "une réponse définitive libère l’identifiant");
});

test("une même intention garde le même identifiant ; un résultat inconnu le conserve pour le rejeu", async () => {
  const journey = new SessionLaunchJourney(counter());
  const first = journey.operationIdFor("intent-a");
  assert.equal(journey.operationIdFor("intent-a"), first, "la même intention reprend le même identifiant");
  assert.notEqual(journey.operationIdFor("intent-b"), first, "une intention différente change d’identifiant");
  // A 503 leaves the outcome unknown and keeps the identifier so a retry replays the same operation.
  const sent = [];
  await journey.prepare("intent-a", async (id) => { sent.push(id); throw apiError(503, "identity_unavailable"); });
  assert.equal(journey.phase, "unknown");
  const replay = journey.operationIdFor("intent-a");
  assert.equal(replay, sent[0], "le rejeu après inconnu reprend exactement l’opération envoyée");
  // A definitive refusal releases the identifier; a fresh intent gets a new one.
  await journey.prepare("intent-a", async () => { throw apiError(403); });
  assert.equal(journey.phase, "refused"); assert.equal(journey.prepareOperationId, null);
});

test("le lancement exige une préparation retenue et commit l’intention exacte", async () => {
  const journey = new SessionLaunchJourney(counter());
  // Without a preparation, launch sends nothing.
  let called = false;
  await journey.launch("k", async () => { called = true; return launchReceipt(uuid); });
  assert.equal(called, false); assert.equal(journey.phase, "editing");
  await journey.prepare("intent-a", async (id) => prepareReceipt(id));
  const sent = [];
  const phase = await journey.launch("prep-key", async (id) => { sent.push(id); return launchReceipt(id); });
  assert.equal(phase, "launched"); assert.equal(journey.receipt.operation_id, sent[0]);
  assert.equal(journey.preparation.operation_id, uuid, "la préparation consommée reste nommée");
});

test("un refus au rejeu laisse le premier lancement inconnu et conserve son identifiant", async () => {
  const journey = new SessionLaunchJourney(counter());
  await journey.prepare("intent-a", async (id) => prepareReceipt(id));
  const sent = [];
  await journey.launch("prep-key", async (id) => { sent.push(id); throw apiError(503, "identity_unavailable"); });
  assert.equal(journey.phase, "unknown"); assert.equal(journey.launchOperationId, sent[0]);
  await journey.launch("prep-key", async (id) => { assert.equal(id, sent[0], "rejeu de la même opération"); throw apiError(403); });
  assert.equal(journey.phase, "unknown"); assert.equal(journey.launchOperationId, sent[0]);
  assert.equal(journey.error.code, "permission_denied");
  await journey.launch("different-key", async () => assert.fail("aucun lancement divergent"));
  journey.reset();
  assert.equal(journey.phase, "unknown");
  await journey.launch("prep-key", async (id) => { assert.equal(id, sent[0]); return launchReceipt(id); });
  assert.equal(journey.phase, "launched");
});

test("recommencer retire la préparation et le lancement sans toucher un envoi en vol", async () => {
  const journey = new SessionLaunchJourney(counter());
  await journey.prepare("intent-a", async (id) => prepareReceipt(id));
  await journey.launch("prep-key", async (id) => launchReceipt(id));
  assert.equal(journey.phase, "launched");
  journey.reset();
  assert.equal(journey.phase, "editing"); assert.equal(journey.preparation, null); assert.equal(journey.receipt, null);
});
