import assert from "node:assert/strict";
import { test } from "node:test";
import { parseMembershipPage, parseHumanQuota, parseUsagePage, parseHumanConfiguration, adminUrls } from "../src/admin-model.ts";

const expected = { contextId: "c".repeat(64), authorizationVersion: "a".repeat(64) };
const anchors = { version: 1, context_id: "c".repeat(64), authorization_version: "a".repeat(64), evaluated_at: "2026-11-21T12:00:00.000000000Z" };
const receipt = { request_id: "00000000-0000-4000-8000-000000000001", audit_event_id: "00000000-0000-4000-8000-000000000002", received_at: "2026-11-21T12:00:00Z" };
const membership = (extra = {}) => ({ membership_id: "00000000-0000-4000-8000-0000000000c1",
  principal: { kind: "human_oidc", issuer: "https://idp.example.test/realms/demo", subject: "user-1" }, display_name: "User One",
  account_active: true, organization_scope: "organization-a", capabilities: ["read_metadata", "read_content"], revision: "3", active: true,
  credentials: [], credentials_complete: true, own: false, ...extra });
const page = (items = [membership()], extra = {}) => ({ ...anchors, source: "tenant_identity_memberships", consistency: "snapshot_per_page",
  items, has_more: false, next_cursor: null, read_receipt: receipt, ...extra });

test("une page d’appartenances garde la provenance et refuse un digest ou une capacité inconnue", () => {
  const parsed = parseMembershipPage(page([membership(), membership({ membership_id: "00000000-0000-4000-8000-0000000000c2", own: true,
    principal: { kind: "human_local", installation_id: "00000000-0000-4000-8000-000000000011", account_id: "00000000-0000-4000-8000-000000000012" },
    credentials: [{ credential_id: "00000000-0000-4000-8000-000000000013", active: true, expires_at: null }] })]), expected);
  assert.equal(parsed.items.length, 2);
  assert.equal(parsed.items[1].own, true);
  for (const bad of [
    page([membership({ capabilities: ["read_everything"] })]),
    page([membership({ credentials: [{ credential_id: "00000000-0000-4000-8000-000000000013", active: true, expires_at: null, sha256_digest: "0".repeat(64) }] })]),
    page([membership(), membership()]),
    page([membership({ revision: 3 })]),
    page([membership()], { has_more: true, next_cursor: null }),
    page([membership()], { context_id: "d".repeat(64) }),
  ]) assert.throws(() => parseMembershipPage(bad, expected), /Contrat/);
});

test("le quota et l’usage restent des chaînes décimales exactes et jamais facturables", () => {
  const quota = parseHumanQuota({ ...anchors, source: "tenant_worker_reservations", max_timeout_seconds: "3600", read_receipt: receipt,
    quota: { max_active_workers: "100", max_cpu_millis: "200000", max_memory_bytes: "107374182400", held_workers: "0", held_cpu_millis: "0", held_memory_bytes: "0" } }, expected);
  assert.equal(quota.quota.max_memory_bytes, "107374182400");
  const allocation = { attempt_id: "00000000-0000-4000-8000-0000000000a1", worker_id: "00000000-0000-4000-8000-0000000000a2",
    terminal_observation_id: "obs-1", started_at: "2026-11-21T10:00:00Z", finished_at: "2026-11-21T10:01:00Z", duration_ns: "60000000000",
    cpu_millicore_nanoseconds: "18446744073709551616", memory_byte_nanoseconds: "1", method_version: 1, billable: false,
    source: { authority_id: "authority-a", runtime_kind: "docker", container_id: "container-1", spec_hash: "f".repeat(64), provenance: "observed" } };
  const usage = { ...anchors, source: "tenant_terminal_allocations", method_version: 1, billable: false, items: [allocation], has_more: false, next_cursor: null, read_receipt: receipt };
  assert.equal(parseUsagePage(usage, expected).items[0].cpu_millicore_nanoseconds, "18446744073709551616");
  for (const bad of [{ ...usage, billable: true }, { ...usage, items: [{ ...allocation, billable: true }] }, { ...usage, items: [{ ...allocation, duration_ns: 60 }] }])
    assert.throws(() => parseUsagePage(bad, expected), /Contrat/);
});

test("la configuration n’accepte que des clés fermées et des mots sans adresse ni chemin", () => {
  const body = { ...anchors, read_receipt: receipt, entries: [
    { axis: "storage", key: "backend", value: "sqlite", provenance: "configured" },
    { axis: "runtime", key: "declared_runtime_kinds", value: "docker,podman", provenance: "declared" },
    { axis: "egress", key: "mode", value: "unknown", provenance: "unknown" } ] };
  assert.equal(parseHumanConfiguration(body, expected).entries.length, 3);
  for (const entry of [
    { axis: "storage", key: "host", value: "db.internal", provenance: "configured" },
    { axis: "storage", key: "backend", value: "/var/lib/ai-guard/tenant.sqlite", provenance: "configured" },
    { axis: "identity", key: "oidc", value: "https://idp.example.test", provenance: "configured" },
    { axis: "storage", key: "backend", value: "sqlite", provenance: "guessed" },
  ]) assert.throws(() => parseHumanConfiguration({ ...body, entries: [entry] }, expected), /Contrat/);
  assert.throws(() => parseHumanConfiguration({ ...body, entries: [body.entries[0], body.entries[0]] }, expected), /Contrat/);
});

test("les routes d’administration restent fermées et paginées par curseur opaque", () => {
  assert.equal(adminUrls.memberships(null), "/api/v1/human/admin/memberships?limit=50");
  assert.equal(adminUrls.usage("ab".repeat(16)), `/api/v1/human/admin/usage?limit=50&cursor=${"ab".repeat(16)}`);
  assert.equal(adminUrls.quota(), "/api/v1/human/admin/quota");
});
