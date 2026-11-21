#!/usr/bin/env python3
"""Replay the live Jev decision and SQLite restart through the Rust gateway."""

import argparse
from contextlib import contextmanager
import hashlib
from http.client import HTTPConnection, HTTPException
import json
import os
from pathlib import Path
import re
import secrets
import selectors
import stat
import subprocess
import tempfile
from time import monotonic
from uuid import uuid4


ROOT = Path(__file__).resolve().parents[2]
JEV_URL = "https://api.typesafe.ai/v1/systemone"
MODEL = "jev-latest"
MIN_CONFIDENCE = 0.85
HOST = "127.0.0.1"
MAX_RESPONSE = 262_144
STARTUP_TIMEOUT = 15
HTTP_TIMEOUT = 10
SHUTDOWN_TIMEOUT = 8
HEAD_HASH = re.compile(r"[0-9a-f]{64}")


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def private_credential_path(path):
    require(not path.name.startswith(".env"), "use a dedicated raw Jev key file")
    path = path.absolute()
    info = path.lstat()
    require(
        stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid()
        and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1,
        "the Jev key must be an owner-only regular file with one link and mode 0600",
    )
    return path


def write_private(path, content):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as private:
        private.write(content)


def gateway_port(process, service):
    ready = re.compile(rb"(?m)^" + re.escape(service.encode("ascii")) + rb" 127\.0\.0\.1:([0-9]{1,5}) ")
    with selectors.DefaultSelector() as selector:
        selector.register(process.stderr, selectors.EVENT_READ)
        deadline = monotonic() + STARTUP_TIMEOUT
        captured = b""
        while events := selector.select(max(0, deadline - monotonic())):
            chunk = os.read(events[0][0].fd, 4096)
            if not chunk:
                break
            captured += chunk
            match = ready.search(captured)
            if match and 0 < int(match[1]) <= 65535:
                return int(match[1])
            if len(captured) >= 8192 or monotonic() >= deadline:
                break
    raise ProbeError(f"{service} did not report readiness on a local ephemeral port")


@contextmanager
def gateway(command, directory, *, service="Gateway"):
    # Supply the endpoint explicitly and keep ambient provider and proxy options out.
    environment = {"PATH": os.defpath, "HOME": os.fspath(directory), "LANG": "C"}
    with subprocess.Popen(
        command, cwd=directory, env=environment,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    ) as process:
        exceeded_deadline = False
        try:
            yield gateway_port(process, service)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=SHUTDOWN_TIMEOUT)
                except subprocess.TimeoutExpired:
                    exceeded_deadline = True
                    process.kill()
                    process.wait(timeout=SHUTDOWN_TIMEOUT)
        require(not exceeded_deadline, f"{service} shutdown exceeded the deadline")
        require(process.returncode == 0, f"{service} did not shut down cleanly")


def request(port, path, token, body=None):
    connection = HTTPConnection(HOST, port, timeout=HTTP_TIMEOUT)
    try:
        headers = {"Authorization": "Bearer " + token, "Content-Type": "application/json"}
        connection.request("GET" if body is None else "POST", path, json.dumps(body) if body else None, headers)
        with connection.getresponse() as response:
            require(response.status == 200, f"gateway returned HTTP {response.status}")
            raw = response.read(MAX_RESPONSE + 1)
        require(len(raw) <= MAX_RESPONSE, "gateway response exceeded the size limit")
        result = json.loads(raw)
        require(isinstance(result, dict), "gateway response was not a JSON object")
        return result
    finally:
        connection.close()


def audit_snapshot(port, viewer):
    audit = request(port, "/api/v1/integrity", viewer)
    require(audit.get("valid") is True, "gateway reported an invalid audit")
    require(type(audit.get("event_count")) is int, "gateway omitted the event count")
    head = audit.get("head_hash")
    require(isinstance(head, str) and HEAD_HASH.fullmatch(head), "gateway omitted the audit head hash")
    workers = request(port, "/api/v1/workers?limit=1", viewer)
    require(workers.get("items") == [] and workers.get("next_cursor") is None, "worker registry must remain empty")
    return audit["event_count"], head


def audit_decision(port, viewer, answer, command, request_id):
    sequence = answer.get("audit_seq")
    require(type(sequence) is int and sequence > 0, "decision omitted its audit sequence")
    receipt = answer.get("receipt")
    require(
        isinstance(receipt, dict)
        and receipt.get("version") == 1
        and receipt.get("request_id") == request_id
        and receipt.get("phase") == "initial"
        and receipt.get("audit_seq") == sequence
        and receipt.get("audit_hash") == answer.get("audit_hash")
        and isinstance(answer.get("request_hash"), str)
        and HEAD_HASH.fullmatch(answer["request_hash"])
        and receipt.get("request_hash") == answer["request_hash"],
        "decision receipt does not match the request and audit",
    )
    events = request(port, "/api/v1/events?kind=security.decision&limit=2", viewer).get("items")
    require(isinstance(events, list) and len(events) == 1, "audit must contain one decision")
    event = events[0]
    require(
        isinstance(event, dict)
        and event.get("kind") == "security.decision"
        and event.get("source") == "gateway"
        and event.get("provenance") == "observed"
        and event.get("seq") == sequence
        and event.get("hash") == answer.get("audit_hash")
        and event.get("command") == command
        and event.get("decision") == answer.get("decision")
        and isinstance(event.get("data"), dict)
        and event["data"].get("request_id") == request_id
        and event["data"].get("request_hash") == answer["request_hash"]
        and event["data"].get("jev") == answer.get("jev"),
        "audited decision does not match the returned receipt",
    )
    return event


