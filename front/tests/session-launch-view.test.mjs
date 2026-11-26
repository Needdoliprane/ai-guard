// DOM-level journey of the "Nouvelle session" view: prepare then launch, distinct refusals, unknown-outcome retry,
// and the catalogue gating. A minimal node shim stands in for the browser; the api is a controlled double.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiError } from "../src/human-api.ts";
import { catalogue, context, uuid, secondUuid } from "./human-fixtures.mjs";
import { SessionLaunchView } from "../src/session-launch-view.ts";

class Element extends EventTarget {
  className = ""; classList = { add: (name) => { this.className = `${this.className} ${name}`; } };
  dataset = {}; children = []; attributes = new Map(); disabled = false; parent = null; value = "";
  constructor(tag, text = "") { super(); this.tag = tag; this.text = text; }
  get isConnected() { return this === document.body || this.parent?.isConnected === true; }
  get textContent() { return this.text + this.children.map((child) => child.textContent).join(""); }
  set textContent(text) { this.replaceChildren(); this.text = String(text); }
  contains(node) { return this === node || this.children.some((child) => child.contains(node)); }
  descendants() { return this.children.flatMap((child) => [child, ...child.descendants()]); }
  append(...children) { for (const value of children) { const child = typeof value === "string" ? new Element("#text", value) : value; child.remove(); child.parent = this; this.children.push(child); } }
  replaceChildren(...children) { for (const child of [...this.children]) child.remove(); this.append(...children); }
  remove() { if (!this.parent) return; this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; }
  setAttribute(name, value) { this.attributes.set(name, value); }
  focus() { if (this.isConnected && !this.disabled) document.activeElement = this; }
  click() { if (!this.disabled) this.dispatchEvent(new Event("click")); }
}
function installNodes(t) {
  const body = new Element("body");
  const globals = { Node: Element, HTMLElement: Element,
    document: { body, activeElement: body, createElement: (tag) => new Element(tag),
      createElementNS: (_ns, tag) => new Element(tag), createTextNode: (text) => new Element("#text", text) } };
  for (const [name, value] of Object.entries(globals)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => original ? Object.defineProperty(globalThis, name, original) : Reflect.deleteProperty(globalThis, name));
  }
}
const tick = () => new Promise((resolve) => setImmediate(resolve));
const hash = "a".repeat(64); const auditHash = "b".repeat(64);
const future = new Date(Date.now() + 600000).toISOString();
const prepareReceipt = (operationId) => ({ version: 1, operation_id: operationId, preparation_id: "prep-1",
  launch_target_id: secondUuid, target_revision: 7, input_hash: hash, expires_at: future, audit_seq: 12, audit_hash: auditHash, replayed: false });
const launchReceipt = (operationId) => ({ version: 1, operation_id: operationId, launch_id: "launch-1", session_id: "session-1",
  launch_target_id: secondUuid, target_revision: 7, preparation_operation_id: uuid, preparation_id: "prep-1", input_hash: hash,
  grant_revision: 3, grant_audit_seq: 13, grant_audit_hash: auditHash, audit_seq: 14, audit_hash: auditHash,
  conversation: { version: 1, tenant_id: "tenant-a", source_id: "source-1", session_id: "session-1", launch_target_id: secondUuid,
    target_revision: 7, broker_id: null, broker_incarnation: null, worker_id: null, attempt_id: null, runtime_instance: null,
    input_epoch: 0, provider: null, protocol: null, expires_at: future, status: "prepared" }, replayed: false });
const launchContext = () => ({ ...context("context-launch", "personal"), operations: catalogue({
  session_launch_prepare: ["granted", "own_and_shared", "available"], session_launch: ["granted", "own_and_shared", "available"] }) });
const byKey = (view, key) => view.element.descendants().find((node) => node.dataset.focusKey === key);
const hasText = (view, text) => view.element.descendants().some((node) => node.textContent.includes(text));
function fill(view, target, revision) {
  byKey(view, "sl-target").value = target; byKey(view, "sl-revision").value = revision;
}

test("le parcours prépare puis lance l’intention exacte et distingue accepté de en cours d’exécution", async (t) => {
  installNodes(t);
  const calls = { prepare: [], launch: [] };
  const api = { sessionPrepare: async (request) => { calls.prepare.push(request); return prepareReceipt(request.operation_id); },
    sessionLaunch: async (request) => { calls.launch.push(request); return launchReceipt(request.operation_id); } };
  const view = new SessionLaunchView({ context: launchContext(), api, onAccessDoubt: () => assert.fail("pas de doute d’accès"), onReadStateChange: () => {} });
  try {
    view.start(); fill(view, secondUuid, "7");
    byKey(view, "sl-prepare").click(); await tick();
    assert.equal(calls.prepare.length, 1);
    assert.equal(calls.prepare[0].launch_target_id, secondUuid); assert.equal(calls.prepare[0].expected_target_revision, 7);
    assert.equal(calls.prepare[0].version, 1); assert.ok(calls.prepare[0].expires_at);
    assert.ok(hasText(view, "Préparation retenue"), "la préparation durable est affichée");
    assert.ok(hasText(view, "ne lance rien"), "la préparation ne consomme aucune réservation");
    const launch = byKey(view, "sl-launch"); assert.ok(launch && !launch.disabled, "le lancement est offert après préparation");
    launch.click(); await tick();
    assert.equal(calls.launch.length, 1);
    assert.equal(calls.launch[0].preparation_operation_id, calls.prepare[0].operation_id, "le lancement consomme la préparation exacte");
    assert.equal(calls.launch[0].preparation_input_hash, hash);
    assert.equal(calls.launch[0].expected_target_revision, 7);
    assert.ok(hasText(view, "Lancement accepté"));
    assert.ok(hasText(view, "worker en cours d’exécution"), "accepté n’est pas présenté comme en cours d’exécution");
    assert.ok(hasText(view, "Inconnu"), "les faits broker non admis restent inconnus");
  } finally { view.close(); }
});

