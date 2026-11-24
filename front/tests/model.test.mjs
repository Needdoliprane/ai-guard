import assert from "node:assert/strict";
import { test } from "node:test";
import {
  buildAgentForest,
  brokerBatchResultDetails,
  brokerWorkspaceDetails,
  decisionEvaluationDetails,
  matchesFilters,
  projectEvents,
  formatMetric,
  eventSummary,
  mergeEventPage,
  overviewEvaluator,
  periodFilters,
  parseEntityCollection,
} from "../src/model.ts";

test("la vue d’ensemble distingue les évaluateurs configurés et leur joignabilité", () => {
  for (const [backend, reachability, name, detail] of [
    ["typesafe", "not_checked", "Jev réel", "Joignabilité non vérifiée"],
    ["rust_rules", "not_applicable", "Moteur Rust déterministe sans modèle", "Joignabilité sans objet"],
    ["local_openai", "not_checked", "Petit modèle local Jev-like", "Joignabilité non vérifiée"],
  ]) {
    assert.deepEqual(overviewEvaluator({ jev: "configured", jev_details: { backend, reachability } }),
      { name, state: "Configuré", detail });
  }
  assert.deepEqual(overviewEvaluator({ jev: "disabled", jev_details: null }),
    { name: null, state: "Désactivé", detail: null });
  assert.deepEqual(overviewEvaluator(undefined), { name: null, state: "Inconnu", detail: null });
  assert.deepEqual(overviewEvaluator({ jev: "configured" }), {
    name: null, state: "Configuré", detail: "Type et joignabilité non renseignés par le gateway",
  });
  assert.deepEqual(overviewEvaluator({ jev: "configured", jev_details: { backend: "future_engine" } }),
    { name: null, state: "Configuré", detail: "Type d’évaluateur inconnu" });
  assert.deepEqual(overviewEvaluator({ jev: "configured", jev_details: { backend: "typesafe", reachability: "future_status" } }),
    { name: "Jev réel", state: "Configuré", detail: "Joignabilité inconnue" });
});

test("une liste d’identités invalide ne devient pas une absence de liens", () => {
  assert.deepEqual(parseEntityCollection({ items: [] }), []);
  const agent = { id: "agent", session_id: "session", sandbox_id: null, provenance: "declared" };
  assert.deepEqual(parseEntityCollection({ items: [agent] }), [agent]);
  for (const invalid of [null, {}, { items: null }, { items: [null] }, { items: [{ name: "agent" }] },
    { items: [{ id: "" }] }, { items: [{ id: "agent", session_id: 42 }] },
    { items: [{ id: "sandbox", sandbox_id: { id: "other" } }] }])
    assert.throws(() => parseEntityCollection(invalid), /Contrat de liste/);
});

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

const observedDecision = {
  seq: 21, id: "decision-21", timestamp: "2026-11-18T12:00:00Z",
  kind: "security.decision", source: "gateway", provenance: "observed",
  decision: { engine: "deterministic", outcome: "allow", rule: "exact_allowlist" },
};

const withEvaluation = (jev, event = observedDecision) => ({ ...event, data: { jev } });

test("la décision distingue l’évaluateur Rust et le modèle demandé et retourné par TypeSafe", () => {
  assert.deepEqual(decisionEvaluationDetails(withEvaluation({
    backend: "rust_rules", status: "evaluated", outcome: "allow", requested_model: "invalide", model: "invalide",
  })), {
    evaluator: "Rust déterministe", evaluationState: "Évaluée", selectionSource: null, requestedModel: null,
    returnedModel: null, advice: null, localAdvisory: false,
  });
  assert.deepEqual(decisionEvaluationDetails(withEvaluation({
    backend: "typesafe", status: "evaluated", requested_model: "jev-latest", model: "jev-1.14.0",
    outcome: "allow", confidence: 0.95, provider_outcome: "deny",
  })), {
    evaluator: "Jev TypeSafe", evaluationState: "Évaluée", selectionSource: null, requestedModel: "jev-latest",
    returnedModel: "jev-1.14.0", advice: "Autoriser", localAdvisory: false,
  });
});

