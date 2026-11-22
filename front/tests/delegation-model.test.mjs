// What the delegated-change editors may offer, from the published delegation rules. Pure: no DOM, no network.
import assert from "node:assert/strict";
import { test } from "node:test";
import { CHOICE_REASON, CONTROL_ONLY_CAPABILITIES, capabilityChoices, delegationOffers, membershipChangeKey, membershipDiff, membershipDraft, membershipProblems,
  permissionAddable, shareCandidates, shareDiff, shareProblems, toggleCapability } from "../src/delegation-model.ts";
import { catalogue, context } from "./human-fixtures.mjs";
import { memberPrincipal } from "./admin-fixtures.mjs";

const served = ["granted", "tenant", "available"];
const admin = (capabilities, operations = catalogue({ memberships_manage: served, grants_manage: served, memberships_read: served })) =>
  ({ ...context("c".repeat(64), "administration"), capabilities, operations });
const membership = (capabilities, extra = {}) => ({ membership_id: "00000000-0000-4000-8000-0000000000c1", principal: memberPrincipal(3),
  display_name: "Member Three", account_active: true, organization_scope: "organization-a", capabilities, revision: "3", active: true,
  credentials: [], credentials_complete: true, own: false, ...extra });

test("les éditeurs ne sont offerts que lorsque le catalogue accorde et sert leur opération", () => {
  assert.deepEqual(delegationOffers(admin([])), { memberships: true, grants: true });
  for (const availability of ["not_in_this_build", "migration_required", "profile_unavailable"])
    assert.deepEqual(delegationOffers(admin(["manage_memberships", "manage_grants", "read_memberships"],
      catalogue({ memberships_manage: ["granted", "tenant", availability], grants_manage: ["granted", "tenant", availability] }))), { memberships: false, grants: false }, availability);
  // Every capability of the delegation, but the catalogue offers nothing: no editor.
  assert.deepEqual(delegationOffers(admin(["manage_memberships", "manage_grants", "read_memberships"], catalogue())), { memberships: false, grants: false });
  assert.deepEqual(delegationOffers(null), { memberships: false, grants: false });
});

test("une capacité ne s’ajoute que si le contexte la détient, et jamais une capacité réservée au contrôle", () => {
  const actor = admin(["read_metadata", "read_content", "manage_memberships", "read_memberships", "read_worker_registry"]);
  const choices = Object.fromEntries(capabilityChoices(actor, membership(["read_worker_registry", "read_metadata"])).map((choice) => [choice.capability, choice]));
  assert.equal(Object.keys(choices).length, 14, "every capability the control knows is listed");
  // Held by the membership: kept or removed, whatever its kind.
  assert.deepEqual([choices.read_worker_registry.current, choices.read_worker_registry.addable, choices.read_worker_registry.reason], [true, false, "control_only"]);
  assert.deepEqual([choices.read_metadata.current, choices.read_metadata.addable], [true, true]);
  // Not held: addable only when the context holds it and it is not control-only.
  assert.deepEqual([choices.read_content.current, choices.read_content.addable, choices.read_content.reason], [false, true, null]);
  assert.deepEqual([choices.export.addable, choices.export.reason], [false, "not_held"]);
  assert.deepEqual([choices.read_memberships.addable, choices.read_memberships.reason], [false, "control_only"], "held by the context, yet reserved to the control");
  for (const capability of CONTROL_ONLY_CAPABILITIES) assert.equal(choices[capability].addable, false, capability);
  assert.match(CHOICE_REASON.control_only, /jamais ajoutée/);
});

