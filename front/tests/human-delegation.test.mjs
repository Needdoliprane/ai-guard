// Delegated membership and share changes: strict wire, the client requests and the life of one change request.
// Real HumanApiClient over a recorded fetch. No DOM.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { grantChangeBody, membershipChangeBody, parseGrantChangeReceipt, parseHumanGrant, parseMembershipChangeReceipt } from "../src/admin-model.ts";
import { ChangeFlow, MembershipChangeRecovery } from "../src/delegation-model.ts";
import { catalogue, context, identity } from "./human-fixtures.mjs";
import { grantBody, grantReceipt, memberPrincipal, membershipPage, membershipReceipt } from "./admin-fixtures.mjs";

const adminId = "c".repeat(64);
const served = ["granted", "tenant", "available"];
const offer = (overrides = {}) => catalogue({ memberships_read: served, memberships_manage: served, grants_manage: served, ...overrides });
const offered = (operations = offer()) => ({ ...identity(), contexts: [{ ...context(adminId, "administration"),
  capabilities: ["read_metadata", "manage_memberships", "manage_grants", "read_memberships"], operations }] });
const membershipId = "00000000-0000-4000-8000-0000000000c1";
const resource = { kind: "session", resource_id: "session-a" };
let sequence = 0;
const newId = () => `00000000-0000-4000-8000-${(0xd00 + ++sequence).toString(16).padStart(12, "0")}`;
const json = (status, value) => Response.json(value, { status });
const refusal = (status, code, retry_action) => json(status, { error: { code, message: `Refus ${code}`, retry_action } });

function client(handler, identityBody = () => offered()) {
  const notifications = []; const requests = [];
  const api = new HumanApiClient("private-human-bearer-123456789", {
    onAuthentication: (error) => notifications.push(["authentication", error.code]),
    onContextInvalidated: (error) => notifications.push(["context", error.code]),
  }, async (path, options) => {
    requests.push({ path, options, body: options.body === undefined ? undefined : JSON.parse(options.body) });
    return path.endsWith("/identity") ? Response.json(identityBody()) : handler(path, options, requests.at(-1));
  });
  return { api, notifications, requests };
}

test("Une réponse d’appartenance perdue propose le rejeu exact sous une offre Administration fraîche", async () => {
  const recovery = new MembershipChangeRecovery();
  const nextId = "d".repeat(64);
  let frame = offered();
  let original;
  const { api, requests } = client((path, options, call) => {
    if (!original) {
      original = membershipReceipt(call.body.operation_id, membershipId);
      frame = { ...frame, contexts: [{ ...frame.contexts[0], id: nextId, authorization_version: "b".repeat(64) }] };
      throw new TypeError("Réponse interrompue après enregistrement.");
    }
    return json(200, original);
  }, () => frame);
  try {
    const initial = await api.identity();
    const selected = api.selectContext(adminId);
    const change = { operation_id: newId(), expected_revision: "3", capabilities: ["export", "read_metadata"], active: true };
    const send = (id, request) => api.changeMembership(id, request);
    await assert.rejects(recovery.submit(initial.principal, selected, membershipId, change, send), TypeError);
    assert.equal(recovery.offer(Date.now()), null);
    assert.equal(await recovery.retry(Date.now(), () => assert.fail("Une ancienne offre ne peut pas servir au rejeu.")), null);
    change.capabilities.length = 0;
    change.active = false;
    api.clearContext();
    recovery.revalidate(await api.identity(), Date.now());
    assert.equal(recovery.offer(Date.now())?.contextId, nextId);
    assert.equal(requests.filter(({ options }) => options.method === "POST").length, 1);
    const replay = await recovery.retry(Date.now(), (contextId, id, request) => {
      api.selectContext(contextId);
      return send(id, request);
    });
    assert.deepEqual(replay, original);
    const [first, second] = requests.filter(({ options }) => options.method === "POST");
    assert.equal(first.path, second.path);
    assert.equal(first.options.body, second.options.body);
    assert.equal(first.options.headers["X-AiGuard-Context"], adminId);
    assert.equal(second.options.headers["X-AiGuard-Context"], nextId);
    assert.equal(recovery.offer(Date.now()), null);
    assert.equal(requests.filter(({ options }) => options.method === "GET").length, 2);
  } finally { api.close(); }
});

