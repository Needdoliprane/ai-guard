import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHumanIdentity } from "../src/human-model.ts";
const id = "00000000-0000-4000-8000-000000000001";
const principal = { kind: "human_oidc", issuer: "https://id.example.test/realms/a", subject: "subject-stable" };
const local = { kind: "human_local", installation_id: id, account_id: id };
const identity = { version: 1, principal, display_name: "Named OIDC account",
  authentication: { method: "oidc_access_token", credential_id: id, expires_at: "2099-01-01T00:00:00Z" }, contexts: [], contexts_complete: true };
test("exact OIDC API method and verified principal pair is supported", () => {
  assert.deepEqual(parseHumanIdentity(identity), identity);
});
test("local and OIDC authentication methods cannot be substituted or lose their expiry", () => {
  for (const bad of [{ ...identity, principal: local }, { ...identity, authentication: { ...identity.authentication, method: "local_credential" } },
    { ...identity, authentication: { ...identity.authentication, method: "oidc" } },
    { ...identity, authentication: { ...identity.authentication, expires_at: null } }]) assert.throws(() => parseHumanIdentity(bad), /Contrat/);
});