test("la sélection de l’évaluateur affiche seulement les provenances attestées", () => {
  const evaluation = { backend: "typesafe", status: "evaluated" };
  for (const [selection_source, label] of [
    ["cli", "Ligne de commande"],
    ["environment", "Variables d’environnement"],
    ["legacy", "Configuration historique"],
    ["programmatic", "Configuration du gateway"],
    [undefined, null], [null, null], ["CLI", null], ["inconnu", null], [["cli"], null],
  ]) {
    assert.equal(decisionEvaluationDetails(withEvaluation({ ...evaluation, selection_source })).selectionSource, label);
  }
  for (const status of ["skipped_local_policy", "unavailable_or_invalid"])
    assert.equal(decisionEvaluationDetails(withEvaluation({ ...evaluation, status, selection_source: "cli" })).selectionSource,
      "Ligne de commande");
});

test("l’avis local conserve le verdict brut du modèle à côté du refus effectif", () => {
  const event = withEvaluation({
    backend: "local_openai", status: "evaluated", requested_model: "local-small", model: "local-revision",
    provider_outcome: "allow", outcome: "review", qualification: "not_qualified_to_authorize",
  }, { ...observedDecision, decision: { engine: "local_openai", outcome: "deny", rule: "local_openai_review_fail_closed" } });
  assert.deepEqual(decisionEvaluationDetails(event), {
    evaluator: "Modèle local", evaluationState: "Évaluée", selectionSource: null, requestedModel: "local-small",
    returnedModel: "local-revision", advice: "Autoriser", localAdvisory: true,
  });
  assert.equal(event.decision.outcome, "deny");
  assert.equal(decisionEvaluationDetails(withEvaluation({
    backend: "local_openai", status: "evaluated", outcome: "review",
  })).advice, null);
});

test("une panne et une règle locale qui saute l’évaluation gardent des états différents", () => {
  for (const [status, evaluationState] of [
    ["unavailable_or_invalid", "Indisponible ou invalide"],
    ["skipped_local_policy", "Non sollicitée par la politique locale"],
  ]) {
    assert.deepEqual(decisionEvaluationDetails(withEvaluation({
      backend: "local_openai", status, requested_model: "local-small", model: "local-revision",
      provider_outcome: "allow", reason: "ne pas projeter", error: "ne pas projeter",
    })), {
      evaluator: "Modèle local", evaluationState, selectionSource: null, requestedModel: "local-small",
      returnedModel: null, advice: null, localAdvisory: true,
    });
  }
});

test("les décisions TypeSafe historiques restent lisibles à partir de leur contrat explicite", () => {
  const answer = { outcome: "allow", confidence: 1, model: "jev-1.14.0", usage: null };
  assert.deepEqual(decisionEvaluationDetails(withEvaluation(answer)), {
    evaluator: "Jev TypeSafe", evaluationState: "Évaluée", selectionSource: null, requestedModel: null,
    returnedModel: "jev-1.14.0", advice: "Autoriser", localAdvisory: false,
  });
  const failure = { status: "unavailable_or_invalid" };
  assert.equal(decisionEvaluationDetails(withEvaluation(failure)), null);
  assert.equal(decisionEvaluationDetails(withEvaluation(failure, {
    ...observedDecision, decision: { engine: "jev", outcome: "deny", rule: "jev_unavailable_or_invalid" },
  })).evaluationState, "Indisponible ou invalide");
  for (const malformed of [
    null, { model: "jev-old", outcome: "allow" }, { ...answer, confidence: -1 }, { ...answer, backend: null },
    { ...answer, outcome: "deny" },
    { ...answer, backend: "futur", status: "evaluated" }, { ...answer, backend: "typesafe", status: "configured" },
  ]) assert.equal(decisionEvaluationDetails(withEvaluation(malformed)), null);
  assert.equal(decisionEvaluationDetails(withEvaluation(answer, { ...observedDecision, decision: null })), null);
  assert.deepEqual(decisionEvaluationDetails(withEvaluation({ ...answer, outcome: "review", model: "jev\u0085old" }, {
    ...observedDecision, decision: { engine: "jev", outcome: "deny", rule: "jev_review_fail_closed" },
  })), {
    evaluator: "Jev TypeSafe", evaluationState: "Évaluée", selectionSource: null, requestedModel: null,
    returnedModel: null, advice: "Demander une revue", localAdvisory: false,
  });
});