test("le contrôle refuserait ces changements : la vérification le dit avant toute requête", () => {
  const actor = admin(["read_metadata", "read_content", "manage_memberships", "read_memberships", "read_worker_registry"]);
  const member = membership(["read_worker_registry", "read_metadata"]);
  assert.deepEqual(membershipProblems(actor, member, { capabilities: ["read_worker_registry", "read_metadata"], active: true }), ["no_change"]);
  assert.deepEqual(membershipProblems(actor, member, { capabilities: ["read_metadata"], active: true }), [], "a control-only capability may be removed");
  assert.deepEqual(membershipProblems(actor, member, { capabilities: ["read_worker_registry", "read_metadata", "read_content"], active: true }), [], "a held capability may be added");
  assert.deepEqual(membershipProblems(actor, member, { capabilities: ["read_worker_registry", "read_metadata", "read_memberships"], active: true }), ["control_only_added"]);
  assert.deepEqual(membershipProblems(actor, member, { capabilities: ["read_worker_registry", "read_metadata", "export"], active: true }), ["not_held_added"]);
  assert.deepEqual(membershipProblems(actor, { ...member, own: true }, { capabilities: ["read_metadata"], active: true }), ["own"]);
  // Reactivation reactivates every capability kept: each must be one this context could add.
  const inactive = { ...member, active: false };
  assert.deepEqual(membershipProblems(actor, inactive, { capabilities: ["read_worker_registry", "read_metadata"], active: true }), ["reactivation_control_only"]);
  assert.deepEqual(membershipProblems(actor, inactive, { capabilities: ["read_metadata"], active: true }), []);
  assert.deepEqual(membershipProblems(actor, { ...inactive, capabilities: ["export"] }, { capabilities: ["export"], active: true }), ["reactivation_not_held"]);
});

test("la confirmation liste exactement ce qui est ajouté et retiré, et dit quand tout accès disparaît", () => {
  const member = membership(["read_worker_registry", "read_metadata"]);
  assert.deepEqual(membershipDiff(member, { capabilities: ["read_metadata", "read_content"], active: true }),
    { added: ["read_content"], removed: ["read_worker_registry"], activation: "kept", removesAllAccess: false });
  assert.equal(membershipDiff(member, { capabilities: ["read_metadata"], active: false }).activation, "deactivated");
  assert.equal(membershipDiff(member, { capabilities: ["read_metadata"], active: false }).removesAllAccess, true);
  assert.equal(membershipDiff(member, { capabilities: [], active: true }).removesAllAccess, true);
  assert.equal(membershipDiff({ ...member, active: false }, { capabilities: ["read_metadata"], active: true }).activation, "reactivated");
});

const grant = () => ({ evaluated_at: "2026-11-21T12:00:00Z", source: "tenant_identity_resources", resource: { kind: "session", resource_id: "session-a" },
  owner: memberPrincipal(1), owner_display_name: "Owner One", revision: "4", active: true,
  shares: [{ principal: memberPrincipal(2), display_name: "Member Two", permissions: ["metadata", "content"] }], read_receipt: {} });

test("une permission de partage ne s’ajoute que si le contexte détient sa capacité", () => {
  assert.equal(permissionAddable(admin(["read_tenant_metadata"]), "metadata"), true);
  assert.equal(permissionAddable(admin(["read_metadata"]), "metadata"), true);
  assert.equal(permissionAddable(admin(["read_tenant_metadata"]), "content"), false);
  assert.equal(permissionAddable(admin(["read_content"]), "content"), true);
  assert.equal(permissionAddable(admin(["read_metadata", "read_content"]), "export"), false);
  assert.equal(permissionAddable(admin(["export"]), "export"), true);
});

