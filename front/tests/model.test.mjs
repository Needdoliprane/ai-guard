import assert from "node:assert/strict";
import { test } from "node:test";
import {
  buildAgentForest,
  matchesFilters,
  projectEvents,
  formatMetric,
  eventSummary,
  mergeEventPage,
  periodFilters,
} from "../src/model.ts";

test("les périodes fixent le début au moment du choix sans borner les événements futurs", () => {
  const chosenAt = Date.parse("2026-10-29T12:00:00.000Z");
  assert.deepEqual(periodFilters("all", chosenAt), {});
  assert.deepEqual(periodFilters("hour", chosenAt), {
    from: "2026-10-29T11:00:00.000Z",
  });
  assert.deepEqual(periodFilters("day", chosenAt), {
    from: "2026-10-28T12:00:00.000Z",
  });
  assert.deepEqual(periodFilters("week", chosenAt), {
    from: "2026-10-22T12:00:00.000Z",
  });
});

test("le filtre temporel des événements live utilise received_at avant timestamp", () => {
  const bounds = {
    from: "2026-10-29T11:00:00.000Z",
    to: "2026-10-29T13:00:00.000Z",
  };
  const event = {
    seq: 1,
    id: "qa",
    kind: "test",
    timestamp: "2020-01-01T00:00:00Z",
    received_at: "2026-10-29T12:00:00Z",
  };
  assert.equal(matchesFilters(event, bounds), true);
  assert.equal(
    matchesFilters(
      {
        ...event,
        timestamp: "2026-10-29T12:00:00Z",
        received_at: "2026-10-29T10:59:59Z",
      },
      bounds,
    ),
    false,
  );
  assert.equal(
    matchesFilters({ ...event, received_at: bounds.from }, bounds),
    true,
  );
  assert.equal(
    matchesFilters({ ...event, received_at: bounds.to }, bounds),
    true,
  );
  assert.equal(
    matchesFilters({ ...event, received_at: "2026-10-29T13:00:01Z" }, bounds),
    false,
  );
  assert.equal(
    matchesFilters(
      { ...event, timestamp: "2026-10-29T12:00:00Z", received_at: null },
      bounds,
    ),
    true,
  );
  assert.equal(
    matchesFilters(
      { ...event, timestamp: "inconnu", received_at: null },
      bounds,
    ),
    false,
  );
});

test("une réponse historique ne supprime pas les événements arrivés pendant sa lecture", () => {
  const event = (seq) => ({
    seq,
    id: String(seq),
    timestamp: "2026-10-29T12:00:00Z",
    kind: "test",
  });
  const result = mergeEventPage(
    [event(12), event(10), event(9)],
    [event(11), event(10)],
    10,
  );
  assert.deepEqual(
    result.map((entry) => entry.seq),
    [12, 11, 10],
  );
  assert.deepEqual(
    mergeEventPage([event(12), event(10)], [event(10), event(9)]).map(
      (entry) => entry.seq,
    ),
    [12, 10, 9],
  );
});

test("construit les liens parent uniquement à partir des identifiants explicites", () => {
  const forest = buildAgentForest([
    { id: "a", name: "Parent", session_id: "identique" },
    { id: "b", name: "Enfant", parent_id: "a" },
    { id: "c", name: "Autre", session_id: "identique" },
    { id: "d", parent_id: "introuvable" },
  ]);
  assert.deepEqual(
    forest.map((node) => node.agent.id),
    ["a", "c", "d"],
  );
  assert.deepEqual(
    forest[0].children.map((node) => node.agent.id),
    ["b"],
  );
  assert.equal(forest[2].issue, "missing-parent");
});

test("un cycle de parents reste visible et ne provoque pas de récursion infinie", () => {
  const forest = buildAgentForest([
    { id: "a", parent_id: "b" },
    { id: "b", parent_id: "a" },
    { id: "c", parent_id: "c" },
  ]);
  const seen = [];
  const visit = (node) => {
    seen.push(node.agent.id);
    node.children.forEach(visit);
  };
  forest.forEach(visit);
  assert.deepEqual([...seen].sort(), ["a", "b", "c"]);
  assert.equal(seen.length, 3);
  assert.ok(forest.some((node) => node.issue === "cycle"));
});

test("une métrique absente reste inconnue tandis que zéro est une valeur réelle", () => {
  assert.equal(formatMetric(null), "Inconnu");
  assert.equal(formatMetric(undefined), "Inconnu");
  assert.equal(formatMetric(Number.NaN), "Inconnu");
  assert.equal(formatMetric(0), "0");
  assert.equal(formatMetric(1200), "1\u202f200");
});

test("les filtres conservent les identifiants exacts et traitent une commande comme du texte", () => {
  const event = {
    seq: 1,
    id: "e1",
    timestamp: "2026-10-29T12:00:00Z",
    kind: "command.completed",
    agent_id: "agent-1",
    sandbox_id: "sandbox-1",
    project_id: "projet",
    command: { argv: ["printf", "<img src=x onerror=alert(1)>"] },
    decision: { outcome: "deny" },
  };
  assert.equal(
    matchesFilters(event, { q: "<img", agent_id: "agent-1", decision: "deny" }),
    true,
  );
  assert.equal(matchesFilters(event, { agent_id: "agent" }), false);
  assert.equal(matchesFilters(event, { decision: "allow" }), false);
  assert.ok(eventSummary(event).includes("<img"));
});

test("les vues sécurité et fichiers reposent sur les événements présents", () => {
  const events = [
    {
      seq: 1,
      id: "1",
      timestamp: "2026-10-29T12:00:00Z",
      kind: "command.requested",
      command: { argv: ["pwd"] },
    },
    {
      seq: 2,
      id: "2",
      timestamp: "2026-10-29T12:00:00Z",
      kind: "policy.decision",
      decision: { outcome: "deny" },
    },
    {
      seq: 3,
      id: "3",
      timestamp: "2026-10-29T12:00:00Z",
      kind: "workspace.diff",
      data: { path: "src/main.rs" },
    },
    {
      seq: 4,
      id: "4",
      timestamp: "2026-10-29T12:00:00Z",
      kind: "agent.started",
    },
  ];
  assert.deepEqual(
    projectEvents(events, "actions").map((item) => item.seq),
    [1, 2],
  );
  assert.deepEqual(
    projectEvents(events, "security").map((item) => item.seq),
    [2],
  );
  assert.deepEqual(
    projectEvents(events, "files").map((item) => item.seq),
    [3],
  );
});
