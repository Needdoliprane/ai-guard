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

const brokerFailureEvent = {
  seq: 5,
  id: "5",
  timestamp: "2026-10-29T12:00:00Z",
  kind: "broker.batch.failure",
  source: "runner",
  provenance: "observed",
  data: {
    version: 1,
    phase: "worker",
    broker_step: "failure_observed",
    runtime_effect: "unknown",
    enforcer: null,
    runtime: null,
  },
};

test("un échec broker v1 présente sa phase et son étape sans confirmer un effet d’exécution", () => {
  for (const [step, label] of [
    ["failure_observed", "Observation d’un échec"],
    ["ownership_reload", "Vérification du conteneur enregistré"],
    ["created_exchange", "Confirmation de la création"],
  ]) {
    const summary = eventSummary({
      ...brokerFailureEvent,
      data: { ...brokerFailureEvent.data, broker_step: step },
    });
    assert.ok(summary.includes("Échec signalé"));
    assert.ok(summary.includes("Phase : Worker"));
    assert.ok(summary.includes(`Étape : ${label}`));
    assert.ok(summary.includes("Effet sur l’exécution : non confirmé"));
  }
});

test("un échec broker sans commande reste dans Actions sans devenir un incident de sécurité", () => {
  const events = [
    brokerFailureEvent,
    { ...brokerFailureEvent, seq: 6, kind: "broker.batch.failure.extra" },
  ];
  assert.deepEqual(projectEvents(events, "actions").map((event) => event.seq), [5]);
  assert.deepEqual(projectEvents(events, "security"), []);
  assert.deepEqual(projectEvents(events, "overview"), events);
});

test("les métadonnées broker inconnues restent inconnues sans reprendre de texte arbitraire", () => {
  const canary = "BROKER_DIAGNOSTIC_PRIVATE_CANARY";
  for (const data of [
    { version: 1, phase: canary, broker_step: canary },
    { version: 2, phase: "worker", broker_step: "created_exchange" },
    { version: canary, phase: "worker", broker_step: "ownership_reload" },
    { phase: "worker", broker_step: "failure_observed" },
    { version: 1, phase: "toString", broker_step: "__proto__" },
  ]) {
    const summary = eventSummary({
      ...brokerFailureEvent,
      command: { argv: [canary] },
      decision: { reason: canary },
      data: {
        ...data,
        runtime_effect: "completed",
        enforcer: { stage: canary },
        runtime: { stage: canary },
        path: canary,
        message: canary,
        reason: canary,
        task: canary,
      },
    });
    assert.ok(summary.includes("Échec signalé"));
    assert.ok(summary.includes("Phase : Inconnue"));
    assert.ok(summary.includes("Étape : Inconnue"));
    assert.ok(summary.includes("Effet sur l’exécution : non confirmé"));
    assert.ok(!summary.includes(canary));
    assert.ok(!/Created|Worker|completed|toString|__proto__|function/.test(summary));
  }
});

test("les étapes natives connues complètent les anciens échecs broker sans inventer leur effet", () => {
  const canary = "NATIVE_FAILURE_PRIVATE_CANARY";
  const summarize = (data) => eventSummary({
    ...brokerFailureEvent,
    command: { argv: [canary] },
    decision: { reason: canary },
    data: {
      version: 1,
      phase: "worker",
      runtime_effect: "unknown",
      enforcer: null,
      runtime: null,
      message: canary,
      reason: canary,
      cause: canary,
      ...data,
    },
  });
  const stages = [
    ["enforcer", "register", "Connexion au gardien d’arrêt"],
    ["enforcer", "before_claim", "Vérification avant démarrage"],
    ["enforcer", "prepare", "Préparation de l’exécution"],
    ["enforcer", "before_spawn", "Vérification finale avant démarrage"],
    ["runtime", "authority_before_spawn", "Vérification de l’autorité du moteur"],
    ["runtime", "invocation_before_spawn", "Vérification du programme"],
    ["runtime", "stop_before_spawn", "Vérification de la poursuite du lancement"],
    ["runtime", "guard_before_spawn", "Vérification du garde"],
    ["runtime", "spawn", "Lancement du programme"],
    ["runtime", "started_observation", "Vérification de l’exécution"],
  ];
  for (const [source, stage, label] of stages) {
    for (const broker of [{}, { broker_step: null }]) {
      const summary = summarize({
        ...broker,
        [source]: { stage, reason: canary, cause: canary, container_id: canary },
      });
      assert.ok(summary.includes(`Étape : ${label}`), `${source}.${stage}`);
      assert.ok(summary.includes("Effet sur l’exécution : non confirmé"));
      assert.ok(!summary.includes(canary));
    }
  }
  for (const [data, label] of [
    [{ broker_step: "failure_observed", enforcer: { stage: "register" }, runtime: { stage: "spawn" } }, "Observation d’un échec"],
    [{ broker_step: canary, enforcer: { stage: "before_spawn" }, runtime: { stage: "spawn" } }, "Vérification finale avant démarrage"],
    [{ enforcer: { stage: canary }, runtime: { stage: "started_observation" } }, "Vérification de l’exécution"],
    [{ enforcer: { stage: canary }, runtime: { stage: canary } }, "Inconnue"],
    [{ enforcer: { stage: "constructor" }, runtime: { stage: "toString" } }, "Inconnue"],
    [{ enforcer: { stage: "spawn" }, runtime: { stage: "register" } }, "Inconnue"],
    [{ version: 2, broker_step: "ownership_reload", enforcer: { stage: "register" }, runtime: { stage: "spawn" } }, "Inconnue"],
  ]) {
    const summary = summarize(data);
    assert.ok(summary.includes(`Étape : ${label}`));
    assert.ok(summary.includes("Effet sur l’exécution : non confirmé"));
    assert.ok(!summary.includes(canary));
    if (data.version === 2) assert.ok(summary.includes("Phase : Inconnue"));
  }
  assert.equal(eventSummary({
    ...brokerFailureEvent,
    kind: "broker.batch.failure.extra",
    data: { version: 1, enforcer: { stage: "register" }, runtime: { stage: "spawn" } },
  }), "broker.batch.failure.extra");
});