test("l’éditeur de partages ne propose jamais l’acteur ni le propriétaire, ni un membre déjà partagé ou inactif", () => {
  const self = memberPrincipal(9);
  const members = [membership([], { principal: memberPrincipal(1), display_name: "Owner One" }), membership([], { principal: memberPrincipal(2), display_name: "Member Two" }),
    membership([], { principal: self, display_name: "Me", own: true }), membership([], { principal: memberPrincipal(4), display_name: "Inactive", active: false }),
    membership([], { principal: memberPrincipal(5), display_name: "Account off", account_active: false }), membership([], { principal: memberPrincipal(6), display_name: "Candidate" }),
    membership([], { principal: memberPrincipal(6), display_name: "Candidate twice" })];
  const draft = grant().shares.map((share) => ({ ...share, permissions: [...share.permissions] }));
  assert.deepEqual(shareCandidates(members, grant(), self, draft).map((member) => member.display_name), ["Candidate"]);
  assert.deepEqual(shareCandidates(members, grant(), self, []).map((member) => member.display_name), ["Member Two", "Candidate"], "the owner is never a candidate");
});

test("les partages refusés par les règles sont nommés ; un retrait reste toujours possible", () => {
  const self = memberPrincipal(9); const actor = admin(["read_metadata"]);
  const current = () => grant().shares.map((share) => ({ ...share, permissions: [...share.permissions] }));
  assert.deepEqual(shareProblems(actor, grant(), current(), self), ["no_change"]);
  // Content is removed although this context could not add it back: removal is free.
  assert.deepEqual(shareProblems(actor, grant(), [{ ...current()[0], permissions: ["metadata"] }], self), []);
  assert.deepEqual(shareProblems(actor, grant(), [], self), []);
  assert.deepEqual(shareProblems(actor, grant(), [...current(), { principal: memberPrincipal(6), display_name: "New", permissions: ["export"] }], self), ["not_addable"]);
  assert.deepEqual(shareProblems(actor, grant(), [...current(), { principal: memberPrincipal(6), display_name: "New", permissions: ["metadata"] }], self), []);
  assert.ok(shareProblems(actor, grant(), [...current(), { principal: self, display_name: "Me", permissions: ["metadata"] }], self).includes("self"));
  assert.ok(shareProblems(actor, grant(), [...current(), { principal: memberPrincipal(1), display_name: "Owner", permissions: ["metadata"] }], self).includes("owner"));
  assert.ok(shareProblems(actor, grant(), [{ ...current()[0], permissions: [] }], self).includes("empty"));
  assert.ok(shareProblems(actor, grant(), [...current(), ...current()], self).includes("duplicate"));
  const many = Array.from({ length: 65 }, (_, index) => ({ principal: { kind: "human_oidc", issuer: "https://idp.example.test", subject: `user-${index}` }, display_name: `U${index}`, permissions: ["metadata"] }));
  assert.ok(shareProblems(actor, grant(), many, self).includes("too_many"));
  assert.deepEqual(shareDiff(grant(), [{ ...current()[0], permissions: ["metadata", "export"] }, { principal: memberPrincipal(6), display_name: "New", permissions: ["metadata"] }]), {
    added: [{ principal: memberPrincipal(6), display_name: "New", permissions: ["metadata"] }], removed: [],
    changed: [{ principal: memberPrincipal(2), display_name: "Member Two", added: ["export"], removed: ["content"] }] });
});

test("un brouillon garde l’ordre fermé du contrôle, et la même intention garde la même clé d’opération", () => {
  const member = membership(["export", "read_metadata"]);
  const draft = membershipDraft(member);
  assert.deepEqual(draft, { capabilities: ["read_metadata", "export"], active: true });
  const added = toggleCapability(toggleCapability(draft, "read_content", true), "read_content", true);
  assert.deepEqual(added.capabilities, ["read_metadata", "read_content", "export"], "a capability is listed once, in the control order");
  assert.deepEqual(toggleCapability(added, "read_content", false), draft);
  assert.equal(membershipChangeKey(member, added), membershipChangeKey(member, { capabilities: ["export", "read_content", "read_metadata"], active: true }));
  assert.notEqual(membershipChangeKey(member, added), membershipChangeKey({ ...member, revision: "4" }, added), "a new revision is a new intention");
  assert.notEqual(membershipChangeKey(member, added), membershipChangeKey(member, { ...added, active: false }));
});
