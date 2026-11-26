import assert from "node:assert/strict";
import { register } from "node:module";
import { test } from "node:test";
import { catalogue, context, identity, own, page, resource, errorResponse, secondUuid, uuid } from "./human-fixtures.mjs";
import { installNodes } from "./session-dom-fixture.mjs";

// Node loads the real dashboard modules; only the CSS side-effect imports are skipped.
register("data:text/javascript," + encodeURIComponent(`
  export function load(url, options, next) {
    return url.endsWith(".css") ? { format: "module", source: "", shortCircuit: true } : next(url, options);
  }
`));
const { HumanDashboard } = await import("../src/human-view.ts");

const owned = "/api/v1/human/owned/sessions/";
const route = owned + secondUuid + "/conversation";
const send = ["granted", "own_and_shared", "available"];
const contexts = [
  { ...context(), operations: catalogue({ resources_read: own, session_message_send: send }) },
  { ...context("context-b"), operations: catalogue({ resources_read: own }) },
];
const shared = { ...resource(uuid), owner: { ...resource().owner, account_id: secondUuid } };
const exactFacts = {
  version: 1, tenant_id: "tenant-a", source_id: uuid, session_id: secondUuid,
  input_epoch: 1, provider: "codex", protocol: "codex_app_server_v1",
  expires_at: "2099-01-01T00:00:00Z", status: "active",
};
const tick = () => new Promise((resolve) => setImmediate(resolve));

async function mount(t, read, write = () => assert.fail("aucun envoi attendu")) {
  const calls = [];
  let dashboard;
  t.after(() => dashboard?.close());
  const root = installNodes(t, { fetch: async (path, options) => {
    const contextId = options.headers["X-AiGuard-Context"];
    if (path === "/api/v1/human/identity") return Response.json({ ...identity(), contexts });
    if (path.startsWith("/api/v1/human/resources")) return Response.json(page(contextId, [resource(secondUuid), shared]));
    calls.push({ path, method: options.method, contextId, signal: options.signal });
    if (path === route) return read();
    if (path === owned + "messages") return write(JSON.parse(options.body));
    assert.fail("requête inattendue " + path);
  } });
  dashboard = new HumanDashboard(root, "synthetic-token", {
    brand: () => document.createElement("span"), themeButton: () => document.createElement("button"),
  }, () => assert.fail("déconnexion inattendue"));
  await dashboard.start();
  const byKey = (key) => root.descendants().find((node) => node.dataset.focusKey === key);
  const click = (key) => {
    const node = byKey(key);
    assert.ok(node && !node.disabled, "action disponible " + key);
    node.click();
  };
  click("context-0");
  await tick();
  return { root, byKey, click, calls };
}

test("la ligne propriétaire ouvre Sessions et seul le canal actif sélectionné peut recevoir une consigne", async (t) => {
  let facts = { ...exactFacts, status: "prepared", provider: null, protocol: null };
  const sent = [];
  const { root, byKey, click, calls } = await mount(t, () => Response.json(facts), (request) => {
    sent.push(request);
    return Response.json({
      version: 1, operation_id: request.operation_id, message_id: "msg-1", session_id: secondUuid,
      input_hash: "a".repeat(64), content_hash: "b".repeat(64), binding_fingerprint: "c".repeat(64),
      input_epoch: request.expected_input_epoch, status: "queued", ack: null, audit_seq: 21, audit_hash: "d".repeat(64), replayed: false,
    });
  });
  assert.equal(byKey("resource-conversation-1"), undefined, "la session partagée n’offre pas cette lecture");
  click("resource-conversation-0");
  await tick();
  assert.deepEqual(calls.map(({ path, method, contextId }) => [path, method, contextId]), [[route, "GET", "context-a"]]);
  assert.match(root.textContent, /Canal de conversation.*Préparé/);
  assert.equal(byKey("sc-select"), undefined);
  assert.equal(byKey("sm-send"), undefined);

  facts = exactFacts;
  click("sc-refresh");
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Brouillon pour la première liaison.";

  facts = { ...exactFacts, input_epoch: 2 };
  click("sc-refresh");
  assert.equal(byKey("sm-send"), undefined, "le formulaire est retiré pendant la relecture");
  await tick();
  assert.equal(byKey("sm-send"), undefined, "la nouvelle époque exige une sélection explicite");
  click("sc-select");
  assert.equal(byKey("sm-content").value, "", "le brouillon de l’ancienne liaison est retiré");
  byKey("sm-content").value = "Pour ce canal seulement.";
  click("sm-send");
  await tick();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].session_id, secondUuid);
  assert.equal(sent[0].expected_input_epoch, 2);
  assert.match(root.textContent, /Consigne enregistrée/);

  facts = { ...facts, status: "closed" };
  click("sc-refresh");
  await tick();
  assert.match(root.textContent, /Fermé/);
  assert.equal(byKey("sc-select"), undefined);
  assert.equal(byKey("sm-send"), undefined, "STOP retire l’envoi à époque inchangée");
});

test("une lecture temporairement indisponible suspend l’envoi et préserve le rejeu exact", async (t) => {
  let unavailable = false;
  const sent = [];
  const { byKey, click } = await mount(t,
    () => unavailable ? errorResponse(503, "identity_unavailable") : Response.json(exactFacts),
    (request) => { sent.push(request); return errorResponse(503, "identity_unavailable"); });
  click("resource-conversation-0");
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Une opération seulement.";
  click("sm-send");
  await tick();
  assert.equal(sent.length, 1);

  unavailable = true;
  click("sc-refresh");
  await tick();
  assert.equal(byKey("sm-send-retry"), undefined);
  unavailable = false;
  click("sc-refresh");
  await tick();
  click("sm-send-retry");
  await tick();
  assert.deepEqual(sent, [sent[0], sent[0]]);
});

test("une nouvelle époque attend la fin de l’envoi épinglé sur l’ancienne", async (t) => {
  let facts = exactFacts;
  const deferred = Promise.withResolvers();
  const { byKey, click } = await mount(t, () => Response.json(facts), () => deferred.promise);
  click("resource-conversation-0");
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Pour l’ancienne époque.";
  click("sm-send");
  facts = { ...exactFacts, input_epoch: 2 };
  click("sc-refresh");
  await tick();
  try {
    assert.equal(byKey("sc-select").disabled, true);
    assert.equal(byKey("sm-content"), undefined);
  } finally {
    deferred.resolve(errorResponse(503, "identity_unavailable"));
  }
  await tick();
  click("sc-select");
  assert.equal(byKey("sm-content").value, "");
});

test("la vue annule la lecture sortante et ne rend rien dans le contexte suivant", async (t) => {
  const deferred = Promise.withResolvers();
  const { root, byKey, click, calls } = await mount(t, () => deferred.promise);
  click("resource-conversation-0");
  assert.equal(calls.length, 1);
  click("human-nav-contexts");
  assert.equal(calls[0].signal.aborted, true);
  await tick();
  click("context-1");
  await tick();
  deferred.resolve(Response.json(exactFacts));
  await tick();
  assert.match(root.textContent, /tenant-b/);
  assert.doesNotMatch(root.textContent, /Canal de conversation/);
  assert.equal(byKey("resource-conversation-0"), undefined, "le propriétaire sans droit distinct ne peut pas consulter");
  assert.equal(byKey("sm-send"), undefined);
});