def run(binary, key_file, result):
    runner, viewer = (secrets.token_hex(32) for _ in range(2))
    with tempfile.TemporaryDirectory(prefix="ai-guard-jev-") as directory:
        work = Path(directory).resolve()
        write_private(work / "runner", runner.encode())
        write_private(work / "viewer", viewer.encode())
        common = [
            binary, "gateway", "--listen", f"{HOST}:0", "--data", work / "audit.sqlite",
            "--runner-token-file", work / "runner", "--viewer-token-file", work / "viewer",
            "--policy", ROOT / "policies" / "default.json",
        ]
        live = [
            *common, "--jev-backend", "typesafe", "--jev-url", JEV_URL,
            "--jev-model", MODEL, "--jev-key-file", key_file,
            "--jev-min-confidence", str(MIN_CONFIDENCE),
        ]
        command = {"argv": ["/bin/echo", "sandbox-ok"], "cwd": "/workspace"}
        request_id = str(uuid4())
        payload = {"request_id": request_id, "profile": "strict", "command": command}
        result["stage"] = "startup"
        with gateway(live, work) as port:
            before, _ = audit_snapshot(port, viewer)
            result["stage"] = "decision"
            answer = request(port, "/api/v1/decide", runner, payload)
            result["stage"] = "audit"
            after, head = audit_snapshot(port, viewer)
            require(after == before + 1 and answer.get("audit_hash") == head, "decision is not the new audit head")
            event = audit_decision(port, viewer, answer, command, request_id)
        result["stage"] = "restart"
        # Reopen without Jev. The exact retry must return the existing decision.
        with gateway(common, work) as port:
            require(request(port, "/api/v1/decide", runner, payload) == answer,
                    "replay changed the committed decision after restart")
            require(audit_snapshot(port, viewer) == (after, head), "audit head changed after restart")
            require(audit_decision(port, viewer, answer, command, request_id) == event, "decision changed after restart")
        result.update(
            audit_seq=answer["audit_seq"], event_count=after, audit_head=head,
            audit_valid_after_restart=True, gateway_reopened=True, decision_replayed=True, workers=0,
        )
        result["stage"] = "verdict"
        provider = answer.get("jev")
        require(
            isinstance(provider, dict)
            and provider.get("backend") == "typesafe"
            and provider.get("selection_source") == "cli"
            and provider.get("status") == "evaluated"
            and provider.get("requested_model") == MODEL,
            "audit does not establish the evaluated TypeSafe backend and requested model",
        )
        configuration_digest = provider.get("config_sha256")
        require(isinstance(configuration_digest, str) and HEAD_HASH.fullmatch(configuration_digest),
                "gateway omitted the TypeSafe configuration fingerprint")
        returned_model = provider.get("model")
        require(
            isinstance(returned_model, str)
            and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/@-]{0,127}", returned_model)
            and all(secret not in returned_model for secret in (runner, viewer)),
            "provider model could not be established",
        )
        choice = provider.get("outcome")
        require(choice in ("allow", "deny", "review"), "provider choice could not be established")
        confidence = provider.get("confidence")
        require(type(confidence) in (int, float) and MIN_CONFIDENCE <= confidence <= 1,
                "provider confidence outside the threshold")
        require(provider.get("confidence_provenance") == "provider_declared_not_calibrated",
                "gateway omitted the provider confidence provenance")
        result.update(
            requested_model=provider["requested_model"], returned_model=returned_model, provider_choice=choice,
            backend="typesafe", selection_source="cli", configuration_sha256=configuration_digest,
            confidence=confidence, confidence_provenance=provider["confidence_provenance"],
        )
        expected = {"outcome": "allow", "engine": "deterministic", "rule": "exact_allowlist"}
        decision = answer.get("decision")
        require(
            choice == "allow" and isinstance(decision, dict)
            and all(decision.get(name) == value for name, value in expected.items()),
            "the local allowlisted sample was not confirmed",
        )
        result.update(
            decision_outcome=expected["outcome"], decision_engine=expected["engine"], decision_rule=expected["rule"],
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", required=True, type=Path, help="agentsandbox executable")
    parser.add_argument("--key-file", required=True, type=Path, help="dedicated raw Jev key file with mode 0600")
    args = parser.parse_args()
    report = {"status": "BLOCKED", "stage": "preflight", "requested_model": MODEL}
    try:
        binary = args.bin.resolve()
        require(binary.is_file() and os.access(binary, os.X_OK), "agentsandbox executable is unavailable")
        key_file = private_credential_path(args.key_file)
        with binary.open("rb") as executable:
            report["cli_sha256"] = hashlib.file_digest(executable, "sha256").hexdigest()
        report["status"] = "FAIL"
        run(binary, key_file, report)
    except ProbeError as failure:
        report["diagnostic"] = str(failure)
    except (OSError, HTTPException, ValueError, subprocess.SubprocessError):
        report["diagnostic"] = "probe could not read its inputs, start the gateway, or complete local HTTP"
    else:
        report.update(status="PASS", stage="complete")
    print(json.dumps(report, indent=2))
    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2}[report["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
