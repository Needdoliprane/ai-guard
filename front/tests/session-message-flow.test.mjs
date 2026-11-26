// Life of one SendSessionMessage journey: stable idempotency per exact instruction, distinct refusals, unknown-outcome
// replay (never a second Submit), and a status refresh that never resends. Pure.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiError, HumanSuperseded } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { SessionMessageJourney, messageContextLost, messageFailure } from "../src/session-message-flow.ts";
import { uuid, secondUuid } from "./human-fixtures.mjs";

const hash = "a".repeat(64);
const receipt = (operationId, extra = {}) => ({ version: 1, operation_id: operationId, message_id: "msg-1", session_id: secondUuid,
  input_hash: hash, content_hash: hash, binding_fingerprint: "c".repeat(64), input_epoch: 0, status: "queued", ack: null,
  audit_seq: 21, audit_hash: "b".repeat(64), replayed: false, ...extra });
const counter = () => { let n = 0; return () => `00000000-0000-4000-8000-${(++n).toString(16).padStart(12, "0")}`; };
const apiError = (status, code = "permission_denied") => new HumanApiError(status, code, `refus ${status}`, "retry");

test("la classification distingue conflit, refus, introuvable, invalide, capacité et inconnu", () => {
  assert.equal(messageFailure(apiError(409, "conflict")), "conflict");
  assert.equal(messageFailure(apiError(409, "binding_changed")), "conflict");
  assert.equal(messageFailure(apiError(403)), "refused");
  assert.equal(messageFailure(apiError(401, "authentication_required")), "refused");
  assert.equal(messageFailure(apiError(404, "not_found")), "not_found");
  assert.equal(messageFailure(apiError(400, "invalid_request")), "invalid");
  assert.equal(messageFailure(apiError(429, "capacity_exhausted")), "capacity");
  assert.equal(messageFailure(apiError(503, "identity_unavailable")), "unknown");
  assert.equal(messageFailure(new Error("réseau")), "unknown");
  assert.equal(messageFailure(new HumanContractError()), "invalid");
  assert.equal(messageContextLost(apiError(403, "context_revoked")), true);
  assert.equal(messageContextLost(apiError(403, "permission_denied")), false);
});

test("l’envoi retient un reçu durable et libère l’identifiant après une réponse définitive", async () => {
  const journey = new SessionMessageJourney(counter());
  const seen = [];
  const phase = await journey.send("intent-a", async (id) => { seen.push(id); return receipt(id); });
  assert.equal(phase, "sent"); assert.equal(journey.receipt.operation_id, seen[0]);
  assert.equal(journey.operationId, null, "une réponse définitive libère l’identifiant");
});

test("une même consigne garde le même identifiant ; un résultat inconnu le conserve pour le rejeu", async () => {
  const journey = new SessionMessageJourney(counter());
  const first = journey.operationIdFor("intent-a");
  assert.equal(journey.operationIdFor("intent-a"), first, "la même intention reprend le même identifiant");
  assert.notEqual(journey.operationIdFor("intent-b"), first, "une consigne différente change d’identifiant");
  const sent = [];
  await journey.send("intent-a", async (id) => { sent.push(id); throw apiError(503, "identity_unavailable"); });
  assert.equal(journey.phase, "unknown");
  assert.equal(journey.operationIdFor("intent-a"), sent[0], "le rejeu après inconnu reprend exactement l’opération envoyée");
  // Le rejeu renvoie la même opération ; le contrôle répond son reçu retenu (replayed), jamais un second envoi.
  const replayed = [];
  await journey.send("intent-a", async (id) => { replayed.push(id); return receipt(id, { replayed: true }); });
  assert.deepEqual(replayed, sent, "le rejeu est la même opération, pas un second Submit");
  assert.equal(journey.phase, "sent"); assert.equal(journey.receipt.replayed, true);
});

test("un refus définitif libère l’identifiant ; le contexte perdu n’est pas un refus produit", async () => {
  const journey = new SessionMessageJourney(counter());
  await journey.send("intent-a", async () => { throw apiError(403); });
  assert.equal(journey.phase, "refused"); assert.equal(journey.operationId, null);
  const lost = new SessionMessageJourney(counter());
  await assert.rejects(lost.send("intent-a", async () => { throw new HumanSuperseded(); }), HumanSuperseded);
  assert.equal(lost.phase, "editing"); assert.equal(lost.receipt, null);
});

test("la relecture du statut ne renvoie jamais et conserve le dernier état connu si elle est perdue", async () => {
  const journey = new SessionMessageJourney(counter());
  await journey.send("intent-a", async (id) => receipt(id, { status: "transmitted" }));
  const reads = [];
  await journey.refresh(async (id) => { reads.push(id); return receipt(id, { status: "acknowledged", ack: { rpc_id: 1, turn_id: "turn-1" } }); });
  assert.deepEqual(reads, [journey.receipt.operation_id], "la relecture porte l’opération retenue");
  assert.equal(journey.receipt.status, "acknowledged"); assert.equal(journey.phase, "sent");
  // Une relecture perdue garde le dernier statut et n’envoie rien.
  let sends = 0;
  await journey.refresh(async () => { sends++; throw apiError(503, "identity_unavailable"); });
  assert.equal(sends, 1); assert.equal(journey.receipt.status, "acknowledged", "le dernier état connu est conservé");
  assert.equal(journey.phase, "sent"); assert.ok(journey.error, "le refus de relecture est affiché");
});

test("la relecture n’a pas lieu sans reçu retenu et le reset retire le reçu", async () => {
  const journey = new SessionMessageJourney(counter());
  let reads = 0;
  await journey.refresh(async () => { reads++; return receipt(uuid); });
  assert.equal(reads, 0);
  await journey.send("intent-a", async (id) => receipt(id));
  journey.reset();
  assert.equal(journey.phase, "editing"); assert.equal(journey.receipt, null);
});