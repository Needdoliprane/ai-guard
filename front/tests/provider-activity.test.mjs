import assert from "node:assert/strict";
import test from "node:test";
import {
  activityChannel,
  activityDetails,
  eventSummary,
  formatActivityCoverageFlag,
  projectEvents,
  providerActivitySummary,
} from "../src/model.ts";

const base = {
  id: "00000000-0000-8000-8000-000000000001",
  timestamp: "2026-11-21T12:00:00.000Z",
  source: "claude-adapter",
  provenance: "declared",
  coverage: "native",
};

test("provider messages are read as declarations with their channel", () => {
  const answer = {
    ...base,
    seq: 1,
    kind: "message.declared",
    data: { role: "assistant", text: "Tests pass.", text_truncated: true, activity: { channel: "stdout" } },
  };
  assert.equal(eventSummary(answer), "Réponse déclarée : Tests pass. (tronqué)");
  assert.equal(activityChannel(answer), "sortie structurée");
  const prompt = { ...answer, data: { role: "user", text: "Fix it", activity: { channel: "hook" } } };
  assert.equal(eventSummary(prompt), "Consigne déclarée : Fix it");
  assert.equal(activityChannel(prompt), "hook officiel");
  assert.equal(activityChannel({ ...base, seq: 3, kind: "runtime.started", data: {} }), null);
});

test("tool, delegation and agent declarations never claim an observed effect", () => {
  assert.equal(
    providerActivitySummary({ ...base, seq: 1, kind: "tool.requested", data: { tool_name: "Bash", command_text: "ls" } }),
    "Outil demandé · Bash : ls",
  );
  assert.equal(
    providerActivitySummary({
      ...base,
      seq: 2,
      kind: "tool.result",
      data: { tool_name: "command_execution" },
      result: { exit_code: 0 },
    }),
    "Résultat déclaré · command_execution · code 0",
  );
  assert.equal(
    providerActivitySummary({
      ...base,
      seq: 3,
      kind: "tool.result",
      data: { provider_tool_use_id: "toolu_4" },
      result: { is_error: true, error: "Exit code 1" },
    }),
    "Résultat déclaré en échec · toolu_4 : Exit code 1",
  );
  assert.equal(
    providerActivitySummary({
      ...base,
      seq: 4,
      kind: "delegation.declared",
      data: { delegation_tool: "Agent", agent_type: "Explore", description: "Find tests" },
    }),
    "Délégation déclarée · Explore : Find tests",
  );
  assert.equal(
    providerActivitySummary({
      ...base,
      seq: 5,
      kind: "agent.declared",
      data: { provider_agent_id: "a1", agent_role: "subagent", state: "started" },
    }),
    "Sous-agent déclaré a1 · started",
  );
  assert.equal(
    providerActivitySummary({
      ...base,
      seq: 6,
      kind: "plan.declared",
      data: { items: [{ text: "a", completed: true }, { text: "b", completed: false }] },
    }),
    "Plan déclaré · 1/2 étapes terminée",
  );
});

test("collection losses distinguish worker declarations from control observations", () => {
  const observed = {
    ...base,
    seq: 1,
    source: "runner",
    provenance: "observed",
    coverage: "observed",
    kind: "activity.loss",
    data: { reason: "session_budget_exhausted" },
  };
  assert.equal(eventSummary(observed), "Perte de collecte observée · session_budget_exhausted");
  assert.equal(
    eventSummary({ ...observed, provenance: "declared", data: { reason: "client_unrecorded" } }),
    "Perte déclarée par le worker · client_unrecorded",
  );
  assert.equal(
    eventSummary({ ...observed, kind: "activity.coverage", data: { complete: false } }),
    "Couverture de collecte live partielle : voir le détail",
  );
  assert.equal(
    eventSummary({ ...observed, kind: "activity.channel", data: { channel: "stdout", state: "stalled", reason: "audit_unavailable" } }),
    "Canal sortie structurée · stalled (audit_unavailable)",
  );
});

test("the actions view includes declared tool calls and delegations", () => {
  const events = [
    { ...base, seq: 1, kind: "tool.requested", data: {} },
    { ...base, seq: 2, kind: "delegation.declared", data: {} },
    { ...base, seq: 3, kind: "message.declared", data: { text: "x" } },
  ];
  assert.deepEqual(projectEvents(events, "actions").map((event) => event.seq), [1, 2]);
});

test("activity details separate the client's declaration from ai-guard's coverage facts", () => {
  const declared = activityDetails({
    ...base,
    seq: 1,
    kind: "tool.requested",
    data: {
      hook_input_truncated: true,
      activity: { channel: "hook", client: "codex", interactive: true, worker_id: "w", attempt_id: "a", received_at: "2026-11-21T12:00:01.000Z" },
    },
  });
  assert.equal(declared.declaredByClient, true);
  assert.equal(declared.truncated, true);
  assert.equal(declared.versionLabel, null);
  assert.equal(declared.interactive, true);
  assert.equal(declared.coverage, null);
  const summary = activityDetails({
    ...base,
    seq: 2,
    kind: "activity.coverage",
    source: "runner",
    provenance: "observed",
    coverage: "observed",
    data: {
      complete: false,
      budget_exhausted: true,
      audit_unavailable: false,
      stalled: false,
      spooled_events: 12,
      delivered_events: 12,
      hook_channel: "closed",
      stdout: { records: 5, mapped: 4, ignored: 1, dropped: 2 },
      hook: { records: 3, refused: 1, declared_losses: 2 },
      activity: { channel: null, client: "claude-code" },
    },
  });
  assert.equal(summary.declaredByClient, false);
  assert.equal(summary.coverage.complete, false);
  const { budgetExhausted, auditUnavailable, stalled } = summary.coverage;
  const flags = [budgetExhausted, auditUnavailable, stalled];
  assert.deepEqual(flags, [true, false, false]);
  assert.deepEqual(flags.map(formatActivityCoverageFlag), ["Oui", "Non", "Non"]);
  assert.equal(summary.coverage.stdout.dropped, 2);
  assert.equal(summary.coverage.hook.declaredLosses, 2);
  // A coverage claim from a provider source is never read as a control fact.
  const forged = activityDetails({ ...base, seq: 3, kind: "activity.coverage", data: { complete: true, activity: {} } });
  assert.equal(forged.coverage, null);
  assert.equal(activityDetails({ ...base, seq: 4, kind: "message.declared", data: {} }), null);
});

test("activity coverage displays unconfirmed outcomes as unknown", () => {
  const replay = {
    ...base,
    seq: 1,
    kind: "activity.coverage",
    source: "runner",
    provenance: "observed",
    coverage: "partial",
    data: {
      version: 1,
      complete: false,
      reason: "activity_finalization_unconfirmed",
      activity: { channel: null, client: "claude-code" },
    },
  };
  const coverage = activityDetails(replay).coverage;
  assert.equal(coverage.complete, false);
  const flags = [coverage.budgetExhausted, coverage.auditUnavailable, coverage.stalled];
  assert.deepEqual(flags, [null, null, null]);
  assert.deepEqual(flags.map(formatActivityCoverageFlag), ["Inconnu", "Inconnu", "Inconnu"]);
});