test("Un 503 garde l’intention jusqu’à relecture et un refus terminal ferme la reprise", async () => {
  const recovery = new MembershipChangeRecovery();
  let attempts = 0;
  let frame = offered();
  const { api, requests } = client(() => ++attempts === 1 ?
    refusal(503, "audit_unavailable", "contact_operator") : refusal(403, "permission_denied", "review_permissions"), () => frame);
  try {
    const initial = await api.identity();
    const context = api.selectContext(adminId);
    const change = { operation_id: newId(), expected_revision: "3", capabilities: [], active: false };
    const send = (id, request) => api.changeMembership(id, request);
    await assert.rejects(recovery.submit(initial.principal, context, membershipId, change, send), { status: 503 });
    assert.equal(recovery.unavailable, true);
    assert.equal(recovery.offer(Date.now()), null);
    assert.equal(attempts, 1);
    api.clearContext();
    frame = { ...offered(), contexts: [], contexts_complete: false };
    recovery.revalidate(await api.identity(), Date.now());
    assert.equal(recovery.operationId, change.operation_id);
    assert.equal(recovery.offer(Date.now()), null);
    frame = offered();
    recovery.revalidate(await api.identity(), Date.now());
    assert.equal(recovery.offer(Date.now())?.contextId, adminId, "Une offre relue peut garder son identifiant.");
    await assert.rejects(recovery.retry(Date.now(), (id, target, request) => { api.selectContext(id); return send(target, request); }), { status: 403 });
    assert.equal(recovery.operationId, null);
    recovery.revalidate(await api.identity(), Date.now());
    assert.equal(recovery.offer(Date.now()), null);
    assert.equal(await recovery.retry(Date.now(), () => assert.fail("Le refus terminal a été reçu.")), null);
    const [first, second] = requests.filter(({ options }) => options.method === "POST");
    assert.equal(first.options.body, second.options.body);
    assert.equal(attempts, 2);
  } finally { api.close(); }
});

for (const [label, substitute] of [
  ["d’identité", (frame) => ({ ...frame, principal: memberPrincipal(1) })],
  ["de périmètre", (frame) => ({ ...frame, contexts: [{ ...frame.contexts[0], organization_scope: "organization-b" }] })],
  ["de droits", () => offered(offer({ memberships_manage: ["granted", "tenant", "not_in_this_build"] }))],
]) {
  test(`Un changement ${label} retire l’intention d’appartenance`, async () => {
    const recovery = new MembershipChangeRecovery();
    let frame = offered();
    const { api, requests } = client(() => { throw new TypeError("Réponse interrompue."); }, () => frame);
    try {
      const initial = await api.identity();
      const selected = api.selectContext(adminId);
      const change = { operation_id: newId(), expected_revision: "3", capabilities: [], active: false };
      await assert.rejects(recovery.submit(initial.principal, selected, membershipId, change, (id, request) => api.changeMembership(id, request)), TypeError);
      api.clearContext();
      frame = substitute(frame);
      recovery.revalidate(await api.identity(), Date.now());
      assert.equal(recovery.operationId, null);
      frame = offered();
      recovery.revalidate(await api.identity(), Date.now());
      assert.equal(recovery.offer(Date.now()), null);
      assert.equal(requests.filter(({ options }) => options.method === "POST").length, 1);
    } finally { api.close(); }
  });
}

test("Le reçu du premier envoi clôt le changement sans offre de reprise", async () => {
  const recovery = new MembershipChangeRecovery();
  const change = { operation_id: newId(), expected_revision: "3", capabilities: [], active: false };
  const receipt = membershipReceipt(change.operation_id, membershipId);
  const { api, requests } = client(() => {
    assert.equal(recovery.operationId, change.operation_id);
    assert.equal(recovery.phase, "sending");
    return json(200, receipt);
  });
  try {
    const initial = await api.identity();
    const context = api.selectContext(adminId);
    assert.deepEqual(await recovery.submit(initial.principal, context, membershipId, change, (id, request) => api.changeMembership(id, request)), receipt);
    assert.equal(recovery.operationId, null);
    recovery.revalidate(await api.identity(), Date.now());
    assert.equal(recovery.offer(Date.now()), null);
    assert.equal(requests.filter(({ options }) => options.method === "POST").length, 1);
  } finally { api.close(); }
});

