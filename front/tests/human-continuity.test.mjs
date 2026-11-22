// Resume intention after a context expiry or a session rotation. Pure decisions, no DOM and no network.
// The control never renews a context: it may offer a new one, and the person confirms before any data read.
import assert from "node:assert/strict";
import { test } from "node:test";
import { identity, principal, uuid, secondUuid } from "./human-fixtures.mjs";
import { adminContext, adminOperations, ownedAdminOperations, personalContext } from "./supervision-fixtures.mjs";

const module = await import("../src/human-continuity.ts").catch((error) => {
  if (error?.code === "ERR_MODULE_NOT_FOUND") return {};
  throw error;
});
const present = (name) => assert.equal(typeof module[name], "function", `human-continuity.ts must export ${name}`);
const now = Date.parse("2026-11-10T10:00:00Z");
const future = (seconds) => new Date(now + seconds * 1000).toISOString();
const offered = (extra = {}) => adminContext(undefined, { id: "context-new", expires_at: future(300), ...extra });
const identityWith = (contexts, extra = {}) => ({ ...identity(), contexts, ...extra });
const filters = { fleet: { text: "typed-by-the-user" } };
const capture = (extra = {}) => module.captureIntent({ cause: "context_expired", principal, context: adminContext(undefined, { id: "context-old", expires_at: future(0) }),
  view: "supervision", now, budgetStopped: false, payload: filters, ...extra });
const decide = (intent, contexts, extra = {}) => module.resumeCandidate(intent, identityWith(contexts), { now: now + 1000, visible: true, budgetStopped: false, attempts: 0, ...extra });

test("seule une expiration ou une rotation de session crée une intention, pour une vue reprenable", () => {
  present("captureIntent");
  const intent = capture();
  assert.ok(intent, "une expiration du contexte de la vue unifiée est reprenable");
  assert.deepEqual(Object.keys(intent).sort(), ["cause", "createdAt", "grant", "payload", "principal", "view"]);
  assert.equal(intent.view, "supervision"); assert.equal(intent.createdAt, now);
  assert.equal(intent.cause, "context_expired", "la cause réelle est conservée pour le libellé de reprise");
  assert.equal(capture({ cause: "session_rotated" }).cause, "session_rotated", "un renouvellement de session n’est pas présenté comme une expiration");
  assert.equal(intent.payload, filters, "les filtres saisis sont conservés tels quels");
  const serialized = JSON.stringify({ ...intent, payload: null });
  assert.ok(!serialized.includes("context-old") && !serialized.includes(future(0)), "ni identifiant de contexte ni échéance de l’ancien contexte");
  assert.ok(capture({ cause: "session_rotated" }), "une rotation de jetons OIDC est reprenable");
  assert.ok(capture({ view: "journal" }));
  // The summary keeps only its period and cadence: it is resumable with them, like the other two views.
  const overview = capture({ view: "overview", payload: { overview: { window: "7d", cadence: 0 } } });
  assert.equal(overview.view, "overview"); assert.deepEqual(overview.payload, { overview: { window: "7d", cadence: 0 } });
  for (const cause of ["context_revoked", "context_invalid", "context_required", "permission_denied", "authentication_required", "credential_expired", "navigation", "logout", ""])
    assert.equal(capture({ cause }), null, `${cause || "cause vide"} ne crée aucune intention`);
  for (const view of ["contexts", "resources", "events", "brokers", "workers", "federation", "memberships", "usage", "configuration"])
    assert.equal(capture({ view }), null, `la vue ${view} n’est pas reprise`);
  assert.equal(capture({ budgetStopped: true }), null, "aucune reprise lorsque le budget durable de lecture est arrêté");
  assert.equal(capture({ context: null }), null); assert.equal(capture({ principal: null }), null);
});

