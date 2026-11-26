// Wire of the nominative session-launch journey: closed bodies, receipts and the offer of the catalogue. Pure.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanContractError } from "../src/human-model.ts";
import { catalogue, context, uuid, secondUuid } from "./human-fixtures.mjs";
import { SESSION_LAUNCH_BODY_BYTES, parseSessionLaunchReceipt, parseSessionPrepareReceipt, preparationExpired,
  sessionLaunchAvailable, sessionLaunchBody, sessionLaunchOffers, sessionLaunchUrls, sessionPrepareBody,
  targetReferenceProblem } from "../src/session-launch-model.ts";

const hash = "a".repeat(64);
const auditHash = "b".repeat(64);
const future = new Date(Date.now() + 600000).toISOString();
const prepareReceipt = (extra = {}) => ({ version: 1, operation_id: uuid, preparation_id: "prep-event-1",
  launch_target_id: secondUuid, target_revision: 7, input_hash: hash, expires_at: future,
  audit_seq: 12, audit_hash: auditHash, replayed: false, ...extra });
const conversation = (extra = {}) => ({ version: 1, tenant_id: "tenant-a", source_id: "source-1", session_id: "session-1",
  launch_target_id: secondUuid, target_revision: 7, broker_id: null, broker_incarnation: null, worker_id: null,
  attempt_id: null, runtime_instance: null, input_epoch: 0, provider: null, protocol: null, expires_at: future,
  status: "prepared", ...extra });
const launchReceipt = (extra = {}) => ({ version: 1, operation_id: uuid, launch_id: "launch-event-1", session_id: "session-1",
  launch_target_id: secondUuid, target_revision: 7, preparation_operation_id: secondUuid, preparation_id: "prep-event-1",
  input_hash: hash, grant_revision: 3, grant_audit_seq: 13, grant_audit_hash: auditHash, audit_seq: 14,
  audit_hash: auditHash, conversation: conversation(), replayed: false, ...extra });

test("le corps de préparation est fermé, borné, et ne porte ni chemin ni argv ni credential", () => {
  const body = sessionPrepareBody({ version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7, expires_at: future });
  assert.deepEqual(JSON.parse(body), { version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7, expires_at: future });
  assert.ok(new TextEncoder().encode(body).length <= SESSION_LAUNCH_BODY_BYTES);
  // A nil operation, a nil target, a zero revision or a non-RFC3339 deadline is refused before any request.
  for (const bad of [
    { operation_id: "00000000-0000-0000-0000-000000000000" },
    { launch_target_id: "00000000-0000-0000-0000-000000000000" },
    { expected_target_revision: 0 },
    { expected_target_revision: 1.5 },
    { expires_at: "demain" },
    { version: 2 },
  ]) assert.throws(() => sessionPrepareBody({ version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7, expires_at: future, ...bad }), HumanContractError);
});

test("le corps de lancement consomme la préparation par référence et empreinte exacte", () => {
  const body = sessionLaunchBody({ version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7,
    preparation_operation_id: secondUuid, preparation_input_hash: hash });
  assert.deepEqual(JSON.parse(body), { version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7,
    preparation_operation_id: secondUuid, preparation_input_hash: hash });
  assert.throws(() => sessionLaunchBody({ version: 1, operation_id: uuid, launch_target_id: secondUuid, expected_target_revision: 7,
    preparation_operation_id: secondUuid, preparation_input_hash: "pas-une-empreinte" }), HumanContractError);
});

test("le reçu de préparation se décode et répond exactement l’opération et la cible demandées", () => {
  const receipt = parseSessionPrepareReceipt(prepareReceipt(), { operationId: uuid, launchTargetId: secondUuid });
  assert.equal(receipt.target_revision, 7); assert.equal(receipt.audit_seq, 12); assert.equal(receipt.replayed, false);
  assert.throws(() => parseSessionPrepareReceipt(prepareReceipt({ operation_id: secondUuid }), { operationId: uuid, launchTargetId: secondUuid }), HumanContractError);
  assert.throws(() => parseSessionPrepareReceipt(prepareReceipt({ launch_target_id: uuid }), { operationId: uuid, launchTargetId: secondUuid }), HumanContractError);
  // A read by operation alone does not force the target, but still decodes the closed receipt.
  assert.equal(parseSessionPrepareReceipt(prepareReceipt(), { operationId: uuid }).launch_target_id, secondUuid);
  for (const bad of [{ input_hash: "court" }, { audit_seq: -1 }, { target_revision: 0 }, { replayed: "oui" }, { extra: 1 }])
    assert.throws(() => parseSessionPrepareReceipt(prepareReceipt(bad), { operationId: uuid }), HumanContractError);
});

