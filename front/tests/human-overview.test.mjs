// Integration of the per-context summary in the named dashboard: destination, landing and the client read.
// Pure decisions and the real HumanApiClient over a recorded fetch. No DOM.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError } from "../src/human-api.ts";
import { HumanContractError } from "../src/human-model.ts";
import { destinationState } from "../src/access-model.ts";
import { landingDestination } from "../src/supervision-model.ts";
import { catalogue, context, errorResponse, identity, organization, own, tenantScope, uuid } from "./human-fixtures.mjs";

const summaryContextId = "c".repeat(64);
const admin = (operations) => ({ ...context(summaryContextId, "administration"), capabilities: [], operations });
const personal = (operations) => ({ ...context(summaryContextId, "personal"), capabilities: [], operations });
const summary = (contextId = summaryContextId, extra = {}) => ({ version: 1, context_id: contextId, authorization_version: "a".repeat(64),
  mode: "administration", evaluated_at: "2026-11-21T12:00:00Z", period: { window: "7d", from: "2026-11-14T12:00:00Z", to: "2026-11-21T12:00:00Z" },
  consistency: "transaction_snapshot",
  sections: Object.fromEntries(["resources", "decisions", "workers", "brokers", "quota", "usage", "attention"].map((key) => [key, { status: "not_granted" }])),
  read_receipt: { request_id: uuid, audit_event_id: "00000000-0000-4000-8000-000000000002", received_at: "2026-11-21T12:00:00Z" }, ...extra });

test("la destination de synthèse suit l’accord et la disponibilité de summary_read, jamais les capacités", () => {
  assert.equal(destinationState(admin(catalogue({ summary_read: ["granted", "tenant", "available"] })), "summary_read"), "view");
  for (const availability of ["migration_required", "profile_unavailable"])
    assert.equal(destinationState(admin(catalogue({ summary_read: ["granted", "tenant", availability] })), "summary_read"), "explanation", availability);
  assert.equal(destinationState(admin(catalogue({ summary_read: ["granted", "tenant", "not_in_this_build"] })), "summary_read"), null,
    "a build without the route offers no destination at all");
  assert.equal(destinationState(admin(catalogue()), "summary_read"), null);
  assert.equal(destinationState(personal(catalogue({ brokers_read: ["requires_administration"] })), "brokers_read"), null);
  assert.equal(destinationState(null, "summary_read"), null);
  // Every capability of the administration, but no summary offered: no destination.
  assert.equal(destinationState({ ...admin(catalogue()), capabilities: ["read_tenant_metadata", "read_broker_registry", "read_worker_registry"] }, "summary_read"), null);
});

test("l’ouverture d’un contexte atterrit sur la synthèse servie, sinon sur l’atterrissage existant", () => {
  const served = ["granted", "tenant", "available"]; const fenced = ["granted", "tenant", "migration_required"];
  assert.equal(landingDestination(admin(catalogue({ summary_read: served, resources_read: organization, brokers_read: tenantScope }))), "overview");
  assert.equal(landingDestination(personal(catalogue({ summary_read: ["granted", "own_and_shared", "available"], resources_read: own }))), "overview");
  // A summary that is not served now never takes the place of a view that reads.
  assert.equal(landingDestination(personal(catalogue({ summary_read: ["granted", "own_and_shared", "migration_required"], resources_read: own }))), "resources");
  assert.equal(landingDestination(admin(catalogue({ summary_read: fenced, workers_read: tenantScope, brokers_read: tenantScope, federation_read: tenantScope }))), "federation");
  assert.equal(landingDestination(admin(catalogue({ summary_read: fenced, workers_read: tenantScope }))), "workers");
  assert.equal(landingDestination(admin(catalogue({ summary_read: fenced, brokers_read: tenantScope }))), "brokers");
  // The worker registry is granted but unavailable for the source profile: the unified view still explains it.
  assert.equal(landingDestination(admin(catalogue({ workers_read: ["granted", "tenant", "profile_unavailable"] }))), "supervision");
  // Nothing else to read: the summary destination says why it is not served instead of an empty landing.
  assert.equal(landingDestination(admin(catalogue({ summary_read: fenced }))), "overview");
  assert.equal(landingDestination(admin(catalogue())), null);
  assert.equal(landingDestination(null), null);
});

function client(handler) {
  const notifications = []; const requests = [];
  const api = new HumanApiClient("private-human-bearer-123456789", {
    onAuthentication: (error) => notifications.push(["authentication", error.code]),
    onContextInvalidated: (error) => notifications.push(["context", error.code]),
  }, async (path, options) => { requests.push({ path, options }); return handler(path, options); });
  return { api, notifications, requests };
}
const offered = (availability = "available") => ({ ...identity(), contexts: [admin(catalogue({ summary_read: ["granted", "tenant", availability], brokers_read: tenantScope }))] });

test("la synthèse est lue dans le contexte choisi et liée à ce contexte", async () => {
  let body = summary();
  const { api, requests } = client((path) => path.endsWith("/identity") ? Response.json(offered()) : Response.json(body));
  try {
    await api.identity(); api.selectContext(summaryContextId);
    const read = await api.summary("7d");
    assert.equal(read.context_id, summaryContextId); assert.equal(read.sections.workers.status, "not_granted");
    const call = requests.at(-1);
    assert.equal(call.path, "/api/v1/human/summary?window=7d");
    assert.equal(call.options.headers["X-AiGuard-Context"], summaryContextId);
    assert.equal(call.options.credentials, "omit"); assert.equal(call.options.cache, "no-store");
    // A summary answered for another context is a contract break, never a value of this one.
    body = summary("d".repeat(64));
    await assert.rejects(api.summary("7d"), HumanContractError);
  } finally { api.close(); }
});

test("un refus de la synthèse reste un 403 lisible par la vue, qui retire ses valeurs et fait relire les accès", async () => {
  const { api } = client((path) => path.endsWith("/identity") ? Response.json(offered()) : errorResponse(403, "permission_denied", "review_permissions"));
  try {
    await api.identity(); api.selectContext(summaryContextId);
    await assert.rejects(api.summary("24h"), (error) => error instanceof HumanApiError && error.status === 403);
  } finally { api.close(); }
});

test("un catalogue d’opérations changé sous le même contexte invalide la sélection", async () => {
  let availability = "migration_required";
  const { api, notifications } = client((path) => path.endsWith("/identity") ? Response.json(offered(availability)) : Response.json(summary()));
  try {
    await api.identity(); api.selectContext(summaryContextId);
    await api.identity();
    assert.equal(api.context?.id, summaryContextId, "the same catalogue keeps the selected context");
    // The operator installed the marker: same capabilities, same authorization version, another offer.
    availability = "available"; await api.identity();
    assert.equal(api.context, null);
    assert.deepEqual(notifications, [["context", "context_revoked"]]);
  } finally { api.close(); }
});
