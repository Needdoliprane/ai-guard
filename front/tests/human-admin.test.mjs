// Administration reads of the named dashboard: the client methods, the destination states and the purge on context
// change. Real HumanApiClient over a recorded fetch; pure decisions. No DOM.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError, HumanSuperseded } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { ADMIN_DESTINATIONS, adminDestinationState, combinedDestinationState } from "../src/access-model.ts";
import { catalogue, context, identity, tenantScope } from "./human-fixtures.mjs";
import { ownedCatalogue, ownedReadProfiles } from "./owned-brokers-fixture.mjs";
import { configurationBody, membershipPage, quotaBody, usagePage } from "./admin-fixtures.mjs";

const adminId = "c".repeat(64);
const served = ["granted", "tenant", "available"];
const adminOffer = (overrides = {}) => catalogue({ summary_read: served, memberships_read: served, quota_read: served, usage_read: served,
  configuration_read: served, brokers_read: tenantScope, ...overrides });
const admin = (operations = adminOffer()) => ({ ...context(adminId, "administration"), capabilities: ["read_memberships", "read_configuration", "read_worker_registry"], operations });
const offered = (operations) => ({ ...identity(), contexts: [admin(operations)] });

function client(handler, identityBody = () => offered()) {
  const notifications = []; const requests = [];
  const api = new HumanApiClient("private-human-bearer-123456789", {
    onAuthentication: (error) => notifications.push(["authentication", error.code]),
    onContextInvalidated: (error) => notifications.push(["context", error.code]),
  }, async (path, options) => { requests.push({ path, options }); return path.endsWith("/identity") ? Response.json(identityBody()) : handler(path, options); });
  return { api, notifications, requests };
}
const reads = [
  { name: "memberships", path: "/api/v1/human/admin/memberships?limit=50", invoke: (api) => api.memberships(null), body: membershipPage, operation: "memberships_read" },
  { name: "memberships/cursor", path: `/api/v1/human/admin/memberships?limit=50&cursor=${"e".repeat(32)}`, invoke: (api) => api.memberships("e".repeat(32)), body: membershipPage, operation: "memberships_read" },
  { name: "quota", path: "/api/v1/human/admin/quota", invoke: (api) => api.quota(), body: quotaBody, operation: "quota_read" },
  { name: "usage", path: "/api/v1/human/admin/usage?limit=50", invoke: (api) => api.usage(null), body: usagePage, operation: "usage_read" },
  { name: "configuration", path: "/api/v1/human/admin/configuration", invoke: (api) => api.configuration(), body: configurationBody, operation: "configuration_read" },
];

for (const read of reads) {
  test(`admin ${read.name} : lecture du contexte choisi, sans cookie ni cache, liée à ce contexte`, async () => {
    let foreign = false;
    const { api, requests } = client(() => Response.json(read.body(foreign ? "d".repeat(64) : adminId)));
    try {
      await api.identity(); api.selectContext(adminId);
      const value = await read.invoke(api);
      assert.ok(value.read_receipt, "the parsed value keeps its read receipt");
      const call = requests.at(-1);
      assert.equal(call.path, read.path);
      assert.equal(call.options.headers["X-AiGuard-Context"], adminId);
      assert.equal(call.options.headers.Authorization, "Bearer private-human-bearer-123456789");
      assert.equal(call.options.credentials, "omit"); assert.equal(call.options.cache, "no-store");
      // An answer for another context is a contract break, never a value of this one.
      foreign = true;
      await assert.rejects(read.invoke(api), HumanContractError);
    } finally { api.close(); }
  });

  test(`admin ${read.name} : une lecture que le catalogue n’annonce pas servie n’est jamais émise`, async () => {
    for (const [availability, code] of [["migration_required", "migration_required"], ["profile_unavailable", "read_profile_unavailable"]]) {
      const { api, requests } = client(() => Response.json(read.body(adminId)), () => offered(adminOffer({ [read.operation]: ["granted", "tenant", availability] })));
      try {
        await api.identity(); api.selectContext(adminId); const before = requests.length;
        await assert.rejects(read.invoke(api), (error) => error instanceof HumanApiError && error.status === 503 && error.code === code);
        assert.equal(requests.length, before, `${availability}: no request is sent`);
      } finally { api.close(); }
    }
  });
}

test("admin : un refus du contrôle reste un 403 lisible par la vue, qui retire ses valeurs et fait relire les accès", async () => {
  const { api } = client(() => Response.json({ error: { code: "permission_denied", message: "Refus", retry_action: "review_permissions" } }, { status: 403 }));
  try {
    await api.identity(); api.selectContext(adminId);
    for (const read of reads) await assert.rejects(read.invoke(api), (error) => error instanceof HumanApiError && error.status === 403, read.name);
  } finally { api.close(); }
});

test("admin : une lecture en vol est abandonnée quand le contexte change, et sa réponse tardive n’est jamais publiée", async () => {
  let release; const held = new Promise((resolve) => { release = resolve; });
  let operations = adminOffer();
  const { api, notifications } = client(async () => { await held; return Response.json(membershipPage(adminId)); }, () => offered(operations));
  try {
    await api.identity(); api.selectContext(adminId);
    const pending = api.memberships(null);
    // The operator installs a change of offer: same capabilities, another catalogue. The context is withdrawn.
    operations = adminOffer({ configuration_read: ["not_granted"] }); await api.identity();
    assert.equal(api.context, null); assert.deepEqual(notifications, [["context", "context_revoked"]]);
    release();
    await assert.rejects(pending, (error) => error instanceof HumanSuperseded || error?.name === "AbortError");
    await assert.rejects(api.memberships(null), (error) => error instanceof HumanApiError && error.code === "context_required");
  } finally { release(); api.close(); }
});

