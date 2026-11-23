import assert from "node:assert/strict";
import { test } from "node:test";
import { adminUrls } from "../src/admin-model.ts";
import { UsageView } from "../src/admin-views.ts";
import { HumanApiClient } from "../src/human-api.ts";
import { formatDate } from "../src/model.ts";
import { quotaBody, usagePage } from "./admin-fixtures.mjs";
import { catalogue, context, errorResponse, identity, secondUuid, uuid } from "./human-fixtures.mjs";
import { ownedReadProfiles } from "./owned-brokers-fixture.mjs";

// Only the node operations used by UsageView. No browser or network is started.
class Element extends EventTarget {
  #attributes = new Map();
  className = "";
  dataset = {};
  children = [];
  constructor(tag, text = "") { super(); this.tag = tag; this.text = text; }
  get textContent() { return this.text + this.children.map((node) => node.textContent).join(""); }
  set textContent(text) { this.text = text; this.children = []; }
  append(...nodes) { this.children.push(...nodes.map((node) => typeof node === "string" ? new Element("#text", node) : node)); }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(name, value) { this.#attributes.set(name, value); }
  getAttribute(name) { return this.#attributes.get(name) ?? null; }
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

async function mount(t, replies, { owned = true, usageAccess = () => served, offeredId = () => adminId } = {}) {
  const globals = { Node: Element, document: { createElement: (tag) => new Element(tag), createTextNode: (text) => new Element("#text", text) } };
  for (const [name, value] of Object.entries(globals)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => original ? Object.defineProperty(globalThis, name, original) : Reflect.deleteProperty(globalThis, name));
  }
  const calls = [];
  const invalidations = [];
  let doubts = 0;
  let identityReads = 0;
  let view;
  const api = new HumanApiClient("synthetic-admin-token", {
    onAuthentication: () => view.close(),
    onContextInvalidated: ({ status, code }) => { view.close(); invalidations.push({ status, code }); },
  }, async (path, { signal, headers }) => {
    if (path === "/api/v1/human/identity") {
      identityReads++;
      const operations = catalogue({ quota_read: served, usage_read: usageAccess(),
        ...(owned ? { federation_read: ["not_granted", null, "profile_unavailable"] } : {}) });
      return Response.json({ ...identity(), contexts: [{ ...context(offeredId(), "administration"), operations }],
        ...(owned ? { read_profiles: { ...ownedReadProfiles(), worker_registry: "human_worker_registry_owned_v1" } } : {}) });
    }
    assert.equal(headers["X-AiGuard-Context"], api.context?.id);
    calls.push({ path, signal });
    return replies[path]();
  });
  t.after(() => { view?.close(); api.close(); });
  const openView = (id) => {
    view = new UsageView({ context: api.selectContext(id), owned: api.readProfiles !== undefined,
      quota: (signal) => api.quota(signal), usage: (cursor, signal) => api.usage(cursor, signal),
      budgetStopped: () => api.readBudgetStopped, onReadStateChange: () => {}, onAccessDoubt: () => { doubts++; } });
    return view;
  };
  await api.identity();
  return { view: openView(adminId), api, calls, openView, invalidations, doubts: () => doubts, identityReads: () => identityReads };
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

test("owned : un 429 temporaire du quota laisse lire l’usage autorisé", async (t) => {
  const { view, api, calls } = await mount(t, {
    [quotaPath]: () => errorResponse(429, "capacity_exhausted"),
    [usagePath]: () => Response.json(initialUsage),
  });
  await view.refresh();
  assert.equal(api.readBudgetStopped, false);
  assert.deepEqual(paths(calls), [quotaPath, usagePath]);
  assert.equal(view.element.dataset.state, "partial");
  const [quota, usage] = panels(view);
  assert.equal(quota.dataset.state, "error");
  assert.equal(usage.dataset.state, "ready");
  assertStamp(usage, initialUsage);
});

for (const path of [quotaPath, usagePath]) {
  test(`Les en-têtes 403 sur ${path === quotaPath ? "le quota" : "l’usage"} owned retirent le contexte et les données`, async (t) => {
    const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
    const { view, api, calls, invalidations, doubts } = await mount(t, replies);
    await view.refresh();
    calls.length = 0;
    const cancellation = Promise.withResolvers();
    let controller;
    let pulled = false;
    let cancelled = false;
    replies[path] = () => new Response(new ReadableStream({
      start(stream) { controller = stream; },
      pull() { pulled = true; },
      cancel() { cancelled = true; return cancellation.promise; },
    }, { highWaterMark: 0 }), { status: 403 });
    const pending = view.refresh();
    try {
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(api.context, null);
      assert.throws(() => api.selectContext(adminId), { code: "context_invalid" });
      assert.equal(view.element.children.length, 0);
      assert.deepEqual(invalidations, [{ status: 403, code: "permission_denied" }]);
      assert.equal(doubts(), 0);
      assert.equal(pulled, false);
      assert.equal(cancelled, true);
      assert.deepEqual(paths(calls), path === quotaPath ? [quotaPath] : [quotaPath, usagePath]);
    } finally {
      if (!cancelled) controller?.close();
      cancellation.resolve();
      await pending;
    }
  });
}

test("Un 403 à l’essai opérateur retire l’offre et laisse le budget suspendu jusqu’à une identité fraîche", async (t) => {
  let offered = adminId;
  const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
  const { view, api, calls, invalidations, openView, identityReads } = await mount(t, replies, { offeredId: () => offered });
  await view.refresh();
  replies[quotaPath] = () => errorResponse(429, "audit_read_budget_exhausted", "contact_operator");
  await view.refresh();
  assert.equal(api.readBudgetStopped, true);
  assertStamp(panels(view)[0], initialQuota);

  calls.length = 0;
  replies[quotaPath] = () => errorResponse(403, "context_revoked", "select_context");
  api.allowReadAfterOperatorAction();
  await view.refresh();
  assert.equal(api.readBudgetStopped, true);
  assert.deepEqual(paths(calls), [quotaPath]);
  assert.deepEqual(invalidations, [{ status: 403, code: "permission_denied" }]);
  assert.equal(api.context, null);
  assert.equal(view.element.children.length, 0);
  assert.throws(() => api.selectContext(adminId), { code: "context_invalid" });
  await assert.rejects(api.identity(), { code: "audit_read_budget_exhausted" });
  assert.equal(identityReads(), 1);

  offered = "d".repeat(64);
  api.allowReadAfterOperatorAction();
  const renewed = await api.identity();
  assert.deepEqual(renewed.contexts.map(({ id }) => id), [offered]);
  assert.equal(identityReads(), 2);
  assert.equal(api.readBudgetStopped, false);
  assert.equal(api.context, null);
  assert.throws(() => api.selectContext(adminId), { code: "context_invalid" });
  const newQuota = quotaBody(offered, stamp(7));
  const newUsage = usagePage(offered, stamp(8));
  replies[quotaPath] = () => Response.json(newQuota);
  replies[usagePath] = () => Response.json(newUsage);
  const renewedView = openView(offered);
  await renewedView.refresh();
  assert.equal(renewedView.element.dataset.state, "ready");
  assertStamp(panels(renewedView)[0], newQuota);
  assertStamp(panels(renewedView)[1], newUsage);
});

test("owned : un HTTP 401 sur l’usage retire les deux lectures", async (t) => {
  const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
  const { view, calls } = await mount(t, replies);
  await view.refresh();
  calls.length = 0;
  replies[usagePath] = () => errorResponse(401, "authentication_required");
  await view.refresh();
  assert.deepEqual(paths(calls), [quotaPath, usagePath]);
  assert.equal(view.element.children.length, 0);
});

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

test("owned : une requête de quota annulée efface les lectures et permet de reprendre dans le contexte actif", async (t) => {
  const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json(initialUsage) };
  const { view, api, calls } = await mount(t, replies);
  await view.refresh();
  calls.length = 0;
  replies[quotaPath] = () => { throw new DOMException("Lecture annulée.", "AbortError"); };
  await view.refresh();
  assert.equal(view.element.dataset.state, "error");
  assert.equal(panels(view).length, 0);
  assert.match(view.element.textContent, /Lecture annulée/);
  assert.equal(api.context?.id, adminId);
  assert.deepEqual(paths(calls), [quotaPath]);

  const nextQuota = quotaBody(adminId, stamp(5));
  replies[quotaPath] = () => Response.json(nextQuota);
  await view.refresh();
  assert.equal(view.element.dataset.state, "ready");
  assert.deepEqual(paths(calls), [quotaPath, quotaPath, usagePath]);
  assertStamp(panels(view)[0], nextQuota);
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

test("owned : une durée sous la seconde diffère d’un zéro observé et une page vide signale la durée inconnue", async (t) => {
  const allocation = {
    worker_id: uuid, started_at: "2026-11-21T11:00:00Z", method_version: 1, billable: false,
    source: { authority_id: "authority-a", runtime_kind: "docker", container_id: "container-1", spec_hash: "f".repeat(64), provenance: "observed" },
  };
  const items = [
    { ...allocation, attempt_id: uuid, terminal_observation_id: "observation-short", finished_at: "2026-11-21T11:00:00.5Z",
      duration_ns: "500000000", cpu_millicore_nanoseconds: "500000000000", memory_byte_nanoseconds: "512000000000" },
    { ...allocation, attempt_id: secondUuid, terminal_observation_id: "observation-zero", finished_at: allocation.started_at,
      duration_ns: "0", cpu_millicore_nanoseconds: "0", memory_byte_nanoseconds: "0" },
  ];
  const replies = { [quotaPath]: () => Response.json(initialQuota), [usagePath]: () => Response.json({ ...initialUsage, items }) };
  const { view } = await mount(t, replies);
  await view.refresh();
  const table = panels(view)[1].children.find((child) => child.className === "ov-scroll").children[0];
  const [short, zero] = table.children.find((child) => child.tag === "tbody").children.map((row) => row.children[2]);
  assert.equal(short.children[0].textContent, "< 1 s");
  assert.equal(short.children[0].getAttribute("aria-hidden"), "true");
  assert.equal(short.children.find((child) => child.className === "sr-only").textContent, "Moins d’une seconde, 500\u202f000\u202f000 ns.");
  assert.equal(zero.textContent, "0 s");

  replies[usagePath] = () => Response.json(initialUsage);
  await view.refresh();
  assert.match(panels(view)[1].textContent, /durée reste inconnue en l’absence d’une observation terminale complète/);
});

test("owned : Allocations suivantes attend la fin de l’actualisation avant de changer de page", async (t) => {
  const cursor = "ab".repeat(16);
  const nextPath = adminUrls.usage(cursor);
  const nextUsage = usagePage(adminId, stamp(6));
  const replies = {
    [quotaPath]: () => Response.json(initialQuota),
    [usagePath]: () => Response.json({ ...initialUsage, has_more: true, next_cursor: cursor }),
    [nextPath]: () => Response.json(nextUsage),
  };
  const { view, calls } = await mount(t, replies);
  const nextButton = () => panels(view)[1].children.find((child) => child.tag === "button" && child.textContent === "Allocations suivantes");
  await view.refresh();
  calls.length = 0;
  const held = Promise.withResolvers();
  replies[quotaPath] = () => held.promise;
  const pending = view.refresh();
  try {
    const next = nextButton();
    assert.equal(next.disabled, true);
    next.dispatchEvent(new Event("click"));
  } finally {
    replies[quotaPath] = () => Response.json(initialQuota);
    held.resolve(Response.json(initialQuota));
  }
  await pending;
  await view.refresh();
  assert.deepEqual(paths(calls), [quotaPath, usagePath, quotaPath, usagePath]);

  const next = nextButton();
  assert.equal(next.disabled, false);
  next.dispatchEvent(new Event("click"));
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(paths(calls).slice(-2), [quotaPath, nextPath]);
  assertStamp(panels(view)[1], nextUsage);
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

test("Legacy conserve le motif structuré d’un refus sur le quota", async (t) => {
  const { api } = await mount(t, { [quotaPath]: () => errorResponse(403, "context_revoked", "select_context") }, { owned: false });
  await assert.rejects(api.quota(), { status: 403, code: "context_revoked" });
  assert.equal(api.context, null);
});
