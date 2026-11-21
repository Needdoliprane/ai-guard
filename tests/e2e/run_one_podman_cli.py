"""One local Podman CLI submission with independent Store and export oracles."""

import argparse
from collections import Counter
from hashlib import file_digest
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import tempfile
import time
from uuid import UUID


if not __debug__:
    raise SystemExit("Run this oracle with Python assertions enabled")

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--cli", type=Path, required=True)
parser.add_argument("--cli-sha256", required=True)
parser.add_argument("--policy", type=Path, required=True)
parser.add_argument("--podman", type=Path, required=True)
parser.add_argument("--endpoint", required=True)
parser.add_argument("--image", required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--allow-rootful", action="store_true")
args = parser.parse_args()

os.umask(0o077)
assert Path(args.endpoint.removeprefix("unix://")).is_socket()
assert args.endpoint.startswith("unix:///")
assert re.fullmatch(r"sha256:[a-f0-9]{64}", args.image)
assert args.cli.is_absolute() and args.podman.is_absolute()
assert args.policy.is_absolute() and args.policy.is_file()
with args.cli.open("rb") as body:
    assert file_digest(body, "sha256").hexdigest() == args.cli_sha256
qa = args.output
assert qa.is_absolute()
qa.mkdir(mode=0o700)
qa = qa.resolve(strict=True)
tmp = tempfile.mkdtemp(prefix="aig-podcli-", dir="/private/tmp")
env = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "LANG": "C", "TMPDIR": tmp}
control = qa / "control"
project = qa / "project"
project.mkdir(mode=0o700)
originals = {"original.txt": b"original unchanged\n", ".env": b"PRIVATE_TEST_VALUE=never-export\n"}
for name, contents in originals.items():
    (project / name).write_bytes(contents)
results = {"stage": "preflight", "cli_sha256": args.cli_sha256, "codes": {}, "seconds": {}}
gateway = None


def run(name, command, timeout):
    start = time.monotonic()
    with (qa / (name + ".stdout")).open("xb") as out, (qa / (name + ".stderr")).open("xb") as err:
        try:
            code = subprocess.run(command, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=err,
                                  timeout=timeout, check=False).returncode
        except subprocess.TimeoutExpired:
            code = 124
    results["codes"][name] = code
    results["seconds"][name] = round(time.monotonic() - start, 2)
    return code


def snapshot(db):
    # WAL must be respected while the owned gateway runs.
    with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True, timeout=2) as read:
        read.execute("BEGIN")
        rows = read.execute(
            "SELECT w.worker_id,w.session_id,w.state,w.quota_held,w.snapshot_json,"
            "a.attempt_id,a.container_id,a.start_operation_id "
            "FROM workers w JOIN worker_attempts a USING(worker_id)"
        ).fetchall()
        if len(rows) != 1:
            return None
        worker, session, state, quota, encoded_worker, attempt, cid, claim = rows[0]
        worker_view = json.loads(encoded_worker)
        assert str(UUID(session)) == session
        events = read.execute("SELECT kind,payload FROM events WHERE sandbox_id=? ORDER BY seq", (session,)).fetchall()
        allocations = read.execute(
            "SELECT attempt_id,payload_json,event_seq FROM worker_allocations WHERE worker_id=?", (worker,)
        ).fetchall()
    return {"worker": worker, "session": session, "state": state, "quota": quota, "attempt": attempt, "cid": cid,
            "claim": claim, "spec_hash": worker_view["spec_hash"], "cleanup_removed": worker_view["cleanup_removed"],
            "events": events, "allocations": allocations}


