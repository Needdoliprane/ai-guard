// Reproduce the frontend checks against HTTP responses captured by the Rust integration tests.
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const suites = [
  {
    target: "federation_workers_api",
    oracle: "federation-wire",
    selectors: [
      "federation_projects_workers_and_historical_brokers_in_authenticated_tenant",
      "federation_keeps_real_legacy_workers_with_explicit_registry_migration_limit",
    ],
  },
  {
    target: "human_workers_api",
    oracle: "human-workers-wire",
    selectors: [
      "legacy_canonical_nil_session_remains_readable_after_a_real_admission",
      "legacy_uninterpretable_runtime_date_is_typed_unknown_instead_of_failing_the_page",
      "legacy_large_exit_code_is_an_exact_decimal_in_the_human_wire_contract",
    ],
  },
  {
    target: "supervision_stack_api",
    oracle: "supervision-wire",
    selectors: [
      "admin_reads_one_hundred_workers_and_all_brokers_in_one_composition",
      "session_events_follow_resource_grants_for_admin_owner_and_stranger",
      "tenants_are_isolated_and_an_empty_registry_is_a_successful_read",
    ],
  },
  {
    target: "human_recent_events",
    oracle: "human-recent",
    selectors: [
      "recent_returns_newest_authorized_events_first",
      "recent_refuses_foreign_resource_other_tenant_and_reused_cursors",
      "a_gateway_without_the_route_answers_the_event_not_found_contract",
    ],
  },
];
const testPattern = "^(the frontend consumes actual modern|the real newest-first|RAW |the real composed pages|a real empty registry|real resources and session events)";
const usage = `Usage: node front/tests/run_rust_wire.mjs [all|supervision]

  all          Run all four Rust suites and eight Node checks (default).
  supervision  Run three supervision_stack_api tests and supervision-wire.test.mjs.

Requires Node 24, the repository Rust toolchain and an offline Cargo cache.
The Rust test servers listen on loopback and use synthetic SQLite data.
Private response files are retained under the system temporary directory.`;

function privateDirectory(prefix) {
  const directory = mkdtempSync(prefix);
  chmodSync(directory, 0o700);
  return directory;
}

function run(command, args, env) {
  const { status, signal, error } = spawnSync(command, args, { cwd: root, env, stdio: "inherit" });
  if (error) throw error;
  if (signal) console.error(`${command} terminated by ${signal}`);
  return status ?? 1;
}

function runWire(mode) {
  const selected = mode === "all" ? suites : suites.filter(({ target }) => target === "supervision_stack_api");
  // Resolve the macOS /var alias before passing paths to the Rust filesystem checks.
  const captures = privateDirectory(join(realpathSync(tmpdir()), "aig-rust-wire-"));
  console.error(`Private wire captures: ${captures}`);
  const env = {
    ...process.env,
    AIGUARD_FEDERATION_WIRE_DIR: privateDirectory(join(captures, "federation-")),
    AIGUARD_HUMAN_WORKERS_WIRE_DIR: privateDirectory(join(captures, "human-workers-")),
    AIGUARD_SUPERVISION_WIRE_DIR: privateDirectory(join(captures, "supervision-")),
  };

  for (const { target, selectors } of selected) {
    const status = run("cargo", ["test", "--locked", "--offline", "-j1", "--test", target,
      "--", "--exact", "--test-threads=1", ...selectors], env);
    if (status !== 0) return status;
  }
  return run(process.execPath, ["--test", "--test-isolation=none", "--test-reporter=tap",
    `--test-name-pattern=${testPattern}`, ...selected.map(({ oracle }) => `front/tests/${oracle}.test.mjs`)], env);
}

const [mode = "all", ...extraArgs] = process.argv.slice(2);
if (mode === "--help" && extraArgs.length === 0) {
  console.log(usage);
} else if (extraArgs.length || (mode !== "all" && mode !== "supervision")) {
  console.error(usage);
  process.exitCode = 2;
} else {
  try {
    process.exitCode = runWire(mode);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