test("seule l’évaluation gateway observée et bornée apporte des champs au détail", () => {
  const evidence = { backend: "typesafe", status: "evaluated", requested_model: "é".repeat(65),
    model: "modèle\nretourné", outcome: "unknown" };
  const event = withEvaluation(evidence);
  assert.deepEqual(decisionEvaluationDetails(event), {
    evaluator: "Jev TypeSafe", evaluationState: "Évaluée", selectionSource: null, requestedModel: null,
    returnedModel: null, advice: null, localAdvisory: false,
  });
  for (const update of [{ kind: "command.completed" }, { source: "runner" }, { provenance: "declared" }])
    assert.equal(decisionEvaluationDetails({ ...event, ...update }), null);
});

const brokerBatchResult = {
  ...brokerFailureEvent, kind: "broker.batch.result", command: null, result: null,
  data: {
    version: 1,
    observation: { process: {
      status: "stopped", running: false, exit_code: 0, oom_killed: null,
      started_at: "2026-10-29T12:00:00Z", finished_at: "2026-10-29T12:00:01Z",
    } },
    transport: null,
  },
};

test("une observation terminale du broker reste dans Actions même sans commande récupérée", () => {
  assert.deepEqual(projectEvents([brokerBatchResult], "actions"), [brokerBatchResult]);
  const result = brokerBatchResultDetails(brokerBatchResult);
  assert.equal(result.observedExitCode, 0);
  assert.equal(result.oomKilled, null);
  assert.equal(result.transportExitCode, null);
  assert.equal(result.transportDurationMs, null);
  assert.equal(result.stdout.state, "unknown");
  assert.equal(result.stderr.state, "unknown");
});

test("le code et la durée du transport restent distincts de l’observation et de la capture", () => {
  const transport = {
    exit_code: -23, duration_ms_decimal: "340282366920938463463374607431768211455",
    stdout: { total_bytes: 0, text: "", truncated: false, text_clipped: false },
    stderr: { total_bytes: 48, text: null, truncated: true, text_clipped: null },
  };
  const result = brokerBatchResultDetails({
    ...brokerBatchResult, data: { ...brokerBatchResult.data, transport },
  });
  assert.equal(result.observedExitCode, 0);
  assert.equal(result.transportExitCode, -23);
  assert.equal(result.transportDurationMs, transport.duration_ms_decimal);
  assert.deepEqual(result.stdout, { state: "retained", text: "", truncated: false, textClipped: false });
  assert.deepEqual(result.stderr, { state: "not_retained", text: null, truncated: true, textClipped: false });
});

test("le résultat broker invalide laisse inconnus les champs et borne le texte conservé", () => {
  const text = "é".repeat(16384) + "…[truncated]";
  const event = {
    ...brokerBatchResult, result: { exit_code: 137, timed_out: true },
    data: { version: 1,
      observation: { process: { ...brokerBatchResult.data.observation.process, running: true, oom_killed: true } },
      transport: { exit_code: 2147483648, duration_ms_decimal: "340282366920938463463374607431768211456",
        stdout: { text, truncated: false, text_clipped: true }, stderr: { text: text + "é", text_clipped: true } },
    },
  };
  const result = brokerBatchResultDetails(event);
  assert.equal(result.observedExitCode, null);
  assert.equal(result.oomKilled, null);
  assert.equal(result.transportExitCode, null);
  assert.equal(result.transportDurationMs, null);
  assert.equal(result.stdout.text?.length, text.length);
  assert.deepEqual(result.stdout, { state: "retained", text, truncated: false, textClipped: true });
  assert.deepEqual(result.stderr, { state: "unknown", text: null, truncated: false, textClipped: false });
  assert.equal(brokerBatchResultDetails({ ...event, source: "provider" }), null);
  assert.equal(brokerBatchResultDetails({ ...event, provenance: "declared" }), null);
  assert.equal(brokerBatchResultDetails({ ...event, data: { ...event.data, version: 2 } }), null);
  assert.equal(brokerBatchResultDetails({ ...event, kind: "command.failed" }), null);
});

