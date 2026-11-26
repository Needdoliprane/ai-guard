// DOM-level journey of the "Envoyer une consigne" view: send to a real destination, truthful distinct statuses,
// unknown-outcome replay of the same operation, status refresh, and catalogue gating. A minimal node shim stands in
// for the browser; the api is a controlled double.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiError } from "../src/human-api.ts";
import { catalogue, context, uuid, secondUuid } from "./human-fixtures.mjs";
import { SessionMessageView } from "../src/session-message-view.ts";
import { installNodes } from "./session-dom-fixture.mjs";

const tick = () => new Promise((resolve) => setImmediate(resolve));
const hash = "a".repeat(64); const auditHash = "b".repeat(64);
const future = new Date(Date.now() + 600000).toISOString();
const destination = (extra = {}) => ({ sessionId: secondUuid, inputEpoch: 0, tenantId: "tenant-a", sourceId: "source-1",
  provider: null, protocol: null, expiresAt: future, ...extra });
const receipt = (operationId, extra = {}) => ({ version: 1, operation_id: operationId, message_id: "msg-1", session_id: secondUuid,
  input_hash: hash, content_hash: hash, binding_fingerprint: "c".repeat(64), input_epoch: 0, status: "queued", ack: null,
  audit_seq: 21, audit_hash: auditHash, replayed: false, ...extra });
const sendContext = () => ({ ...context("context-send", "personal"), operations: catalogue({ session_message_send: ["granted", "own_and_shared", "available"] }) });
const byKey = (view, key) => view.element.descendants().find((node) => node.dataset.focusKey === key);
const hasText = (view, text) => view.element.descendants().some((node) => node.textContent.includes(text));

test("l’envoi porte la destination exacte et n’est jamais présenté comme exécuté", async (t) => {
  installNodes(t);
  const calls = [];
  const api = { sessionMessageSend: async (request) => { calls.push(request); return receipt(request.operation_id); },
    sessionMessageReceipt: async () => assert.fail("pas de relecture implicite") };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => assert.fail("pas de doute"), onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Résume le journal.";
    byKey(view, "sm-send").click(); await tick();
    assert.equal(calls.length, 1);
    assert.equal(calls[0].session_id, secondUuid); assert.equal(calls[0].expected_input_epoch, 0);
    assert.equal(calls[0].content, "Résume le journal."); assert.equal(calls[0].version, 1);
    assert.ok(hasText(view, "Consigne enregistrée"));
    assert.ok(hasText(view, "queued"), "le statut durable est nommé");
    assert.ok(hasText(view, "ne signifie que la consigne a été exécutée"), "aucun statut n’est présenté comme exécuté");
    assert.ok(byKey(view, "sm-send").disabled, "pas de double activation après un reçu");
  } finally { view.close(); }
});

test("un résultat inconnu conserve l’opération : réessayer rejoue la même, jamais un second envoi", async (t) => {
  installNodes(t);
  const calls = [];
  const api = { sessionMessageSend: async (request) => { calls.push(request); throw new HumanApiError(503, "identity_unavailable", "indisponible", "retry"); },
    sessionMessageReceipt: async () => assert.fail("pas de relecture") };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Consigne unique.";
    byKey(view, "sm-send").click(); await tick();
    assert.ok(hasText(view, "premier envoi reste inconnu"));
    const retry = byKey(view, "sm-send-retry"); assert.ok(retry && !retry.disabled, "le rejeu est offert");
    retry.click(); await tick();
    assert.equal(calls.length, 2);
    assert.equal(calls[0].operation_id, calls[1].operation_id, "le rejeu est la même opération");
    assert.equal(calls[0].content, calls[1].content, "le contenu exact est rejoué, pas un second Submit");
  } finally { view.close(); }
});

test("la relecture du statut est explicite et met à jour le reçu sans renvoyer", async (t) => {
  installNodes(t);
  let sends = 0; const reads = [];
  const api = { sessionMessageSend: async (request) => { sends++; return receipt(request.operation_id, { status: "transmitted" }); },
    sessionMessageReceipt: async (id) => { reads.push(id); return receipt(id, { status: "acknowledged", ack: { rpc_id: 1, turn_id: "turn-1" } }); } };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Consigne.";
    byKey(view, "sm-send").click(); await tick();
    assert.ok(hasText(view, "transmitted"));
    byKey(view, "sm-status-refresh").click(); await tick();
    assert.equal(sends, 1, "la relecture ne renvoie jamais");
    assert.equal(reads.length, 1);
    assert.ok(hasText(view, "acknowledged")); assert.ok(hasText(view, "turn-1"), "l’acquittement corrélé est affiché");
  } finally { view.close(); }
});

