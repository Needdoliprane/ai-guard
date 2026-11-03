import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHumanIdentity } from "../src/human-model.ts";

const uuid = "00000000-0000-4000-8000-000000000001";
const identity = {
  version: 1,
  principal: { kind: "human_local", installation_id: uuid, account_id: uuid },
  display_name: "Fixture user",
  authentication: { method: "local_credential", credential_id: uuid, expires_at: null },
  contexts: [],
};

test("context coverage preserves an explicit incomplete or complete identity result", () => {
  for (const complete of [false, true]) {
    const parsed = parseHumanIdentity({ ...identity, contexts_complete: complete });
    assert.equal(parsed.contexts_complete, complete);
    assert.deepEqual(parsed.contexts, []);
  }
});

test("context coverage rejects missing and malformed completeness rather than assuming success", () => {
  for (const bad of [identity, ...[null, "false", "true", 0, 1, {}, []].map((value) => ({
    ...identity, contexts_complete: value,
  }))]) {
    assert.throws(() => parseHumanIdentity(bad), /Contrat/);
  }
});
