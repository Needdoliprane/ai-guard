import assert from "node:assert/strict";
import { test } from "node:test";
import { adminUrls } from "../src/admin-model.ts";
import { UsageView } from "../src/admin-views.ts";
import { HumanApiClient } from "../src/human-api.ts";
import { formatDate } from "../src/model.ts";
import { quotaBody, usagePage } from "./admin-fixtures.mjs";
import { catalogue, context, errorResponse, identity, uuid } from "./human-fixtures.mjs";
import { ownedReadProfiles } from "./owned-brokers-fixture.mjs";

// Only the node operations used by UsageView. No browser or network is started.
class Element {
  className = "";
  dataset = {};
  children = [];
  constructor(tag, text = "") { this.tag = tag; this.text = text; }
  get textContent() { return this.text + this.children.map((node) => node.textContent).join(""); }
  set textContent(text) { this.text = text; this.children = []; }
  append(...nodes) { this.children.push(...nodes.map((node) => typeof node === "string" ? new Element("#text", node) : node)); }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute() {}
  addEventListener() {}
}

const adminId = "c".repeat(64);
const served = ["granted", "tenant", "available"];
const quotaPath = adminUrls.quota();
const usagePath = adminUrls.usage(null);
const stamp = (number) => {
  const evaluated_at = `2026-11-21T12:00:0${number}Z`;
  return { evaluated_at, read_receipt: { request_id: `1000000${number}-0000-4000-8000-000000000000`, audit_event_id: uuid, received_at: evaluated_at } };
};
const initialQuota = quotaBody(adminId, stamp(1));
const initialUsage = usagePage(adminId, stamp(2));

async function mount(t, replies, { owned = true, usageAccess = () => served } = {}) {
  const globals = { Node: Element, document: { createElement: (tag) => new Element(tag), createTextNode: (text) => new Element("#text", text) } };
  for (const [name, value] of Object.entries(globals)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => original ? Object.defineProperty(globalThis, name, original) : Reflect.deleteProperty(globalThis, name));
  }
  const calls = [];
  let doubts = 0;
  let view;
  const api = new HumanApiClient("synthetic-admin-token", {
    onAuthentication: () => view.close(),
    onContextInvalidated: () => view.close(),
  }, async (path, { signal, headers }) => {
    if (path === "/api/v1/human/identity") {
      const operations = catalogue({ quota_read: served, usage_read: usageAccess(),
        ...(owned ? { federation_read: ["not_granted", null, "profile_unavailable"] } : {}) });
      return Response.json({ ...identity(), contexts: [{ ...context(adminId, "administration"), operations }],
        ...(owned ? { read_profiles: { ...ownedReadProfiles(), worker_registry: "human_worker_registry_owned_v1" } } : {}) });
    }
    assert.equal(headers["X-AiGuard-Context"], adminId);
    calls.push({ path, signal });
    return replies[path]();
  });
  t.after(() => { view?.close(); api.close(); });
  await api.identity();
  view = new UsageView({ context: api.selectContext(adminId), owned: api.readProfiles !== undefined,
    quota: (signal) => api.quota(signal), usage: (cursor, signal) => api.usage(cursor, signal),
    budgetStopped: () => api.readBudgetStopped, onReadStateChange: () => {}, onAccessDoubt: () => { doubts++; } });
  return { view, api, calls, doubts: () => doubts };
}

const paths = (calls) => calls.map(({ path }) => path);
function panels(view) { return view.element.children.find((child) => child.className === "ov-body").children; }
function assertStamp(panel, value) {
  const provenance = panel.children.find((child) => child.className === "ov-provenance");
  assert.equal(provenance.children.find((child) => child.tag === "time").textContent, formatDate(value.evaluated_at, true));
  assert.ok(provenance.textContent.endsWith(value.read_receipt.request_id.slice(0, 8)));
}

test("owned : le quota précède l’usage et chaque section garde la date et le reçu de sa propre lecture", async (t) => {
  const held = Promise.withResolvers();
  const replies = { [quotaPath]: () => held.promise, [usagePath]: () => Response.json(initialUsage) };
  const { view, calls } = await mount(t, replies);
  const first = view.refresh();
  try { assert.deepEqual(paths(calls), [quotaPath]); }
  finally { held.resolve(Response.json(initialQuota)); }
  await first;
  assert.deepEqual(paths(calls), [quotaPath, usagePath]);
  assert.equal(view.element.dataset.state, "ready");
  assertStamp(panels(view)[0], initialQuota);
  assertStamp(panels(view)[1], initialUsage);

  const nextQuota = quotaBody(adminId, stamp(3));
  replies[quotaPath] = () => Response.json(nextQuota);
  replies[usagePath] = () => errorResponse(503, "audit_unavailable");
  await view.refresh();
  assert.equal(view.element.dataset.state, "partial");
  const [quota, usage] = panels(view);
  assert.equal(quota.dataset.state, "ready");
  assert.equal(usage.dataset.state, "stale");
  assertStamp(quota, nextQuota);
  assertStamp(usage, initialUsage);

  const nextUsage = usagePage(adminId, stamp(4));
  replies[quotaPath] = () => errorResponse(503, "audit_unavailable");
  replies[usagePath] = () => Response.json(nextUsage);
  await view.refresh();
  assert.equal(panels(view)[0].dataset.state, "stale");
  assert.equal(panels(view)[1].dataset.state, "ready");
  assertStamp(panels(view)[0], nextQuota);
  assertStamp(panels(view)[1], nextUsage);
});

