import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const driver = fileURLToPath(new URL("./browser_provider_live.mjs", import.meta.url));
const rejectHttp = "data:text/javascript," + encodeURIComponent(`
  globalThis.fetch = () => {
    process.stderr.write("unexpected HTTP in viewer preflight fixture");
    throw new Error("fixture forbids HTTP");
  };
`);

for (const [label, config] of [["missing", undefined], ["blank", "  "]]) {
  test(`provider browser preflight blocks a ${label} viewer configuration`, () => {
    const env = {
      AI_GUARD_TEST_GATEWAY: "http://127.0.0.1:1",
      AI_GUARD_TEST_DASHBOARD: "http://127.0.0.1:2",
    };
    if (config !== undefined) env.AI_GUARD_TEST_CONFIG = config;
    const child = spawnSync(process.execPath, ["--import", rejectHttp, driver, "--preflight"], {
      env, encoding: "utf8", timeout: 10_000,
    });

    assert.ifError(child.error);
    assert.equal(child.status, 2);
    const report = JSON.parse(child.stdout);
    assert.equal(report.status, "BLOCKED");
    assert.equal(report.stage, "configuration");
    assert.match(report.message, /AI_GUARD_TEST_CONFIG/);
    assert.deepEqual(report.checks, []);
    assert.equal(child.stderr, "");
  });
}