test("le statut unknown conserve l’incertitude et n’annonce aucun renvoi automatique", async (t) => {
  installNodes(t);
  const api = { sessionMessageSend: async (request) => receipt(request.operation_id, { status: "unknown" }),
    sessionMessageReceipt: async (id) => receipt(id, { status: "unknown" }) };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Consigne.";
    byKey(view, "sm-send").click(); await tick();
    assert.ok(hasText(view, "unknown"));
    assert.ok(hasText(view, "jamais renvoyée automatiquement"));
  } finally { view.close(); }
});

test("un contexte sans droit d’envoi ou sans destination n’émet aucune requête", async (t) => {
  installNodes(t);
  let sends = 0;
  const api = { sessionMessageSend: async () => { sends++; return receipt(uuid); }, sessionMessageReceipt: async () => receipt(uuid) };
  const readOnly = { ...context("context-read", "personal"), operations: catalogue() };
  const noRight = new SessionMessageView({ context: readOnly, api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, destination());
  try {
    noRight.start(); byKey(noRight, "sm-content").value = "Consigne.";
    assert.ok(byKey(noRight, "sm-send").disabled);
    byKey(noRight, "sm-send").click(); await tick();
    assert.ok(hasText(noRight, "n’offre pas l’envoi"));
  } finally { noRight.close(); }
  const noDest = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, null);
  try {
    noDest.start(); byKey(noDest, "sm-content").value = "Consigne.";
    assert.ok(byKey(noDest, "sm-send").disabled, "sans liaison réelle, aucun envoi");
    byKey(noDest, "sm-send").click(); await tick();
    assert.ok(hasText(noDest, "Aucune session retenue"));
  } finally { noDest.close(); }
  assert.equal(sends, 0);
});

test("un contexte perdu pendant l’envoi retire la vue et relit les accès, sans refus produit", async (t) => {
  installNodes(t);
  let doubt = 0;
  const api = { sessionMessageSend: async () => { throw new HumanApiError(403, "context_revoked", "révoqué", "select_context"); },
    sessionMessageReceipt: async () => receipt(uuid) };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => { doubt++; }, onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Consigne.";
    byKey(view, "sm-send").click(); await tick();
    assert.equal(doubt, 1, "le contexte perdu déclenche la relecture des accès");
  } finally { view.close(); }
});

test("une destination changée (époque avancée) retire le reçu retenu de l’ancienne liaison", async (t) => {
  installNodes(t);
  const api = { sessionMessageSend: async (request) => receipt(request.operation_id), sessionMessageReceipt: async (id) => receipt(id) };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Consigne.";
    byKey(view, "sm-send").click(); await tick();
    assert.ok(hasText(view, "Consigne enregistrée"));
    view.setDestination(destination({ inputEpoch: 1 }));
    assert.ok(!hasText(view, "Consigne enregistrée"), "le reçu de l’ancienne époque est retiré");
    assert.ok(hasText(view, "Époque d’entrée"));
  } finally { view.close(); }
});
test("une relecture tardive du reçu ne réapparaît pas après le changement d’époque", async (t) => {
  installNodes(t);
  const held = Promise.withResolvers();
  let signal;
  const api = { sessionMessageSend: async (request) => receipt(request.operation_id),
    sessionMessageReceipt: (_id, current) => { signal = current; return held.promise; } };
  const view = new SessionMessageView({ context: sendContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} }, destination());
  try {
    view.start(); byKey(view, "sm-content").value = "Consigne.";
    byKey(view, "sm-send").click(); await tick();
    byKey(view, "sm-status-refresh").click();
    view.setDestination(destination({ inputEpoch: 1 }));
    assert.equal(signal.aborted, true);
    held.resolve(receipt(uuid, { status: "transmitted" }));
    await tick();
    assert.equal(hasText(view, "Consigne enregistrée"), false);
  } finally { view.close(); }
});