test("owned : le quota arrêté par le budget suspend l’usage sans afficher de valeurs", async (t) => {
  const { view, api, calls } = await mount(t, {
    [quotaPath]: () => errorResponse(429, "audit_read_budget_exhausted", "contact_operator"),
  });
  await view.refresh();
  assert.equal(api.readBudgetStopped, true);
  assert.deepEqual(paths(calls), [quotaPath]);
  const [quota, usage] = panels(view);
  assert.equal(quota.dataset.state, "error");
  assert.equal(usage.dataset.state, "error");
  assert.match(usage.textContent, /limite de lecture/);
  assert.doesNotMatch(view.element.textContent, /Aucune allocation|workers réservés|reçu de lecture/);
});

for (const [path, status] of [[quotaPath, 403], [usagePath, 403], [usagePath, 401]]) {
  test(`owned : HTTP ${status} sur ${path === quotaPath ? "le quota" : "l’usage"} retire les deux lectures`, async (t) => {
    const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
    const { view, calls, doubts } = await mount(t, replies);
    await view.refresh();
    calls.length = 0;
    replies[path] = () => errorResponse(status, status === 401 ? "authentication_required" : "permission_denied");
    await view.refresh();
    assert.deepEqual(paths(calls), path === quotaPath ? [quotaPath] : [quotaPath, usagePath]);
    if (status === 401) assert.equal(view.element.children.length, 0);
    else {
      assert.equal(view.element.dataset.state, "refused");
      assert.equal(panels(view).length, 0);
      assert.equal(doubts(), 1);
    }
  });
}

test("owned : retirer le droit d’usage annule le quota en cours et écarte sa réponse tardive", async (t) => {
  let access = served;
  const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
  const { view, api, calls } = await mount(t, replies, { usageAccess: () => access });
  await view.refresh();
  calls.length = 0;
  const held = Promise.withResolvers();
  replies[quotaPath] = () => held.promise;
  const pending = view.refresh();
  access = ["not_granted", null, "available"];
  try {
    await api.identity();
    assert.equal(api.context, null);
    assert.equal(calls[0].signal.aborted, true);
    assert.equal(view.element.children.length, 0);
  } finally { held.resolve(Response.json(initialQuota)); }
  await pending;
  assert.equal(view.element.children.length, 0);
  assert.deepEqual(paths(calls), [quotaPath]);
});

test("owned : une requête de quota annulée efface les lectures précédentes", async (t) => {
  const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
  const { view, calls } = await mount(t, replies);
  await view.refresh();
  calls.length = 0;
  replies[quotaPath] = () => { throw new DOMException("Lecture annulée.", "AbortError"); };
  await view.refresh();
  assert.equal(view.element.children.length, 0);
  assert.deepEqual(paths(calls), [quotaPath]);
});

test("owned : quota seul conserve un reçu unique et explique l’absence d’usage", async (t) => {
  const { view, calls } = await mount(t, { [quotaPath]: () => Response.json(initialQuota) },
    { usageAccess: () => ["granted", "tenant", "profile_unavailable"] });
  await view.refresh();
  assert.deepEqual(paths(calls), [quotaPath]);
  assert.equal(view.element.dataset.state, "ready");
  assertStamp(view.element.children.find((child) => child.className === "ov-status"), initialQuota);
  assert.match(panels(view)[1].textContent, /profil de sources/);
});

test("Legacy conserve ses deux lectures parallèles", async (t) => {
  const held = Promise.withResolvers();
  const { view, calls } = await mount(t, { [quotaPath]: () => held.promise, [usagePath]: () => Response.json(initialUsage) }, { owned: false });
  const pending = view.refresh();
  try { assert.deepEqual(paths(calls), [quotaPath, usagePath]); }
  finally { held.resolve(Response.json(initialQuota)); }
  await pending;
  assert.equal(view.element.dataset.state, "ready");
  assertStamp(view.element.children.find((child) => child.className === "ov-status"), initialUsage);
});