test("le reçu de lancement décode le canal retenu et garde les faits broker inconnus", () => {
  const receipt = parseSessionLaunchReceipt(launchReceipt(), { operationId: uuid, launchTargetId: secondUuid });
  assert.equal(receipt.session_id, "session-1"); assert.equal(receipt.grant_revision, 3);
  assert.equal(receipt.conversation.status, "prepared"); assert.equal(receipt.conversation.broker_id, null);
  assert.equal(receipt.conversation.input_epoch, 0);
  // The binding must name the same session, target and revision as the receipt: a divergent binding is a contract break.
  assert.throws(() => parseSessionLaunchReceipt(launchReceipt({ conversation: conversation({ session_id: "autre" }) }), { operationId: uuid, launchTargetId: secondUuid }), HumanContractError);
  assert.throws(() => parseSessionLaunchReceipt(launchReceipt({ conversation: conversation({ target_revision: 8 }) }), { operationId: uuid, launchTargetId: secondUuid }), HumanContractError);
  assert.throws(() => parseSessionLaunchReceipt(launchReceipt({ conversation: conversation({ status: "running" }) }), { operationId: uuid, launchTargetId: secondUuid }), HumanContractError);
  // An admitted binding carries real broker facts; they decode without invention.
  const admitted = parseSessionLaunchReceipt(launchReceipt({ conversation: conversation({ broker_id: "broker-1", worker_id: "worker-1", provider: "codex", protocol: "stdio", input_epoch: 2 }) }), { operationId: uuid, launchTargetId: secondUuid });
  assert.equal(admitted.conversation.broker_id, "broker-1"); assert.equal(admitted.conversation.input_epoch, 2);
});

test("les routes sont exactes et l’identifiant de préparation est canonique", () => {
  assert.equal(sessionLaunchUrls.prepare(), "/api/v1/human/owned/sessions/prepare");
  assert.equal(sessionLaunchUrls.launch(), "/api/v1/human/owned/sessions/launch");
  assert.equal(sessionLaunchUrls.preparation(uuid), `/api/v1/human/owned/sessions/preparations/${uuid}`);
  assert.throws(() => sessionLaunchUrls.preparation("pas-un-uuid"), HumanContractError);
});

test("la référence de cible est un UUID canonique et une révision positive, sinon un motif distinct", () => {
  assert.equal(targetReferenceProblem(secondUuid, "7"), null);
  assert.equal(targetReferenceProblem("pas-un-uuid", "7"), "target");
  assert.equal(targetReferenceProblem("00000000-0000-0000-0000-000000000000", "7"), "target");
  assert.equal(targetReferenceProblem(secondUuid, "0"), "revision");
  assert.equal(targetReferenceProblem(secondUuid, "-3"), "revision");
  assert.equal(targetReferenceProblem(secondUuid, "9223372036854775808"), "revision");
});

test("le parcours n’est offert que par le catalogue : préparation et lancement sont deux droits distincts", () => {
  const both = { ...context("context-launch", "personal"), operations: catalogue({
    session_launch_prepare: ["granted", "own_and_shared", "available"], session_launch: ["granted", "own_and_shared", "available"] }) };
  assert.deepEqual(sessionLaunchOffers(both), { prepare: true, launch: true });
  assert.equal(sessionLaunchAvailable(both), true);
  const prepareOnly = { ...context("context-launch", "personal"), operations: catalogue({ session_launch_prepare: ["granted", "own_and_shared", "available"] }) };
  assert.deepEqual(sessionLaunchOffers(prepareOnly), { prepare: true, launch: false });
  // Granted but not served by this build: no request, no destination.
  const fenced = { ...context("context-launch", "personal"), operations: catalogue({ session_launch_prepare: ["granted", "own_and_shared", "not_in_this_build"] }) };
  assert.equal(sessionLaunchAvailable(fenced), false);
  assert.equal(sessionLaunchAvailable(null), false);
  assert.equal(sessionLaunchAvailable(context()), false, "un contexte en lecture seule n’offre aucun lancement");
});

test("une préparation expirée ne peut plus être lancée", () => {
  const past = parseSessionPrepareReceipt(prepareReceipt({ expires_at: "2020-01-01T00:00:00Z" }), { operationId: uuid });
  assert.equal(preparationExpired(past, Date.now()), true);
  assert.equal(preparationExpired(parseSessionPrepareReceipt(prepareReceipt(), { operationId: uuid }), Date.now()), false);
});