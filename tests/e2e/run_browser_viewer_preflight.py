#!/usr/bin/env python3
"""Check the viewer preflight against fresh Rust services on loopback."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
from tempfile import TemporaryDirectory

from run_browser_decision import BROWSER_ENV, sha256, tenant_config
from run_jev_probe import ROOT, ProbeError, gateway, require, write_private


def run(cli, frontend, node, evidence, report):
    driver = ROOT / "front/tests/browser_provider_live.mjs"
    with TemporaryDirectory(dir=evidence) as folder:
        work = Path(folder)
        tenant, _, viewer = tenant_config(work, "tenant-viewer-preflight")
        registry = work / "tenants.json"
        write_private(registry, json.dumps({"version": 1, "tenants": [tenant]}).encode())
        gateway_command = [
            cli, "gateway", "--listen", "127.0.0.1:0", "--tenants-file", registry,
            "--policy", ROOT / "policies/default.json", "--jev-backend", "rust-rules",
        ]
        with gateway(gateway_command, work) as gateway_port:
            origin = f"http://127.0.0.1:{gateway_port}"
            dashboard_command = [
                cli, "dashboard", "--listen", "127.0.0.1:0", "--upstream", origin,
                "--front-dir", frontend, "--viewer-token-file", tenant["viewer_token_file"],
            ]
            with gateway(dashboard_command, work, service="Dashboard") as dashboard_port:
                report["stage"] = "browser"
                env = {name: os.environ[name] for name in BROWSER_ENV if name in os.environ}
                env.update(
                    PATH=os.defpath, LANG="C", HOME=folder, TMPDIR=folder,
                    AI_GUARD_TEST_GATEWAY=origin,
                    AI_GUARD_TEST_DASHBOARD=f"http://127.0.0.1:{dashboard_port}",
                    AI_GUARD_TEST_CONFIG=str(work / "tenant-viewer-preflight"),
                    AI_GUARD_TEST_ARTIFACT_DIR=str(evidence),
                )
                child = subprocess.run(
                    [node, driver, "--preflight"], cwd=work, env=env,
                    capture_output=True, timeout=90,
                )
                require(viewer.encode() not in child.stdout, "viewer marker in browser report")
                browser = json.loads(child.stdout)
                require(isinstance(browser, dict), "browser report is not a JSON object")
                write_private(evidence / "browser.json", child.stdout)
                report["browser_report"] = "browser.json"
                if child.returncode == 2 and browser.get("status") == "BLOCKED":
                    report["status"] = "BLOCKED"
                    raise ProbeError("browser preflight reported an unavailable prerequisite")
                require(
                    child.returncode == 0 and browser.get("status") == "PASS"
                    and browser.get("mode") == "preflight"
                    and browser.get("checks") == ["services HTTP", "Playwright et Chromium", "identité viewer"],
                    "viewer preflight did not verify both services and the dashboard",
                )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--front-dir", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    args = parser.parse_args()
    evidence = args.evidence_dir.absolute()
    try:
        evidence.mkdir(mode=0o700)
    except OSError:
        parser.error("evidence directory must be new and have a writable parent")

    report = {"status": "BLOCKED", "stage": "prerequisites"}
    try:
        cli, frontend = args.cli.resolve(), args.front_dir.resolve()
        node = shutil.which("node")
        require(cli.is_file() and os.access(cli, os.X_OK), "Rust CLI is unavailable")
        require((frontend / "index.html").is_file(), "compiled frontend is unavailable")
        assets = sorted((frontend / "assets").glob("*.js"))
        require(assets, "compiled frontend JavaScript is unavailable")
        require(node, "Node is unavailable")
        report.update(status="FAIL", stage="services", cli_sha256=sha256(cli),
                      html_sha256=sha256(frontend / "index.html"),
                      bundle_js_sha256={asset.name: sha256(asset) for asset in assets})
        run(cli, frontend, node, evidence, report)
    except ProbeError as error:
        report["diagnostic"] = str(error)
    except (OSError, ValueError, subprocess.SubprocessError):
        report["diagnostic"] = "could not complete the local viewer check"
    else:
        report.update(status="PASS", stage="complete")
    encoded = json.dumps(report, indent=2) + "\n"
    write_private(evidence / "result.json", encoded.encode())
    print(encoded, end="")
    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2}[report["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
