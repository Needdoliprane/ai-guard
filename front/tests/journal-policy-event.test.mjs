import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHumanEvent } from "../src/human-model.ts";
import { journalEgressEventPresentation, journalPolicyEventLabel } from "../src/journal-policy-event.ts";
import { eventOf, sessionId } from "./supervision-fixtures.mjs";

const session = { kind: "session", resource_id: sessionId(1) };
const replaced = "control.egress_policy.replaced";
const label = (metadata) => journalPolicyEventLabel(parseHumanEvent(eventOf(1, session, metadata), "metadata").metadata);

test("le Journal distingue le remplacement de politique et la demande de révocation", () => {
  assert.equal(label({ kind: replaced }), "Politique réseau remplacée");
  assert.equal(label({ kind: "control.egress_policy.revoke_requested" }), "Révocation de politique réseau demandée");
});

test("les autres événements gardent le libellé générique", () => {
  for (const metadata of [
    { kind: "control.egress_policy.genesis" },
    { kind: "control.egress_policy.revoked" },
    { kind: "command.completed" },
    { source: "worker" },
    { provenance: "declared" },
    { provenance: "inferred" },
    { coverage: "partial" },
    { resource: { kind: "project", resource_id: "fixture-project" } },
    { decision: "deny" },
  ]) assert.equal(label({ kind: replaced, ...metadata }), null);
});

test("le Journal nomme les traces egress et indique les champs absents des métadonnées", () => {
  const present = (kind, extra = {}) => journalEgressEventPresentation(
    parseHumanEvent(eventOf(1, session, { kind, coverage: "partial", ...extra }), "metadata").metadata
  );

  assert.deepEqual(present("egress.decision"), {
    label: "Décision egress enregistrée",
    unknown: "Verdict non fourni dans les métadonnées",
  });
  assert.deepEqual(present("egress.connection"), {
    label: "Résultat de connexion egress enregistré",
    unknown: "Résultat non fourni dans les métadonnées",
  });
  assert.equal(present("egress.closed"), null);
  assert.equal(present("egress.connection", { coverage: "native" }), null);
  assert.equal(present("egress.decision", { decision: "allow" }), null);
  assert.throws(() => present("egress.connection", { connected: true }));
});
