import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanContractError, parseHumanBrokerPage } from "../src/human-model.ts";
import { humanOwnedBrokersUrl, parseHumanOwnedBrokerPage, checkpointLabel, checkpointFacts, formatBrokerDate } from "../src/human-owned-brokers.ts";
import { ownedBroker, ownedBrokerPage, noCheckpoint } from "./owned-brokers-fixture.mjs";
import { brokerAuthorityLabel } from "../src/supervision-model.ts";

test("owned: la projection conserve les révisions exactes et les fraîcheurs indépendantes", () => {
  const value = ownedBrokerPage(); value.items[0].grant.revision = "9223372036854775807";
  value.items[0].checkpoint.freshness = "stale";
  const page = parseHumanOwnedBrokerPage(value); const item = page.items[0];
  assert.equal(page.profile, "human_broker_registry_owned_v1"); assert.equal(page.count_scope, "page");
  assert.equal(item.grant_revision, "9223372036854775807"); assert.equal(item.state, "current");
  assert.equal(item.owned.checkpoint.freshness, "stale");
  assert.match(checkpointLabel(item), /périmé/);
  assert.match(checkpointFacts(item).map(([, value]) => value).join(" "), /déclarée.*observée/);
  assert.throws(() => parseHumanBrokerPage(value), HumanContractError, "Le profil historique n’accepte jamais la forme owned");
});

test("owned: absent, inconnu et reçu vide ne deviennent jamais zéro activité", () => {
  const unknown = parseHumanOwnedBrokerPage(ownedBrokerPage([ownedBroker({ checkpoint: noCheckpoint() })])).items[0];
  const absent = parseHumanOwnedBrokerPage(ownedBrokerPage([ownedBroker({ checkpoint: noCheckpoint("absent") })])).items[0];
  const empty = ownedBroker(); empty.checkpoint.reference_count = 0;
  const received = parseHumanOwnedBrokerPage(ownedBrokerPage([empty])).items[0];
  assert.notEqual(checkpointLabel(unknown), checkpointLabel(absent));
  assert.match(checkpointFacts(unknown).map(([, value]) => value).join(" "), /inconnue/);
  assert.match(checkpointFacts(received).map(([, value]) => value).join(" "), /0 référence.*ne mesure pas l’activité/);
});

test("owned: parser fermé, versionné, borné et cohérent avec la projection Rust", () => {
  const cases = [
    (p) => { p.version = 2; }, (p) => { p.profile = "legacy"; }, (p) => { p.count_scope = "tenant"; },
    (p) => { p.credential = "private"; }, (p) => { p.items[0].grant.raw_grant = {}; },
    (p) => { p.items[0].checkpoint.jobs = []; }, (p) => { p.items[0].heartbeat.operation_id = "private"; },
    (p) => { p.items[0].checkpoint.reference_count = 101; }, (p) => { p.items[0].checkpoint.reference_count = null; },
    (p) => { p.items[0].checkpoint.publication_provenance = "observed"; },
    (p) => { p.items[0].checkpoint = { ...noCheckpoint(), reference_count: 0 }; },
    (p) => { p.items[0].clock_uncertain = true; }, (p) => { p.items[0].heartbeat.current_incarnation = false; },
    (p) => { p.items[0].heartbeat.last = null; }, (p) => { p.items[0].grant.revoked = true; },
    (p) => { p.items[0].grant.lease_seconds = 301; }, (p) => { p.items[0].grant.revision = "9223372036854775808"; },
    (p) => { p.items[0].checkpoint.lease_expires_at = "2026-11-01T12:00:00Z"; },
    (p) => { p.items[0].checkpoint.received_at = "2026-11-01T12:00:01.000000002Z"; p.items[0].checkpoint.lease_expires_at = "2026-11-01T12:00:01.000000001Z"; },
    (p) => { p.items[0].heartbeat.last.lease_expires_at = "2020-01-01T00:00:00Z"; },
    (p) => { p.items[0].grant.not_after = "1969-12-31T23:59:59Z"; },
    (p) => { p.evaluated_at = "2026-11-01T12:00:02." + "0".repeat(65) + "Z"; },
    (p) => { p.items[0].grant.granted_at = "2026-02-30T12:00:00Z"; },
    (p) => { p.read_receipt.received_at = "1970-01-01T00:00:00Z"; },
    (p) => { p.items.push(structuredClone(p.items[0])); },
    (p) => { p.items = Array.from({ length: 101 }, () => ownedBroker()); },
    (p) => { p.has_more = true; p.next_cursor = "opaque-legacy-cursor"; },
    (p) => { p.items[0].grant.authorities[0].authority_id = "a".repeat(262145); },
  ];
  for (const mutate of cases) { const page = ownedBrokerPage(); mutate(page); assert.throws(() => parseHumanOwnedBrokerPage(page), HumanContractError, String(mutate)); }
});

