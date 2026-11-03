import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHumanIdentity, parseResourcePage, parseHumanEventPage, humanUrl } from "../src/human-model.ts";

const uuid = "00000000-0000-4000-8000-000000000001";
const principal = { kind: "human_local", installation_id: uuid, account_id: uuid };
const context = { id: "signed-context-a", tenant_id: "tenant-a", organization_scope: "organization-a", mode: "personal",
  capabilities: ["read_metadata"], membership_revision: "1", authorization_version: "a".repeat(64), expires_at: "2099-01-01T00:00:00Z" };
const envelope = { version: 1, context_id: context.id, authorization_version: context.authorization_version,
  coverage: "authorized_selection", items: [], next_cursor: null, has_more: false,
  read_receipt: { request_id: uuid, audit_event_id: uuid, received_at: "2026-11-01T12:00:00Z" } };

test("l’identité conserve une version d’autorisation opaque et la révision propre d’appartenance", () => {
  const identity = { version: 1, principal, display_name: "Alice", authentication: { method: "local_credential", credential_id: uuid, expires_at: null }, contexts: [context], contexts_complete: true };
  assert.equal(parseHumanIdentity(identity).contexts[0].authorization_version, "a".repeat(64));
  for (const bad of [1, "01", "18446744073709551616", "g".repeat(64)])
    assert.throws(() => parseHumanIdentity({ ...identity, contexts: [{ ...context, authorization_version: bad }] }), /Contrat/);
  for (const bad of [1, "01", "18446744073709551616", "-1"])
    assert.throws(() => parseHumanIdentity({ ...identity, contexts: [{ ...context, membership_revision: bad }] }), /Contrat/);
  assert.throws(() => parseHumanIdentity({ ...identity, authentication: { ...identity.authentication, method: "viewer" } }), /Contrat/);
});

test("les droits sur une ressource restent distincts et une page incorrecte ne devient pas vide", () => {
  const item = { resource: { kind: "sandbox", resource_id: "sandbox/a" }, owner: principal, revision: "2",
    permissions: { metadata: true, content: false, export: false } };
  assert.deepEqual(parseResourcePage({ ...envelope, items: [item] }).items[0].permissions, item.permissions);
  for (const bad of [{}, { ...envelope, items: null }, { ...envelope, next_cursor: "123", has_more: true },
    { ...envelope, has_more: true }, { ...envelope, items: [{ ...item, permissions: { metadata: true } }] }])
    assert.throws(() => parseResourcePage(bad), /Contrat/);
});

test("une page partagée conserve les propriétaires locaux et OIDC sans modifier leurs identités", () => {
  const local = { resource: { kind: "session", resource_id: "session-local" }, owner: principal, revision: "1",
    permissions: { metadata: true, content: true, export: true } };
  const shared = { resource: { kind: "project", resource_id: "project-shared" },
    owner: { kind: "human_oidc", issuer: "https://identity.example.test/realms/a/", subject: '<img src=x onerror="alert(1)">' },
    revision: "2", permissions: { metadata: true, content: false, export: false } };
  const parsed = parseResourcePage({ ...envelope, items: [local, shared] });
  assert.deepEqual(parsed.items, [local, shared]);
  const loopbackOwner = { kind: "human_oidc", issuer: "http://127.0.0.1:8080/realms/discovery", subject: "é".repeat(256) };
  assert.deepEqual(parseResourcePage({ ...envelope, items: [{ ...shared, owner: loopbackOwner }] }).items[0].owner, loopbackOwner);
});

test("un propriétaire OIDC invalide ou une identité de contrôle ne deviennent pas un propriétaire humain", () => {
  const owner = { kind: "human_oidc", issuer: "https://identity.example.test/realms/a", subject: "user-1" };
  const item = { resource: { kind: "session", resource_id: "session-shared" }, owner, revision: "1",
    permissions: { metadata: true, content: false, export: false } };
  const badIssuers = [null, "", " ", "identity.example.test/realms/a", "file:///realms/a", "https://",
    "https://user:password@identity.example.test", "https://identity.example.test?", "https://identity.example.test?tenant=a",
    "https://identity.example.test#", "https://identity.example.test#fragment", "https://identity.example.test/\u0085",
    `https://identity.example.test/${"é".repeat(1024)}`];
  const badSubjects = [null, "", " ", "user\n1", "user\u00851", "a".repeat(513), "é".repeat(257)];
  const badOwners = [
    ...badIssuers.map((issuer) => ({ ...owner, issuer })), ...badSubjects.map((subject) => ({ ...owner, subject })),
    { kind: "legacy_control", credential_id: "legacy-viewer", role: "viewer" }, { kind: "unknown" },
    { ...owner, account_id: uuid },
  ];
  for (const bad of badOwners)
    assert.throws(() => parseResourcePage({ ...envelope, items: [{ ...item, owner: bad }] }), /Contrat/);
});

test("un propriétaire OIDC ne permet pas de se connecter avec une identité ou une méthode SSO non intégrée", () => {
  const identity = { version: 1, principal, display_name: "Alice locale",
    authentication: { method: "local_credential", credential_id: uuid, expires_at: null }, contexts: [context], contexts_complete: true };
  assert.deepEqual(parseHumanIdentity(identity).principal, principal);
  const oidc = { kind: "human_oidc", issuer: "https://identity.example.test/realms/a", subject: "user-1" };
  assert.throws(() => parseHumanIdentity({ ...identity, principal: oidc }), /Contrat/);
  for (const method of ["oidc", "oidc_access_token", "unknown"])
    assert.throws(() => parseHumanIdentity({ ...identity, authentication: { ...identity.authentication, method } }), /Contrat/);
});

test("la projection metadata interdit contenus, séquences et identités de contrôle", () => {
  const event = { metadata: { id: uuid, timestamp: "2026-11-01T12:00:00Z", received_at: "2026-11-01T12:00:01Z",
    kind: "command.completed", source: "test", provenance: "observed", coverage: "partial",
    resource: { kind: "session", resource_id: "session-a" }, decision: "allow" }, content: null };
  assert.equal(parseHumanEventPage({ ...envelope, items: [event], view: "metadata" }).items[0].content, null);
  for (const bad of [{ ...event, content: { command: {}, decision: null, result: null, data: {} } },
    { ...event, metadata: { ...event.metadata, seq: 5 } },
    { ...event, metadata: { ...event.metadata, kind: "control.identity.provisioned" } }])
    assert.throws(() => parseHumanEventPage({ ...envelope, items: [bad], view: "metadata" }), /Contrat/);
  assert.throws(() => parseHumanEventPage({ ...envelope, items: [], view: "metadata", selection_complete: false }, true), /Contrat/);
  const denseStream = { ...envelope, items: [event], view: "metadata", has_more: true, next_cursor: null };
  assert.throws(() => parseHumanEventPage(denseStream), /Contrat/);
  assert.equal(parseHumanEventPage(denseStream, false, "stream").has_more, true);
});

test("les URLs humaines ne contiennent aucun tenant ni identifiant de contexte libre", () => {
  const path = humanUrl("events", { view: "metadata", resource: { kind: "sandbox", resource_id: "a/b & c" }, cursor: "opaque-cursor" });
  const url = new URL(path, "http://localhost");
  assert.equal(url.pathname, "/api/v1/human/events");
  assert.equal(url.searchParams.get("resource_id"), "a/b & c");
  assert.equal(url.searchParams.get("limit"), "25");
  assert.equal(url.searchParams.has("tenant_id"), false);
  assert.equal(url.searchParams.has("context_id"), false);
});