test("les trois destinations d’administration suivent leurs opérations : lecture, explication ou rien", () => {
  assert.deepEqual(Object.keys(ADMIN_DESTINATIONS), ["memberships", "usage", "configuration"]);
  const state = (operations, destination) => adminDestinationState(admin(operations), destination);
  for (const destination of ["memberships", "usage", "configuration"]) {
    assert.equal(state(adminOffer(), destination), "view", destination);
    assert.equal(state(catalogue(), destination), null, `${destination}: not granted, no destination`);
    // The older owned catalogue serves memberships and configuration while quota and usage are unavailable.
    assert.equal(state(ownedCatalogue(adminOffer()), destination), destination === "usage" ? "explanation" : "view",
      `${destination}: owned profile`);
    assert.equal(state(catalogue({ memberships_read: ["granted", "tenant", "migration_required"], quota_read: ["granted", "tenant", "migration_required"],
      usage_read: ["granted", "tenant", "migration_required"], configuration_read: ["granted", "tenant", "migration_required"] }), destination), "explanation", `${destination}: marker missing`);
    assert.equal(state(catalogue(Object.fromEntries(ADMIN_DESTINATIONS[destination].map((operation) => [operation, ["granted", "tenant", "not_in_this_build"]]))), destination), null,
      `${destination}: no route in this build, no destination`);
  }
  // Quotas et usage can open with quota alone. The usage page still requires its own offered and served operation.
  assert.equal(state(adminOffer({ usage_read: ["granted", "tenant", "migration_required"] }), "usage"), "view");
  assert.equal(state(catalogue({ quota_read: served }), "usage"), "view");
  assert.equal(state(catalogue({ usage_read: served }), "usage"), "explanation");
  assert.equal(combinedDestinationState(admin(adminOffer()), []), null);
  // A personal context of the same membership never offers them: the administration context does.
  const personal = { ...context(adminId, "personal"), operations: catalogue({ memberships_read: ["requires_administration"], quota_read: ["requires_administration"],
    usage_read: ["requires_administration"], configuration_read: ["requires_administration"] }) };
  for (const destination of ["memberships", "usage", "configuration"]) assert.equal(adminDestinationState(personal, destination), null);
});

test("le profil possédé est refusé localement pour les registres, d’après le catalogue de chaque contexte", async () => {
  const { api, requests } = client(() => Response.json({}), () => offered(ownedCatalogue(adminOffer({ workers_read: tenantScope, federation_read: tenantScope }))));
  try {
    await api.identity(); api.selectContext(adminId); const before = requests.length;
    for (const read of [() => api.workers(), () => api.federationWorkers(), () => api.quota(), () => api.usage(null)])
      await assert.rejects(read(), (error) => error.code === "read_profile_unavailable");
    assert.equal(requests.length, before, "no historical route is tried");
  } finally { api.close(); }
});

test("quota owned seul : reçu, usage indisponible et retrait du contexte après révocation", async () => {
  let operations = ownedCatalogue(adminOffer({ workers_read: tenantScope })).map((entry) =>
    ["workers_read", "quota_read"].includes(entry.operation) ? { ...entry, availability: "available" } : entry);
  const read_profiles = { ...ownedReadProfiles(), worker_registry: "human_worker_registry_owned_v1" };
  const quota = quotaBody(adminId);
  let hold = false;
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const { api, notifications, requests } = client(async (path) => {
    assert.equal(path, "/api/v1/human/admin/quota");
    if (hold) await held;
    return Response.json(quota);
  }, () => ({ ...offered(operations), read_profiles }));

  try {
    await api.identity();
    assert.equal(adminDestinationState(api.selectContext(adminId), "usage"), "view");
    const read = await api.quota();
    assert.equal(read.source, quota.source);
    assert.equal(read.evaluated_at, quota.evaluated_at);
    assert.deepEqual(read.quota, quota.quota);
    assert.deepEqual(read.read_receipt, quota.read_receipt);
    await assert.rejects(api.usage(null), (error) => error.code === "read_profile_unavailable");
    assert.deepEqual(requests.map(({ path }) => path), ["/api/v1/human/identity", "/api/v1/human/admin/quota"]);
    assert.equal(requests[1].options.headers["X-AiGuard-Context"], adminId);
    await api.identity();
    assert.equal(api.context?.id, adminId);
    assert.deepEqual(notifications, []);

    hold = true;
    const pending = api.quota();
    const signal = requests.at(-1).options.signal;
    operations = operations.map((entry) => entry.operation === "quota_read" ?
      { operation: "quota_read", permission: "not_granted", availability: "available" } : entry);
    await api.identity();
    assert.equal(api.context, null);
    assert.deepEqual(notifications, [["context", "context_revoked"]]);
    assert.equal(signal.aborted, true);
    release();
    await assert.rejects(pending, HumanSuperseded);
  } finally { release(); api.close(); }
});