test("le reçu d’appartenance répond exactement à l’opération et à l’appartenance demandées", () => {
  const expected = { operationId: newId(), membershipId };
  const valid = membershipReceipt(expected.operationId, membershipId);
  assert.deepEqual(parseMembershipChangeReceipt(valid, expected), valid);
  for (const bad of [{ ...valid, extra: true }, { ...valid, version: 2 }, { ...valid, operation_id: newId() }, { ...valid, membership_id: "00000000-0000-4000-8000-0000000000c2" },
    { ...valid, membership_revision: 4 }, { ...valid, authorization_revision: "0" }, { ...valid, audit_seq: "01" }, { ...valid, audit_seq: "-1" }])
    assert.throws(() => parseMembershipChangeReceipt(bad, expected), HumanContractError);
  const { audit_seq: _seq, ...missing } = valid;
  assert.throws(() => parseMembershipChangeReceipt(missing, expected), HumanContractError);
});

test("le grant lu nomme une ressource de ce contexte et refuse un partage au propriétaire ou en double", () => {
  const expected = { contextId: adminId, authorizationVersion: "a".repeat(64), resource };
  const parsed = parseHumanGrant(grantBody(adminId), expected);
  assert.equal(parsed.revision, "4"); assert.deepEqual(parsed.shares[0].permissions, ["metadata"]);
  assert.deepEqual(parseHumanGrant(grantBody(adminId, { shares: [{ principal: memberPrincipal(2), display_name: "Two", permissions: ["export", "metadata"] }] }), expected)
    .shares[0].permissions, ["metadata", "export"], "permissions are kept in the wire order");
  const share = (extra = {}) => ({ principal: memberPrincipal(2), display_name: "Two", permissions: ["metadata"], ...extra });
  for (const extra of [{ context_id: "d".repeat(64) }, { source: "tenant_resources" }, { resource: { kind: "session", resource_id: "session-b" } },
    { resource: { kind: "file", resource_id: "session-a" } }, { revision: "0" }, { active: "yes" }, { owner_display_name: "" }, { extra: 1 },
    { shares: [share({ principal: memberPrincipal(1) })] }, { shares: [share(), share()] }, { shares: [share({ permissions: [] })] },
    { shares: [share({ permissions: ["metadata", "metadata"] })] }, { shares: [share({ permissions: ["admin"] })] }, { shares: [share({ grant: true })] },
    { shares: Array.from({ length: 65 }, (_, index) => share({ principal: { kind: "human_oidc", issuer: "https://idp.example.test", subject: `u${index}` } })) }])
    assert.throws(() => parseHumanGrant(grantBody(adminId, extra), expected), HumanContractError, JSON.stringify(extra).slice(0, 60));
  const receiptId = newId();
  assert.equal(parseGrantChangeReceipt(grantReceipt(receiptId), { operationId: receiptId, resource }).grant_revision, "5");
  for (const bad of [grantReceipt(newId()), grantReceipt(receiptId, { kind: "session", resource_id: "other" }), grantReceipt(receiptId, resource, { grant_revision: "x" }),
    { ...grantReceipt(receiptId), owner: memberPrincipal(1) }])
    assert.throws(() => parseGrantChangeReceipt(bad, { operationId: receiptId, resource }), HumanContractError);
});

test("les corps des changements sont fermés, canoniques et bornés avant toute requête", () => {
  const operation = newId();
  assert.deepEqual(JSON.parse(membershipChangeBody({ operation_id: operation, expected_revision: "3", capabilities: ["export", "read_metadata"], active: true })),
    { operation_id: operation, expected_revision: "3", capabilities: ["read_metadata", "export"], active: true });
  for (const bad of [{ capabilities: ["read_metadata", "read_metadata"] }, { capabilities: ["admin"] }, { operation_id: "op-1" }, { expected_revision: "0" }, { active: "true" }])
    assert.throws(() => membershipChangeBody({ operation_id: operation, expected_revision: "3", capabilities: [], active: true, ...bad }), HumanContractError);
  const shares = [{ principal: memberPrincipal(2), permissions: ["content", "metadata"] }];
  assert.deepEqual(JSON.parse(grantChangeBody({ operation_id: operation, resource, expected_revision: "4", shares })).shares[0].permissions, ["metadata", "content"]);
  for (const bad of [[...shares, ...shares], [{ principal: memberPrincipal(2), permissions: [] }], [{ principal: memberPrincipal(1), permissions: ["metadata"] }],
    Array.from({ length: 65 }, (_, index) => ({ principal: { kind: "human_oidc", issuer: "https://idp.example.test", subject: `u${index}` }, permissions: ["metadata"] }))])
    assert.throws(() => grantChangeBody({ operation_id: operation, resource, expected_revision: "4", shares: bad }, memberPrincipal(1)), HumanContractError);
});