test("une référence invalide est refusée localement avant toute requête", async (t) => {
  installNodes(t);
  let calls = 0;
  const api = { sessionPrepare: async () => { calls++; return prepareReceipt(uuid); }, sessionLaunch: async () => launchReceipt(uuid) };
  const view = new SessionLaunchView({ context: launchContext(), api, onAccessDoubt: () => {}, onReadStateChange: () => {} });
  try {
    view.start(); fill(view, "pas-un-uuid", "7");
    byKey(view, "sl-prepare").click(); await tick();
    assert.equal(calls, 0, "aucune requête pour une référence invalide");
    assert.ok(hasText(view, "identifiant opaque"), "le motif de la cible est affiché");
  } finally { view.close(); }
});

test("une réponse de lancement perdue se réconcilie en rejouant exactement le même lancement", async (t) => {
  installNodes(t);
  const prepared = [];
  const launched = [];
  let now = Date.now();
  const api = {
    sessionPrepare: async (request) => {
      prepared.push(request);
      return { ...prepareReceipt(request.operation_id), expires_at: new Date(now + 60_000).toISOString() };
    },
    sessionLaunch: async (request) => {
      launched.push(request);
      if (launched.length === 1) {
        now += 120_000;
        throw new HumanApiError(503, "identity_unavailable", "réponse perdue", "retry");
      }
      return { ...launchReceipt(request.operation_id), replayed: true };
    },
  };
  const view = new SessionLaunchView({
    context: launchContext(), api, now: () => now,
    onAccessDoubt: () => assert.fail("pas de doute d’accès"), onReadStateChange: () => {},
  });
  try {
    view.start();
    fill(view, secondUuid, "7");
    byKey(view, "sl-prepare").click();
    await tick();
    byKey(view, "sl-launch").click();
    await tick();

    const retry = byKey(view, "sl-launch-retry");
    assert.ok(retry && !retry.disabled, "le reçu retenu reste récupérable après expiration");
    assert.ok(byKey(view, "sl-prepare").disabled);
    assert.ok(byKey(view, "sl-reset").disabled);
    retry.click();
    await tick();

    assert.equal(prepared.length, 1);
    assert.equal(launched.length, 2);
    assert.deepEqual(launched[1], launched[0]);
    assert.ok(hasText(view, "Lancement accepté"));
    assert.ok(hasText(view, "Rejeu exact"));
  } finally { view.close(); }
});

test("un refus de permission, un conflit et un résultat inconnu restent trois états distincts", async (t) => {
  installNodes(t);
  for (const [status, code, expected] of [[403, "permission_denied", "Refusé par le contrôle"], [409, "conflict", "Conflit"], [503, "identity_unavailable", "inconnu"]]) {
    let doubts = 0;
    const api = { sessionPrepare: async () => { throw new HumanApiError(status, code, `refus ${status}`, status === 409 ? "reread" : "retry"); }, sessionLaunch: async () => launchReceipt(uuid) };
    const view = new SessionLaunchView({ context: launchContext(), api, onAccessDoubt: () => { doubts++; }, onReadStateChange: () => {} });
    try {
      view.start(); fill(view, secondUuid, "7");
      byKey(view, "sl-prepare").click(); await tick();
      assert.ok(hasText(view, expected), `état ${status} affiché`);
      assert.equal(doubts, 0, "un refus métier n’est pas une perte de contexte");
      if (status === 503) {
        assert.ok(byKey(view, "sl-prepare-retry"), "le rejeu de la même opération est proposé");
        assert.ok(hasText(view, "réessayer reprend la même opération"));
      }
    } finally { view.close(); }
  }
});

test("un contexte qui n’offre pas la préparation désactive l’action et le dit", async (t) => {
  installNodes(t);
  let calls = 0;
  const api = { sessionPrepare: async () => { calls++; return prepareReceipt(uuid); }, sessionLaunch: async () => launchReceipt(uuid) };
  const readOnly = { ...context("context-launch", "personal"), operations: catalogue() };
  const view = new SessionLaunchView({ context: readOnly, api, onAccessDoubt: () => {}, onReadStateChange: () => {} });
  try {
    view.start(); fill(view, secondUuid, "7");
    const prepare = byKey(view, "sl-prepare"); assert.ok(prepare.disabled, "aucune action offerte");
    prepare.click(); await tick();
    assert.equal(calls, 0); assert.ok(hasText(view, "n’offre pas la préparation"));
  } finally { view.close(); }
});
