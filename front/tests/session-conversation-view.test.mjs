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

async function mount(t, read, write = () => assert.fail("aucun envoi attendu"),
  receipt = () => assert.fail("aucune relecture de reçu attendue"), offered = contexts) {
  const calls = [];
  const accessReads = [];
  let dashboard;
  t.after(() => dashboard?.close());
  const root = installNodes(t, { fetch: async (path, options) => {
    const contextId = options.headers["X-AiGuard-Context"];
    if (path === "/api/v1/human/identity") {
      accessReads.push(path);
      return Response.json({ ...identity(), contexts: offered });
    }
    if (path.startsWith("/api/v1/human/resources")) {
      accessReads.push(path);
      return Response.json(page(contextId, [resource(secondUuid), shared]));
    }
    calls.push({ path, method: options.method, contextId, signal: options.signal });
    if (path.startsWith(owned) && path.endsWith("/conversation")) return read(path);
    if (path === owned + "messages") return write(JSON.parse(options.body));
    if (path.startsWith(owned + "messages/")) return receipt(path.split("/").at(-1));
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
  return { root, byKey, click, calls, accessReads };
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
  assert.equal(byKey("sc-select").disabled, true);
  click("sm-unknown-abandon");
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

test("l’échéance retire l’envoi pendant que Sessions reste ouverte, sans nouvelle requête", async (t) => {
  const now = Date.parse("2026-11-26T20:00:00Z");
  t.mock.timers.enable({ apis: ["Date", "setTimeout"], now });
  const facts = { ...exactFacts, expires_at: new Date(now + 1_000).toISOString() };
  const { root, byKey, click, calls } = await mount(t, () => Response.json(facts));
  click("resource-conversation-0");
  await tick();
  click("sc-select");
  assert.ok(byKey("sm-send"));

  t.mock.timers.tick(1_000);
  assert.match(root.textContent, /Expiré/);
  assert.equal(byKey("sc-select"), undefined);
  assert.equal(byKey("sm-send"), undefined);
  assert.deepEqual(calls.map(({ method }) => method), ["GET"]);
});

for (const [code, latest, expectedStatus] of [
  ["session_closed", { ...exactFacts, status: "closed" }, /Fermé/],
  ["binding_changed", { ...exactFacts, input_epoch: 2 }, /Époque d’entrée 2/],
  ["binding_expired", exactFacts, /Expiré/],
]) {
  test("le refus " + code + " retire la sélection et provoque une unique relecture", async (t) => {
    const held = Promise.withResolvers();
    let reads = 0;
    const sent = [];
    const { root, byKey, click, calls } = await mount(t,
      () => ++reads === 1 ? Response.json(exactFacts) : held.promise,
      (request) => { sent.push(request); return errorResponse(409, code, "reread"); });
    click("resource-conversation-0");
    await tick();
    click("sc-select");
    byKey("sm-content").value = "Pour cette époque.";
    click("sm-send");
    await tick();
    assert.equal(byKey("sm-send"), undefined);
    assert.deepEqual(calls.map(({ method }) => method), ["GET", "POST", "GET"]);
    held.resolve(Response.json(latest));
    await tick();
    assert.match(root.textContent, expectedStatus);
    assert.equal(byKey("sm-send"), undefined);
    if (code === "binding_changed") {
      click("sc-select");
      assert.equal(byKey("sm-content").value, "");
    } else {
      assert.equal(byKey("sc-select"), undefined);
    }
    assert.equal(sent.length, 1);
    assert.equal(sent[0].expected_input_epoch, 1);
  });
}

test("un refus au rejeu garde l’opération inconnue et permet la relecture exacte du reçu", async (t) => {
  let facts = exactFacts;
  const sent = [];
  const checked = [];
  const { root, byKey, click, calls } = await mount(t, () => Response.json(facts),
    (request) => {
      sent.push(request);
      if (sent.length === 1) return errorResponse(503, "identity_unavailable");
      facts = { ...exactFacts, input_epoch: 2 };
      return errorResponse(409, "binding_changed", "reread");
    }, (operationId) => {
      checked.push(operationId);
      return Response.json({
        version: 1, operation_id: operationId, message_id: "msg-1", session_id: secondUuid,
        input_hash: "a".repeat(64), content_hash: "b".repeat(64), binding_fingerprint: "c".repeat(64),
        input_epoch: 1, status: "queued", ack: null, audit_seq: 21, audit_hash: "d".repeat(64), replayed: true,
      });
    });
  click("resource-conversation-0");
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Opération épinglée.";
  click("sm-send");
  await tick();
  click("sm-send-retry");
  await tick();
  assert.deepEqual(sent, [sent[0], sent[0]]);
  assert.match(root.textContent, /résultat de l’envoi épinglé reste inconnu/);
  assert.equal(byKey("sc-select").disabled, true);
  assert.equal(byKey("sm-send-retry"), undefined);
  click("sm-unknown-refresh");
  await tick();
  assert.deepEqual(checked, [sent[0].operation_id]);
  assert.deepEqual(calls.map(({ method }) => method), ["GET", "POST", "POST", "GET", "GET"]);
  assert.match(root.textContent, /Consigne enregistrée/);
  click("sc-select");
  assert.equal(byKey("sm-content").value, "");
});

test("une lecture qui répète l’époque refusée ne rouvre pas le canal", async (t) => {
  let facts = exactFacts;
  let posts = 0;
  const { root, byKey, click } = await mount(t, () => Response.json(facts), () => {
    posts++;
    return errorResponse(409, "binding_changed", "reread");
  });
  click("resource-conversation-0");
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Époque déjà périmée.";
  click("sm-send");
  await tick();
  assert.match(root.textContent, /Canal modifié/);
  assert.equal(byKey("sc-select"), undefined);
  assert.equal(byKey("sm-send"), undefined);
  facts = { ...exactFacts, input_epoch: 2 };
  click("sc-refresh");
  await tick();
  click("sc-select");
  assert.equal(byKey("sm-content").value, "");
  assert.equal(posts, 1);
});

test("l’expiration conserve un envoi incertain consultable par son identifiant", async (t) => {
  const now = Date.parse("2026-11-26T20:00:00Z");
  t.mock.timers.enable({ apis: ["Date", "setTimeout"], now });
  const facts = { ...exactFacts, expires_at: new Date(now + 1_000).toISOString() };
  const sent = [];
  const checked = [];
  const { root, byKey, click, calls } = await mount(t, () => Response.json(facts),
    (request) => { sent.push(request); return errorResponse(503, "identity_unavailable"); },
    (id) => { checked.push(id); return errorResponse(404, "not_found"); });
  click("resource-conversation-0");
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Une seule consigne.";
  click("sm-send");
  await tick();
  t.mock.timers.tick(1_000);
  assert.match(root.textContent, /Expiré/);
  assert.match(root.textContent, /résultat de l’envoi épinglé reste inconnu/);
  assert.equal(byKey("sm-send-retry"), undefined);
  assert.deepEqual(calls.map(({ method }) => method), ["GET", "POST"]);
  click("sm-unknown-refresh");
  await tick();
  assert.deepEqual(checked, [sent[0].operation_id]);
  assert.match(root.textContent, /résultat de l’envoi épinglé reste inconnu/);
  assert.deepEqual(calls.map(({ method }) => method), ["GET", "POST", "GET"]);
});

const sendOnly = [{
  ...context(),
  capabilities: ["send_session_message"],
  operations: catalogue({ session_message_send: send }),
}];

function submitKnownSession(byKey, sessionId) {
  const input = byKey("sc-known-id");
  const form = byKey("sc-known-form");
  assert.ok(input && form, "la saisie directe est proposée dans Sessions");
  input.value = sessionId;
  const submit = new Event("submit", { cancelable: true });
  form.dispatchEvent(submit);
  assert.equal(submit.defaultPrevented, true, "l’identifiant ne devient pas une navigation");
}

test("Send seul ouvre Sessions et consulte uniquement l’UUID canonique saisi", async (t) => {
  const { root, byKey, click, calls, accessReads } = await mount(t, () => Response.json(exactFacts),
    undefined, undefined, sendOnly);
  assert.equal(byKey("human-nav-sessions").attributes.get("aria-current"), "page");
  assert.equal(byKey("human-nav-resources").disabled, true);
  for (const id of ["", "00000000-0000-0000-0000-000000000000", "AAAAAAAA-0000-4000-8000-000000000000",
    secondUuid + "?x=1", " " + secondUuid]) {
    submitKnownSession(byKey, id);
    assert.match(root.textContent, /UUID canonique/);
  }
  assert.equal(calls.length, 0);
  submitKnownSession(byKey, secondUuid);
  await tick();
  assert.deepEqual(accessReads, ["/api/v1/human/identity"]);
  assert.deepEqual(calls.map(({ path, method, contextId }) => [path, method, contextId]), [[route, "GET", "context-a"]]);
  assert.equal(byKey("sm-send"), undefined);
  click("sc-select");
  assert.ok(byKey("sm-send"));
});

test("Send seul présente le même écran pour un refus et une session introuvable", async (t) => {
  let responseStatus = 403;
  const { root, byKey, calls, accessReads } = await mount(t,
    () => errorResponse(responseStatus, responseStatus === 403 ? "permission_denied" : "not_found"),
    undefined, undefined, sendOnly);
  submitKnownSession(byKey, secondUuid);
  await tick();
  const refused = root.textContent;
  assert.match(refused, /Session indisponible dans ce contexte/);
  assert.equal(byKey("sc-select"), undefined);
  assert.equal(byKey("sm-send"), undefined);

  responseStatus = 404;
  submitKnownSession(byKey, secondUuid);
  await tick();
  assert.equal(root.textContent, refused);
  assert.deepEqual(calls.map(({ path, method }) => [path, method]), [[route, "GET"], [route, "GET"]]);
  assert.deepEqual(accessReads, ["/api/v1/human/identity"]);
});

test("saisir une autre session ne redirige pas l’opération déjà incertaine", async (t) => {
  const otherRoute = owned + uuid + "/conversation";
  const held = Promise.withResolvers();
  const sent = [];
  const checked = [];
  const { byKey, click, calls } = await mount(t,
    (path) => path === route ? Response.json(exactFacts) : held.promise,
    (request) => { sent.push(request); return errorResponse(503, "identity_unavailable"); },
    (operationId) => {
      checked.push(operationId);
      return Response.json({
        version: 1, operation_id: operationId, message_id: "msg-1", session_id: secondUuid,
        input_hash: "a".repeat(64), content_hash: "b".repeat(64), binding_fingerprint: "c".repeat(64),
        input_epoch: 1, status: "queued", ack: null, audit_seq: 21, audit_hash: "d".repeat(64), replayed: true,
      });
    }, sendOnly);
  submitKnownSession(byKey, secondUuid);
  await tick();
  click("sc-select");
  byKey("sm-content").value = "Consigne de la première session.";
  click("sm-send");
  await tick();

  submitKnownSession(byKey, uuid);
  assert.equal(byKey("sm-send-retry"), undefined);
  assert.equal(byKey("sm-content"), undefined);
  held.resolve(Response.json({ ...exactFacts, session_id: uuid, input_epoch: 8 }));
  await tick();
  assert.equal(byKey("sc-select").disabled, true);
  assert.equal(sent.length, 1);

  click("sm-unknown-refresh");
  await tick();
  assert.deepEqual(checked, [sent[0].operation_id]);
  click("sc-select");
  assert.equal(byKey("sm-content").value, "");
  byKey("sm-content").value = "Consigne distincte.";
  click("sm-send");
  await tick();
  assert.deepEqual(calls.filter(({ method }) => method === "GET").slice(0, 2).map(({ path }) => path), [route, otherRoute]);
  assert.deepEqual(sent.map(({ session_id, expected_input_epoch }) => [session_id, expected_input_epoch]), [[secondUuid, 1], [uuid, 8]]);
  assert.notEqual(sent[0].operation_id, sent[1].operation_id);
});

test("un contexte révoqué retire la saisie et relit les accès", async (t) => {
  const { root, byKey, calls, accessReads } = await mount(t, () => errorResponse(403, "context_revoked", "select_context"),
    undefined, undefined, sendOnly);
  submitKnownSession(byKey, secondUuid);
  await tick();
  assert.equal(byKey("sc-known-id"), undefined);
  assert.equal(byKey("sm-send"), undefined);
  assert.match(root.textContent, /Choisir un contexte/);
  assert.deepEqual(calls.map(({ path, method }) => [path, method]), [[route, "GET"]]);
  assert.deepEqual(accessReads, ["/api/v1/human/identity", "/api/v1/human/identity"]);
});