test("un changement d’appartenance est un POST fermé dans le contexte choisi, lié à son reçu", async () => {
  const { api, requests } = client((path, options, request) => json(200, membershipReceipt(request.body.operation_id, membershipId)));
  try {
    await api.identity(); api.selectContext(adminId);
    const operation = newId();
    const receipt = await api.changeMembership(membershipId, { operation_id: operation, expected_revision: "3", capabilities: ["read_metadata"], active: true });
    assert.equal(receipt.audit_seq, "250");
    const call = requests.at(-1);
    assert.equal(call.path, `/api/v1/human/admin/memberships/${membershipId}`);
    assert.equal(call.options.method, "POST"); assert.equal(call.options.headers["Content-Type"], "application/json");
    assert.equal(call.options.headers["X-AiGuard-Context"], adminId);
    assert.equal(call.options.credentials, "omit"); assert.equal(call.options.cache, "no-store"); assert.equal(call.options.redirect, "error");
    assert.deepEqual(call.body, { operation_id: operation, expected_revision: "3", capabilities: ["read_metadata"], active: true });
  } finally { api.close(); }
});

test("le grant est lu puis remplacé par un POST fermé ; un reçu d’une autre ressource est une rupture de contrat", async () => {
  let foreign = false;
  const { api, requests } = client((path, options, request) => options.method === "GET" ? json(200, grantBody(adminId)) :
    json(200, grantReceipt(request.body.operation_id, foreign ? { kind: "session", resource_id: "other" } : resource)));
  try {
    await api.identity(); api.selectContext(adminId);
    const grant = await api.grant(resource);
    assert.equal(requests.at(-1).path, "/api/v1/human/admin/grants?resource_kind=session&resource_id=session-a");
    assert.equal(requests.at(-1).options.method, "GET");
    const change = { operation_id: newId(), resource, expected_revision: grant.revision, shares: [{ principal: memberPrincipal(2), permissions: ["metadata", "content"] }] };
    assert.equal((await api.changeGrant(change, grant.owner)).grant_revision, "5");
    assert.equal(requests.at(-1).path, "/api/v1/human/admin/grants"); assert.equal(requests.at(-1).options.method, "POST");
    assert.deepEqual(requests.at(-1).body, { operation_id: change.operation_id, resource, expected_revision: "4", shares: [{ principal: memberPrincipal(2), permissions: ["metadata", "content"] }] });
    foreign = true;
    await assert.rejects(api.changeGrant({ ...change, operation_id: newId() }, grant.owner), HumanContractError);
  } finally { api.close(); }
});

test("aucun POST n’est émis quand le catalogue ne sert pas l’opération de changement", async () => {
  for (const [operation, availability] of [["memberships_manage", "not_in_this_build"], ["grants_manage", "not_in_this_build"], ["grants_manage", "migration_required"]]) {
    const { api, requests } = client(() => json(200, {}), () => offered(offer({ [operation]: ["granted", "tenant", availability] })));
    try {
      await api.identity(); api.selectContext(adminId); const before = requests.length;
      const attempt = operation === "memberships_manage" ?
        api.changeMembership(membershipId, { operation_id: newId(), expected_revision: "3", capabilities: [], active: false }) :
        api.changeGrant({ operation_id: newId(), resource, expected_revision: "4", shares: [] });
      await assert.rejects(attempt, (error) => error instanceof HumanApiError && error.status === 503 && error.code === availability);
      if (operation === "grants_manage") await assert.rejects(api.grant(resource), (error) => error.code === availability);
      assert.equal(requests.length, before, `${operation} ${availability}: nothing is sent`);
    } finally { api.close(); }
  }
});