test("un échec broker v1 présente sa phase et son étape sans confirmer un effet d’exécution", () => {
  for (const [step, label] of [
    ["activity_spool", "Collecte d’activité fournisseur indisponible"],
    ["full3_binding_publication", "Enregistrement de la liaison au gardien"],
    ["preclaim_choice", "Vérification du droit de poursuivre le démarrage"],
    ["native_claim_reserve", "Réservation du démarrage auprès du gardien"],
    ["native_claim_consume", "Contrôle de la réservation auprès du gardien"],
    ["failure_observed", "Observation d’un échec"],
    ["ownership_reload", "Vérification du conteneur enregistré"],
    ["created_exchange", "Confirmation de la création"],
    ["created_exchange_reload", "Relecture du journal pour la création"],
    ["created_exchange_prepare_pending", "Préparation de la demande de confirmation"],
    ["created_exchange_advance_pending", "Enregistrement de la demande de confirmation"],
    ["terminal_observation_ack", "Acquittement de l’observation terminale"],
    ["result_audit", "Audit du résultat"],
    ["cleanup_confirmation", "Confirmation du retrait du worker"],
    ["cleanup_ack", "Enregistrement du résultat du retrait"],
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

test("un échec de liaison Full3 affiche un libellé fermé et préserve les anciens reçus", () => {
  for (const [stage, label] of [
    ["read_native_binding", "Lecture de la liaison au gardien"],
    ["reload_journal", "Relecture du journal avant publication de la liaison"],
    ["advance_writer", "Publication de la liaison dans le journal"],
    ["retry_limit", "Conflits répétés à la publication de la liaison"],
    ["synthetic-private-stage", "Enregistrement de la liaison au gardien"],
  ]) {
    const summary = eventSummary({
      ...brokerFailureEvent,
      data: {
        ...brokerFailureEvent.data,
        broker_step: "full3_binding_publication",
        full3_binding: { stage },
      },
    });
    assert.ok(summary.includes(`Étape : ${label}`));
    assert.ok(summary.includes("Effet sur l’exécution : non confirmé"));
    assert.ok(!summary.includes("synthetic-private-stage"));
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

const scanDigest = "a".repeat(64);
const exportDigest = "b".repeat(64);
const beforeDigest = "c".repeat(64);
const afterDigest = "d".repeat(64);
const exportRequest = "81cfa3ec-3e98-4152-9dea-b89203618b8c";
const brokerScan = {
  ...brokerFailureEvent,
  kind: "broker.batch.scan",
  coverage: "observed",
  data: {
    version: 1,
    manifest_digest: scanDigest,
    changed_files: 3,
    truncated: false,
    changes: [
      { path: "src/nouveau.rs", change: "created", before: null, after: { sha256: afterDigest, bytes: 12 } },
      { path: "modifié.txt", change: "modified", before: { sha256: beforeDigest }, after: { sha256: afterDigest } },
      { path: "ancien.txt", change: "deleted", before: { sha256: beforeDigest }, after: null },
    ],
  },
};
const brokerExport = {
  ...brokerFailureEvent,
  seq: 6,
  kind: "broker.batch.export",
  coverage: "observed",
  data: {
    version: 1,
    manifest_digest: exportDigest,
    request_id: exportRequest,
    files: 2,
    destination_ref: "/private/reports/workspace",
    destination_intent_hash: "e".repeat(64),
  },
};

test("Fichiers montre les événements broker exacts et préserve leur lecture brute", () => {
  const legacy = { ...brokerScan, kind: "workspace.diff" };
  const unknownVersion = { ...brokerScan, data: { ...brokerScan.data, version: 2 } };
  const unrelated = { ...brokerScan, kind: "broker.batch.scan.extra" };
  const events = [legacy, brokerScan, brokerExport, unknownVersion, unrelated];
  const files = projectEvents(events, "files");
  assert.deepEqual(files, [legacy, brokerScan, brokerExport, unknownVersion]);
  assert.equal(files[3], unknownVersion);
  assert.equal(brokerWorkspaceDetails(unknownVersion), null);
  assert.equal(brokerWorkspaceDetails(unrelated), null);
  assert.equal(eventSummary(unknownVersion), "broker.batch.scan");
  const oneChange = { ...brokerScan, data: { ...brokerScan.data, changed_files: 1, changes: brokerScan.data.changes.slice(0, 1) } };
  assert.equal(eventSummary(oneChange), "1 modification observée");
  assert.equal(eventSummary(brokerExport), "2 fichiers exportés");
});

test("l’analyse broker distingue création, modification et suppression avec leurs empreintes", () => {
  assert.deepEqual(brokerWorkspaceDetails(brokerScan), {
    kind: "scan", changedFiles: 3, manifestDigest: scanDigest, truncated: false,
    changes: [
      { path: "src/nouveau.rs", change: "created", beforeSha256: null, afterSha256: afterDigest },
      { path: "modifié.txt", change: "modified", beforeSha256: beforeDigest, afterSha256: afterDigest },
      { path: "ancien.txt", change: "deleted", beforeSha256: beforeDigest, afterSha256: null },
    ],
  });
  const partial = { ...brokerScan, data: { ...brokerScan.data, changes: brokerScan.data.changes.slice(0, 1), truncated: true } };
  assert.equal(brokerWorkspaceDetails(partial).changedFiles, 3);
  assert.equal(brokerWorkspaceDetails(partial).changes.length, 1);
  const empty = { ...brokerScan, data: { ...brokerScan.data, changed_files: 0, changes: [] } };
  assert.equal(brokerWorkspaceDetails(empty).changedFiles, 0);
});

test("l’export broker projette son propre manifeste, sans la destination", () => {
  assert.deepEqual(brokerWorkspaceDetails(brokerExport), {
    kind: "export", files: 2, manifestDigest: exportDigest, requestId: exportRequest,
  });
  const empty = { ...brokerExport, data: { ...brokerExport.data, files: 0 } };
  assert.equal(brokerWorkspaceDetails(empty).files, 0);
  for (const event of [
    { ...brokerExport, provenance: "declared" },
    { ...brokerExport, source: "gateway" },
    ...[
      { files: undefined }, { files: -1 }, { files: 20_001 }, { files: "2" }, { version: "1" },
      { request_id: "00000000-0000-0000-0000-000000000000" },
      { request_id: exportRequest.toUpperCase() },
      { manifest_digest: undefined, scan_manifest_digest: scanDigest },
      { manifest_digest: `${exportDigest}\n` },
    ].map((data) => ({ ...brokerExport, data: { ...brokerExport.data, ...data } })),
  ]) assert.equal(brokerWorkspaceDetails(event), null);
});

test("une analyse incohérente ou hors des bornes ne produit pas de détail structuré", () => {
  const first = brokerScan.data.changes[0];
  for (const data of [
    { changed_files: undefined }, { changed_files: 40_001 }, { truncated: true },
    { truncated: "false" }, { changes: brokerScan.data.changes.slice(1) },
    { changed_files: 201, changes: Array(201).fill(first) },
    ...[
      { path: "/absolute" }, { path: "src/../elsewhere" }, { path: "src\\other" }, { path: "é".repeat(2049) },
      { change: "renamed" }, { after: null }, { before: undefined }, { after: { sha256: "inconnu" } },
    ].map((row) => ({ changes: [{ ...first, ...row }, ...brokerScan.data.changes.slice(1)] })),
  ]) assert.equal(brokerWorkspaceDetails({ ...brokerScan, data: { ...brokerScan.data, ...data } }), null);
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
    ["runtime", "observe_only", "Démarrage non émis lors de cette tentative"],
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
