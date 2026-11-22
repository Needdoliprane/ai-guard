// Presentation of the operation catalogue of a named-human context. Pure: no DOM, no network.
import assert from "node:assert/strict";
import { test } from "node:test";
import { OPERATIONS } from "../src/human-model.ts";
import { OPERATION_FACTS, access, accessExplanation, accessState, accessStateLabel, availabilityText, can, grantedUnavailable, permissionText, scopeText } from "../src/access-model.ts";
import { OPERATION_ORDER, catalogue, context, own, tenantScope } from "./human-fixtures.mjs";

const admin = (operations) => ({ ...context("context-admin", "administration"), capabilities: [], operations });
const personal = (operations) => ({ ...context("context-a", "personal"), capabilities: [], operations });

test("le catalogue statique couvre exactement les opérations du contrat, dans leur ordre", () => {
  assert.deepEqual([...OPERATIONS], OPERATION_ORDER);
  assert.deepEqual(Object.keys(OPERATION_FACTS), OPERATION_ORDER);
  const classes = Object.fromEntries(Object.entries(OPERATION_FACTS).map(([operation, facts]) => [operation, facts.class]));
  assert.deepEqual(classes, {
    resources_read: "read_metadata", events_read_metadata: "read_metadata", events_read_content: "read_content", events_export: "export",
    events_stream: "read_metadata", summary_read: "read_metadata", brokers_read: "admin_read", workers_read: "admin_read",
    federation_read: "admin_read", quota_read: "admin_read", usage_read: "admin_read", memberships_read: "admin_read",
    configuration_read: "admin_read", memberships_manage: "mutation", grants_manage: "mutation", launch_approve: "mutation",
    session_launch_prepare: "mutation", session_launch: "mutation", session_message_send: "mutation", session_messages_read: "read_content",
    session_attribution_read: "read_metadata", egress_policies_read: "read_metadata", egress_attempts_read: "read_metadata",
    egress_grants_read: "read_metadata", egress_coverage_read: "read_metadata", egress_summary_read: "read_metadata",
    egress_grant_approve: "mutation", egress_grant_revoke: "mutation" });
  assert.deepEqual(OPERATION_ORDER.filter((operation) => OPERATION_FACTS[operation].resourceAcl), ["resources_read", "events_read_metadata",
    "events_read_content", "events_export", "events_stream", "grants_manage", "launch_approve", "session_launch_prepare", "session_launch", "session_message_send",
    "session_messages_read", "session_attribution_read", "egress_policies_read", "egress_attempts_read", "egress_grants_read",
    "egress_coverage_read", "egress_summary_read"]);
  for (const facts of Object.values(OPERATION_FACTS)) assert.ok(facts.label && facts.description && !facts.description.endsWith("."));
});

test("seule une opération accordée et servie ouvre une lecture ; les quatre états restent distincts", () => {
  const offer = admin(catalogue({ brokers_read: tenantScope, workers_read: ["granted", "tenant", "profile_unavailable"],
    federation_read: ["granted", "tenant", "migration_required"], quota_read: tenantScope }));
  assert.equal(can(offer, "brokers_read"), true);
  for (const operation of ["workers_read", "federation_read", "quota_read", "resources_read"]) assert.equal(can(offer, operation), false, operation);
  assert.equal(can(null, "brokers_read"), false); assert.equal(access(null, "brokers_read"), null);
  assert.deepEqual(access(offer, "brokers_read"), { operation: "brokers_read", permission: "granted", scope: "tenant", availability: "available" });
  assert.equal(grantedUnavailable(offer, "workers_read"), "profile_unavailable");
  assert.equal(grantedUnavailable(offer, "federation_read"), "migration_required");
  assert.equal(grantedUnavailable(offer, "quota_read"), "migration_required", "the fixture stores carry no human rights marker by default");
  assert.equal(grantedUnavailable(offer, "brokers_read"), null); assert.equal(grantedUnavailable(offer, "resources_read"), null);
  assert.equal(accessState(access(offer, "brokers_read")), "granted");
  assert.equal(accessState(access(offer, "workers_read")), "unavailable");
  assert.equal(accessState(access(offer, "resources_read")), "not_granted");
  assert.equal(accessState(null), "not_granted");
  const redirected = personal(catalogue({ brokers_read: ["requires_administration"] }));
  assert.equal(accessState(access(redirected, "brokers_read")), "elsewhere"); assert.equal(can(redirected, "brokers_read"), false);
  assert.deepEqual(Object.values(accessStateLabel), ["Accordé", "Non accordé", "Autre contexte", "Indisponible"]);
});

test("chaque permission et chaque disponibilité a son texte, sans jamais présenter une lecture absente comme vide", () => {
  assert.equal(permissionText.not_granted, "Non accordé à ce contexte : aucune requête n’est émise.");
  assert.match(permissionText.requires_administration, /ouvrez le contexte d’administration de la même appartenance\.$/);
  assert.match(permissionText.requires_personal, /ouvrez le contexte personnel de la même appartenance\.$/);
  assert.match(availabilityText.migration_required, /^La base du tenant attend la migration explicite des droits humains par l’opérateur/);
  assert.match(availabilityText.profile_unavailable, /^Le profil de sources adopté par le contrôle ne fournit pas cette lecture/);
  assert.match(availabilityText.profile_unavailable, /rien n’est déduit/);
  assert.match(availabilityText.not_in_this_build, /^Ce build ne sert encore aucune route pour cette opération/);
  for (const availability of ["migration_required", "profile_unavailable", "not_in_this_build"]) {
    const text = availabilityText[availability];
    assert.match(text, /aucune requête n’est émise|aucune route historique n’est appelée|rien n’est lu/, `${availability} says that nothing is read`);
    assert.doesNotMatch(text, /\b(?:aucun(?:e)? (?:worker|broker|ressource|événement)s?|zéro|est vide|sont vides)\b/, `${availability} never reads as an empty result`);
  }
  assert.deepEqual(Object.keys(scopeText), ["own_and_shared", "organization", "tenant"]);
});

test("l’explication nomme l’opération, sa portée ou son motif, et ce qui est lu", () => {
  const offer = personal(catalogue({ resources_read: own, events_read_content: ["granted", "own_and_shared", "migration_required"],
    workers_read: ["requires_administration"] }));
  assert.equal(accessExplanation(offer, "resources_read"),
    "Accordé par l’opération resources_read (portée : vos ressources et celles partagées avec vous) : ressources listées avec leurs propres permissions de métadonnées, de contenu et d’export.");
  assert.equal(accessExplanation(offer, "events_read_content"),
    `Opération events_read_content accordée (portée : vos ressources et celles partagées avec vous), mais indisponible. ${availabilityText.migration_required}`);
  assert.equal(accessExplanation(offer, "workers_read"), `Opération workers_read non offerte par ce contexte. ${permissionText.requires_administration}`);
  assert.equal(accessExplanation(offer, "brokers_read"), "Opération brokers_read non accordée à ce contexte : aucune requête n’est émise.");
  assert.equal(accessExplanation(null, "brokers_read"), "Aucun contexte sélectionné : aucune requête n’est émise.");
  const launch = admin(catalogue({ session_launch: ["requires_personal"] }));
  assert.equal(accessExplanation(launch, "session_launch"), `Opération session_launch non offerte par ce contexte. ${permissionText.requires_personal}`);
});