/** One change through the real client and a scripted sequence of control answers. */
function scripted(answers) {
  const hooks = { applied: [], conflict: 0, refused: 0, missing: 0 };
  const { api, requests, notifications } = client((path, options, request) => {
    const next = answers.shift();
    if (next === "network") throw new TypeError("network down");
    return typeof next === "function" ? next(request) : next;
  });
  const flow = new ChangeFlow({ applied: (receipt) => hooks.applied.push(receipt), conflict: () => { hooks.conflict++; }, refused: () => { hooks.refused++; }, missing: () => { hooks.missing++; } }, newId);
  const send = (capabilities) => (operationId) => api.changeMembership(membershipId, { operation_id: operationId, expected_revision: "3", capabilities, active: true });
  const posted = () => requests.filter((request) => request.options.method === "POST").map((request) => request.body.operation_id);
  return { api, flow, hooks, send, posted, notifications };
}
const ok = (request) => json(200, membershipReceipt(request.body.operation_id, membershipId));

test("un 503, une panne réseau ou un reçu illisible laissent l’issue inconnue et la reprise garde la même opération", async () => {
  const { api, flow, hooks, send, posted } = scripted([refusal(503, "audit_unavailable", "retry"), "network",
    json(200, { version: 1 }), ok, ok]);
  try {
    await api.identity(); api.selectContext(adminId);
    assert.equal(await flow.submit("change-1", send(["read_metadata"])), "unknown");
    assert.equal(await flow.submit("change-1", send(["read_metadata"])), "unknown");
    assert.equal(await flow.submit("change-1", send(["read_metadata"])), "unknown", "an unreadable 200 does not prove that nothing was applied");
    assert.equal(await flow.submit("change-1", send(["read_metadata"])), "applied");
    const ids = posted(); assert.equal(new Set(ids).size, 1, "every attempt of the same change carries the same operation");
    assert.equal(hooks.applied.length, 1); assert.equal(hooks.applied[0].operation_id, ids[0]);
    // After a definitive answer the next change, even identical, is a new operation.
    assert.equal(await flow.submit("change-1", send(["read_metadata"])), "applied");
    assert.notEqual(posted().at(-1), ids[0]);
  } finally { api.close(); }
});

test("un 409 dit que le contenu a changé, le fait relire, et la tentative suivante est une nouvelle opération", async () => {
  const { api, flow, hooks, send, posted } = scripted([refusal(409, "conflict", "reread"), ok]);
  try {
    await api.identity(); api.selectContext(adminId);
    assert.equal(await flow.submit("change-2", send(["read_metadata"])), "conflict");
    assert.equal(hooks.conflict, 1, "the caller reads the membership or the grant again");
    assert.equal(flow.pendingOperation, null);
    assert.equal(await flow.submit("change-2", send(["read_metadata"])), "applied");
    const [first, second] = posted(); assert.notEqual(first, second);
  } finally { api.close(); }
});

test("un 403 retire l’éditeur et fait relire les accès ; 400 et 404 sont définitifs", async () => {
  const { api, flow, hooks, send, posted } = scripted([refusal(403, "permission_denied", "review_permissions"), refusal(400, "invalid_request", "retry"),
    refusal(404, "not_found", "retry")]);
  try {
    await api.identity(); api.selectContext(adminId);
    assert.equal(await flow.submit("change-3", send(["read_metadata"])), "refused"); assert.equal(hooks.refused, 1);
    assert.equal(await flow.submit("change-3", send(["read_metadata"])), "invalid");
    assert.equal(await flow.submit("change-3", send(["read_metadata"])), "not_found"); assert.equal(hooks.missing, 1);
    assert.equal(new Set(posted()).size, 3, "each definitive answer retires its operation");
  } finally { api.close(); }
});

test("un 403 context_revoked retire le contexte avant même que l’éditeur réagisse", async () => {
  const { api, flow, hooks, send, notifications } = scripted([refusal(403, "context_revoked", "select_context")]);
  try {
    await api.identity(); api.selectContext(adminId);
    assert.equal(await flow.submit("change-4", send([])), "refused");
    assert.deepEqual(notifications, [["context", "context_revoked"]]); assert.equal(api.context, null); assert.equal(hooks.refused, 1);
  } finally { api.close(); }
});

test("une autre intention reçoit sa propre opération, même pendant une issue inconnue", async () => {
  const { api, flow, send, posted } = scripted([refusal(503, "audit_unavailable", "retry"), ok]);
  try {
    await api.identity(); api.selectContext(adminId);
    assert.equal(await flow.submit("change-5a", send(["read_metadata"])), "unknown");
    assert.equal(await flow.submit("change-5b", send([])), "applied");
    const [first, second] = posted(); assert.notEqual(first, second, "a different change never reuses the operation of another");
  } finally { api.close(); }
});