try:
    podman = [args.podman, "--remote", "--url", args.endpoint]
    assert run("image", [*podman, "image", "inspect", "--format", "{{.Id}} {{.Config.User}}", args.image], 15) == 0
    image_id, image_user = (qa / "image.stdout").read_text().split()
    assert image_id.removeprefix("sha256:") == args.image.removeprefix("sha256:") and image_user == "1000:1000"
    assert run("active", [*podman, "ps", "--filter", "ancestor=" + args.image, "--quiet"], 15) == 0
    assert not (qa / "active.stdout").read_text().strip()
    runtime = ["--runtime", "podman", "--runtime-program", args.podman, "--runtime-endpoint", args.endpoint]
    if args.allow_rootful:
        runtime.append("--allow-rootful-runtime")
    assert run("doctor", [args.cli, "doctor", *runtime], 30) == 0
    assert run("init", [args.cli, "init", "--config-dir", control], 15) == 0
    db = control / "audit.sqlite"
    with (qa / "gateway.stdout").open("xb") as out, (qa / "gateway.stderr").open("xb") as err:
        gateway = subprocess.Popen(
            [args.cli, "gateway", "--listen", "127.0.0.1:0", "--data", db,
             "--runner-token-file", control / "secrets/runner", "--viewer-token-file", control / "secrets/viewer",
             "--policy", args.policy, "--worker-limit", "1", "--jev-backend", "rust-rules"],
            env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=err,
        )
        limit = time.monotonic() + 25
        while time.monotonic() < limit:
            assert gateway.poll() is None
            announced = " ".join(path.read_text()[-32768:] for path in (qa / "gateway.stdout", qa / "gateway.stderr"))
            match = re.search(r"Gateway 127\.0\.0\.1:([0-9]{1,5})\b", announced)
            if match:
                port = int(match.group(1))
                http = HTTPConnection("127.0.0.1", port, timeout=1)
                try:
                    http.request("GET", "/healthz")
                    if http.getresponse().status == 200:
                        break
                except OSError:
                    pass
                finally:
                    http.close()
            time.sleep(0.1)
        else:
            raise RuntimeError("owned gateway readiness expired")

    origin = f"http://127.0.0.1:{port}"
    gateway_opts = ["--config-dir", control, "--gateway", origin]
    results["stage"] = "single_run"
    rc = run("run", [args.cli, "run", *gateway_opts, "--project", project, *runtime, "--image", args.image,
                     "--profile", "strict", "--network", "none", "--provider", "generic", "--timeout", "30",
                     "--capture-output", "--", "/usr/bin/id"], 320)
    before = snapshot(db)
    results["stage"] = "registry"
    assert before is not None, "expected exactly one owned worker"
    assert (control / "sessions" / (before["session"] + ".json")).is_file()
    session_opts = ["--session", before["session"], *gateway_opts]
    assert run("status", [args.cli, "worker-status", *session_opts], 30) == 0
    results["worker_state"] = before["state"]
    if rc != 124 and before["state"] == "terminal" and before["claim"] and before["quota"] == 0:
        if rc != 0:
            run("resume", [args.cli, "worker-resume", *session_opts, "--runtime-program", args.podman], 130)
        export = qa / "export"
        assert not export.exists()
        results["stage"] = "export"
        assert run("export", [args.cli, "export-workspace", *session_opts, "--runtime-program", args.podman,
                              "--output", export], 160) == 0
        assert (export / "original.txt").read_bytes() == originals["original.txt"]
        assert not (export / ".env").exists()

    assert all((project / name).read_bytes() == contents for name, contents in originals.items())
    assert run("audit", [args.cli, "verify-audit", *gateway_opts], 40) == 0
    audit = json.loads((qa / "audit.stdout").read_text())
    after = snapshot(db)
    assert after is not None
    assert all(before[key] == after[key] for key in ("worker", "session", "attempt", "cid", "claim",
                                                       "spec_hash", "allocations"))
    counts = Counter(kind for kind, _ in after["events"])
    initial_start = [payload for kind, payload in before["events"] if kind == "command.started"]
    final_start = [payload for kind, payload in after["events"] if kind == "command.started"]
    same_audited_start = len(initial_start) == 1 and initial_start == final_start
    completed = [json.loads(payload)["result"] for kind, payload in after["events"] if kind == "command.completed"]
    uid_observed = (len(completed) == 1 and completed[0].get("exit_code") == 0
                    and bool(re.search(r"(?:^|\s)uid=1000(?:\(|\s|$)", completed[0].get("stdout") or "")))
    allocation = json.loads(after["allocations"][0][1]) if len(after["allocations"]) == 1 else {}
    allocation_source = allocation.get("source", {})
    allocation_valid = (allocation.get("attempt_id") == after["attempt"] and allocation.get("worker_id") == after["worker"]
                        and allocation.get("billable") is False and allocation_source.get("provenance") == "observed"
                        and allocation_source.get("runtime_kind") == "podman"
                        and allocation_source.get("container_id") == after["cid"]
                        and allocation_source.get("spec_hash") == after["spec_hash"]
                        and int(allocation.get("duration_ns") or 0) > 0)
    removed = False
    if after["cid"] and re.fullmatch(r"[a-f0-9]{64}", after["cid"]):
        removed = run("container_exists", [*podman, "container", "exists", after["cid"]], 15) == 1
    results.update(stage="complete", counts={key: counts[key] for key in
                   ("command.started", "command.completed", "workspace.export.completed")},
                   allocation_count=len(after["allocations"]), allocation_valid=allocation_valid,
                   uid_observed=uid_observed, audit_valid=audit["valid"] is True,
                   recovery_export=results["codes"].get("export") == 0,
                    source_unchanged=True, same_audited_start=same_audited_start,
                    container_removed=removed and after["cleanup_removed"] is True)
    results["status"] = "PASS" if (rc == 0 and results["recovery_export"] and results["audit_valid"]
                                    and uid_observed and allocation_valid
                                    and same_audited_start and results["container_removed"]
                                    and all(count == 1 for count in results["counts"].values())) else "FAIL"
finally:
    if gateway is not None and gateway.poll() is None:
        gateway.terminate()
        try:
            gateway.wait(timeout=5)
        except subprocess.TimeoutExpired:
            gateway.kill()
            gateway.wait(timeout=5)
    results.setdefault("status", "BLOCKED" if results["stage"] == "preflight" else "FAIL")
    (qa / "result.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results), flush=True)

raise SystemExit(0 if results["status"] == "PASS" else 1)