test("owned: horloge checkpoint incertaine et heartbeat historique sont explicites", () => {
  const item = ownedBroker(); item.checkpoint.freshness = "clock_uncertain"; item.clock_uncertain = true;
  item.heartbeat.last.incarnation_id = item.broker_id; item.heartbeat.current_incarnation = false;
  const parsed = parseHumanOwnedBrokerPage(ownedBrokerPage([item])).items[0];
  assert.equal(parsed.owned.clock_uncertain, true); assert.match(checkpointLabel(parsed), /Horloge incertaine/);
  assert.notEqual(parsed.last_heartbeat.incarnation_id, parsed.incarnation_id);
});

test("owned: les curseurs appartiennent uniquement au profil owned", () => {
  assert.equal(humanOwnedBrokersUrl(), "/api/v1/human/owned/brokers?limit=25");
  assert.match(humanOwnedBrokersUrl("a".repeat(1082), 100), /^\/api\/v1\/human\/owned\/brokers\?limit=100&cursor=a+$/);
  assert.equal(parseHumanOwnedBrokerPage(ownedBrokerPage([], { has_more: true, next_cursor: "1".repeat(1082) })).next_cursor, "1".repeat(1082));
  for (const cursor of ["legacy-cursor", "a".repeat(1081), "A".repeat(1082)]) assert.throws(() => humanOwnedBrokersUrl(cursor), HumanContractError);
});

test("owned: un identifiant d’autorité blanc valide garde sa valeur et son libellé explicite", () => {
  const value = ownedBrokerPage(); value.items[0].grant.authorities[0].authority_id = "  ";
  const authority = parseHumanOwnedBrokerPage(value).items[0].authorities[0];
  assert.equal(authority.authority_id, "  "); assert.equal(brokerAuthorityLabel(authority), 'Identifiant blanc · "  "');
});

test("owned: une seconde intercalaire conserve sa chaîne et son ordre59/60/00 sans inventer un instant local", () => {
  const value = ownedBrokerPage(); const item = value.items[0];
  const leap = "2016-12-31T23:59:60.500Z";
  value.evaluated_at = leap; value.read_receipt.received_at = leap;
  item.grant.granted_at = "2016-12-31T23:59:59Z"; item.grant.not_after = "2017-01-01T00:00:00Z";
  item.grant.revoked = true; item.grant.revoked_at = leap;
  item.heartbeat.last.received_at = leap; item.heartbeat.last.lease_expires_at = "2017-01-01T00:00:00Z";
  item.checkpoint.received_at = "2016-12-31T23:59:59.999999999Z"; item.checkpoint.lease_expires_at = leap;
  const parsed = parseHumanOwnedBrokerPage(value);
  assert.equal(parsed.evaluated_at, leap); assert.equal(parsed.items[0].last_heartbeat.received_at, leap);
  assert.equal(parsed.items[0].owned.checkpoint.lease_expires_at, leap);
  assert.equal(formatBrokerDate(leap), `${leap} · date serveur conservée, conversion locale inconnue`);
  value.items[0].checkpoint.received_at = "2017-01-01T00:00:00Z";
  assert.throws(() => parseHumanOwnedBrokerPage(value), HumanContractError, "La seconde suivante précède une expiration intercalaire : ordre invalide");
});