test("un seul contexte équivalent est proposé à la reprise ; un contexte élargi, réduit, absent ou ambigu ne l’est pas", () => {
  present("resumeCandidate");
  const intent = capture();
  const decision = decide(intent, [personalContext(undefined, { id: "context-personal" }), offered()]);
  assert.equal(decision.kind, "offer"); assert.equal(decision.context.id, "context-new", "le contexte offert est celui que le contrôle vient de proposer");
  assert.deepEqual(decide(intent, [offered({ capabilities: [...offered().capabilities].reverse() })]).kind, "offer", "l’ordre des capacités ne change pas l’attribution");
  const widened = offered({ capabilities: [...offered().capabilities, "manage_grants"] });
  const reduced = offered({ capabilities: offered().capabilities.filter((capability) => capability !== "read_worker_registry") });
  // Same capabilities, another offer: a withdrawn operation or a changed availability is another attribution.
  const withdrawn = offered({ operations: adminOperations({ brokers_read: ["not_granted"] }) });
  const unavailable = offered({ operations: ownedAdminOperations() });
  for (const [contexts, reason] of [[[widened], "none"], [[reduced], "none"], [[withdrawn], "none"], [[unavailable], "none"], [[], "none"], [[personalContext(undefined, { id: "context-personal" })], "none"],
    [[offered({ tenant_id: "tenant-b" })], "none"], [[offered({ organization_scope: "organization-b" })], "none"], [[offered({ membership_revision: "2" })], "none"],
    [[offered(), offered({ id: "context-new-2" })], "ambiguous"]])
    assert.deepEqual(decide(intent, contexts), { kind: "manual", reason });
  assert.deepEqual(module.resumeCandidate(intent, { ...identityWith([offered()]), principal: { kind: "human_local", installation_id: uuid, account_id: secondUuid } },
    { now: now + 1000, visible: true, budgetStopped: false, attempts: 0 }), { kind: "manual", reason: "other_principal" });
  assert.deepEqual(decide(intent, [offered()], { now: now + 15 * 60 * 1000 + 1 }), { kind: "manual", reason: "too_old" });
  assert.deepEqual(decide(intent, [offered()], { budgetStopped: true }), { kind: "manual", reason: "budget" });
});

test("un contexte reproposé avec son ancienne échéance donne au plus trois relectures, puis le choix manuel", () => {
  present("resumeCandidate");
  const intent = capture();
  const same = offered({ id: "context-old", expires_at: future(0) });
  for (const attempts of [0, 1, 2]) assert.deepEqual(decide(intent, [same], { attempts }), { kind: "retry" });
  assert.deepEqual(decide(intent, [same], { attempts: 3 }), { kind: "manual", reason: "still_expired" });
  assert.deepEqual(decide(intent, [same], { attempts: 0, visible: false }), { kind: "manual", reason: "hidden" }, "aucune relecture automatique onglet masqué");
  assert.equal(decide(intent, [offered()], { visible: false }).kind, "offer", "une offre valide reste une offre : elle n’émet aucune lecture");
  assert.equal(decide(intent, [offered()], { attempts: 3 }).kind, "offer", "après les tentatives, une relecture explicite peut encore offrir la reprise");
  present("resumeRetryDelay");
  assert.deepEqual([0, 1, 2, 3, 9].map(module.resumeRetryDelay), [2000, 4000, 8000, 8000, 8000], "relectures espacées et bornées");
});

test("une reprise déjà confirmée ne rouvre la vue que dans un onglet visible : onglet masqué, l’offre est gardée pour un nouveau clic", () => {
  present("resumeCandidate");
  const intent = capture();
  assert.deepEqual(decide(intent, [offered()], { confirmed: true, visible: false }), { kind: "offer", context: offered(), reopen: false },
    "onglet masqué : un contexte valide proposé après la confirmation reste une offre, rien ne se rouvre");
  assert.deepEqual(decide(intent, [offered()], { confirmed: true }), { kind: "offer", context: offered(), reopen: true }, "onglet visible : la reprise déjà confirmée aboutit sans second clic");
  assert.deepEqual(decide(intent, [offered()], { confirmed: false }), { kind: "offer", context: offered(), reopen: false }, "sans confirmation, une offre attend toujours un clic");
  assert.deepEqual(decide(intent, [offered()]), { kind: "offer", context: offered(), reopen: false }, "la confirmation n’est jamais supposée");
  const same = offered({ id: "context-old", expires_at: future(0) });
  assert.deepEqual(decide(intent, [same], { confirmed: true, visible: false }), { kind: "manual", reason: "hidden" }, "onglet masqué et contexte encore échu : la confirmation ne demande aucune relecture");
  assert.deepEqual(decide(intent, [same], { confirmed: true }), { kind: "retry" });
  present("deferredReadAllowed");
  assert.equal(module.deferredReadAllowed({ visible: false }), false, "aucune relecture différée ne démarre onglet masqué");
  assert.equal(module.deferredReadAllowed({ visible: true }), true);
});
