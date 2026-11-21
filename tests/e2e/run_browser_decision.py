#!/usr/bin/env python3
"""Drive a real Rust gateway, dashboard, and Chromium with two fresh tenants."""

import argparse
from contextlib import contextmanager, nullcontext
import hashlib
from http.client import HTTPConnection, HTTPException
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
from tempfile import TemporaryDirectory
from threading import Thread
from time import monotonic_ns
from uuid import uuid4

from run_jev_probe import (
    HEAD_HASH, JEV_URL, MAX_RESPONSE, MIN_CONFIDENCE, MODEL as TYPESAFE_REQUESTED_MODEL,
    ROOT, ProbeError, audit_snapshot, gateway, private_credential_path, request, require, write_private,
)


QWEN_MODEL = "qwen2.5-0.5b-q4km"
QWEN_MAX_TOKENS = 256
TENANT_A = "tenant-a"
TENANT_B = "tenant-b"
BROWSER_ENV = ("CODEX_PRIMARY_RUNTIME_NODE_MODULES", "PLAYWRIGHT_CHROMIUM_EXECUTABLE", "PLAYWRIGHT_BROWSERS_PATH")
TYPESAFE_RETURNED_MODEL = "jev-fixture-version"
TYPESAFE_REPLAY_MODES = ("typesafe-real", "typesafe-replay-fixture")
TYPESAFE_RULES = {
    "allow": ("allow", "deterministic", "exact_allowlist"),
    "deny": ("deny", "jev", "jev_deny"),
    "review": ("deny", "jev", "jev_review_fail_closed"),
}
FIXTURE_USAGE = {"prompt_tokens": 82, "completion_tokens": 29, "total_tokens": 111}
REAL_RECIPE_HTTP_TIMEOUT = 14
DEFAULT_MODEL_TIMEOUT = 5


def model_timeout_seconds(value):
    try:
        seconds = int(value)
    except ValueError:
        seconds = 0
    if not 1 <= seconds <= 12:
        raise argparse.ArgumentTypeError("expected a model timeout from 1 to 12 seconds")
    return seconds


@contextmanager
def model_endpoint(mode, credential, model_port=None, fixture_outcome=None, *, relay_timeline=None):
    calls = []
    is_typesafe = mode in ("typesafe-fixture", "typesafe-replay-fixture")
    endpoint = "/v1/systemone" if is_typesafe else "/v1/chat/completions"
    auth = "Bearer " + credential if is_typesafe else None

    class LocalModel(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            started = monotonic_ns() if mode == "local-real" and relay_timeline is not None else None
            try:
                self.connection.settimeout(3)
                size = int(self.headers.get("Content-Length", "0"))
                if (self.path != endpoint or not 0 < size <= 16384 or
                        self.headers.get("Authorization") != auth):
                    raise ValueError
                body = self.rfile.read(size)
                payload = json.loads(body)
                if is_typesafe:
                    state = payload["state"]
                    valid = (payload["model"] == TYPESAFE_REQUESTED_MODEL
                             and payload["questions"]["execution_policy"]["type"] == "choice"
                             and self.headers.get("TypeSafe-Danger-Zone-Long-Context") is None)
                else:
                    state = json.loads(payload["messages"][1]["content"])
                    valid = payload["model"] == QWEN_MODEL and payload["max_tokens"] == QWEN_MAX_TOKENS and payload["stream"] is False
                valid = valid and state["command"]["argv"] == ["/usr/bin/id"]
            except (KeyError, IndexError, TypeError, ValueError, OSError):
                valid = False
            calls.append(valid)
            if model_port is not None:
                if not valid or len(calls) != 1:
                    self.send_error(503)
                    return
                self.forward_once(body, started)
                return
            if not valid or len(calls) > 2:
                self.send_error(400)
                return
            if (not is_typesafe and len(calls) == 2) or fixture_outcome == "unavailable":
                self.send_error(503)
                return
            result = {
                "model": TYPESAFE_RETURNED_MODEL, "answers": {"execution_policy": {
                    "type": "choice", "choice": fixture_outcome or ("allow" if len(calls) == 1 else "deny"), "confidence": 0.99,
                }}, "usage": {"input_tokens": 82, "output_tokens": 29},
            } if is_typesafe else {
                "model": QWEN_MODEL, "choices": [{
                    "index": 0, "finish_reason": "stop", "message": {
                        "role": "assistant", "content": json.dumps({"outcome": "allow", "reason": "Avis de recette"}),
                    },
                }],
                "usage": {**FIXTURE_USAGE, "prompt_tokens_details": {"cached_tokens": 0}},
            }
            response = json.dumps(result).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(response)))
            self.end_headers()
            self.wfile.write(response)

        def forward_once(self, body, started):
            # Both sides are IPv4 loopback. Preserve the validated request and provider response;
            # refuse retries, redirects and subsequent decisions before reaching the real model.
            def observe(stage, **facts):
                if started is not None:
                    relay_timeline.append({"stage": stage, "elapsed_ms": (monotonic_ns() - started) // 1_000_000, **facts})

            observe("validated", request_bytes=len(body), request_limit_bytes=16_384)
            connection = HTTPConnection("127.0.0.1", model_port, timeout=REAL_RECIPE_HTTP_TIMEOUT)
            phase = "upstream_request"
            try:
                connection.request("POST", endpoint, body, {
                    "Content-Type": "application/json", "Accept": "application/json",
                })
                observe("upstream_request_sent")
                phase = "response_headers"
                with connection.getresponse() as upstream:
                    status = upstream.status
                    observe("response_headers", http_status=status)
                    phase = "response_body"
                    response = upstream.read(65_537)
                if len(response) > 65_536:
                    observe("error", phase=phase, kind="body_limit", response_limit_bytes=65_536)
                    self.send_error(502)
                    return
                observe("bounded_body", body_bytes=len(response), response_limit_bytes=65_536)
                if 300 <= status < 400:
                    observe("error", phase=phase, kind="redirect_refused")
                    self.send_error(502)
                    return
            except (HTTPException, OSError) as error:
                observe("error", phase=phase, kind="timeout" if isinstance(error, TimeoutError) else "transport_error")
                self.send_error(502)
                return
            finally:
                connection.close()
            try:
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(response)))
                self.end_headers()
                self.wfile.write(response)
            except (HTTPException, OSError) as error:
                observe("error", phase="downstream_write", kind="timeout" if isinstance(error, TimeoutError) else "transport_error")
                raise
            observe("downstream_written")

    with HTTPServer(("127.0.0.1", 0), LocalModel) as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield server.server_port, calls
        finally:
            server.shutdown()
            worker.join(timeout=5)
            require(not worker.is_alive(), "local model fixture did not stop")


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def tenant_config(directory, tenant):
    root = directory / tenant
    root.mkdir(mode=0o700)
    (root / "data").mkdir(mode=0o700)
    (root / "secrets").mkdir(mode=0o700)
    runner, viewer = (secrets.token_hex(32) for _ in range(2))
    write_private(root / "secrets/runner", runner.encode())
    write_private(root / "secrets/viewer", viewer.encode())
    return {
        "tenant_id": tenant, "data": str(root / "data/audit.sqlite"),
        "runner_token_file": str(root / "secrets/runner"),
        "viewer_token_file": str(root / "secrets/viewer"),
    }, runner, viewer


def verify_expected_answer(decision, provider, outcome, rule, backend, backend_status):
    engine = "deterministic"
    if backend_status != "skipped_local_policy":
        if backend == "local_openai":
            engine = "local_openai"
        elif backend == "typesafe" and outcome == "deny":
            engine = "jev"
    require(isinstance(decision, dict) and decision.get("engine") == engine
            and decision.get("outcome") == outcome and decision.get("rule") == rule,
            "gateway decision differs from the expected effective policy")
    require(isinstance(provider, dict) and provider.get("backend") == backend
            and provider.get("selection_source") == "cli" and provider.get("status") == backend_status,
            "gateway omitted the selected backend or its provenance")
    if backend == "local_openai":
        require(provider.get("requested_model") == QWEN_MODEL, "the selected model is missing")
        if backend_status == "evaluated":
            require(provider.get("provider_outcome") == "allow" and provider.get("outcome") == "review"
                    and provider.get("model") == QWEN_MODEL and provider.get("usage") == FIXTURE_USAGE
                    and provider.get("qualification") == "not_qualified_to_authorize"
                    and provider.get("coverage") == "top_level_advisory_only",
                    "the fixture advice or its limited qualification is missing")
        elif backend_status == "unavailable_or_invalid":
            require(provider.get("model") is None and provider.get("diagnostic", {}).get("http_status") == 503,
                    "the backend outage does not report the fixture HTTP refusal")
    elif backend == "typesafe":
        require(provider.get("requested_model") == TYPESAFE_REQUESTED_MODEL, "the selected Jev model is missing")
        if backend_status == "evaluated":
            require(provider.get("model") == TYPESAFE_RETURNED_MODEL and provider.get("outcome") == outcome
                    and provider.get("usage_provenance") == "provider_declared"
                    and provider.get("confidence_provenance") == "provider_declared_not_calibrated",
                    "the audit omitted the returned Jev model, advice, or usage provenance")
        else:
            require(provider.get("model") is None and "outcome" not in provider,
                    "an uncalled Jev endpoint must not attest advice or a returned model")


def request_real_decision(port, runner, payload):
    # The probe helper's 10-second HTTP timeout is shorter than the admissible
    # 12-second gateway deadline. Leave that helper unchanged for other reads.
    connection = HTTPConnection("127.0.0.1", port, timeout=REAL_RECIPE_HTTP_TIMEOUT)
    try:
        connection.request("POST", "/api/v1/decide", json.dumps(payload), {
            "Authorization": "Bearer " + runner, "Content-Type": "application/json",
        })
        with connection.getresponse() as response:
            require(response.status == 200, f"gateway returned HTTP {response.status}")
            raw = response.read(MAX_RESPONSE + 1)
        require(len(raw) <= MAX_RESPONSE, "gateway response exceeded the size limit")
        answer = json.loads(raw)
        require(isinstance(answer, dict), "gateway response was not a JSON object")
        return answer
    finally:
        connection.close()


def read_decision_audit(port, viewer, payload, answer, before):
    decision, provider, receipt = (answer.get(key) for key in ("decision", "jev", "receipt"))
    request_id = payload["request_id"]
    seq, digest = answer.get("audit_seq"), answer.get("audit_hash")
    require(type(seq) is int and isinstance(digest, str) and HEAD_HASH.fullmatch(digest)
            and isinstance(receipt, dict) and receipt.get("request_id") == request_id
            and receipt.get("audit_seq") == seq and receipt.get("audit_hash") == digest,
            "gateway receipt does not match the request and audit coordinates")
    require(audit_snapshot(port, viewer) == (before + 1, digest), "decision is not the new audit head")
    events = request(port, f"/api/v1/events?kind=security.decision&after={seq - 1}&limit=2&order=asc", viewer).get("items")
    require(isinstance(events, list) and len(events) == 1, "one durable decision expected for the session")
    event = events[0]
    require(event.get("seq") == seq and event.get("hash") == digest and event.get("session_id") == payload["session_id"]
            and event.get("source") == "gateway" and event.get("provenance") == "observed"
            and event.get("command") == payload["command"] and event.get("decision") == decision
            and event.get("data", {}).get("request_id") == request_id and event["data"].get("jev") == provider,
            "durable decision does not match the receipt and request")
    return event


def decision_payload(argv):
    return {"profile": "strict", "command": {"argv": argv, "cwd": "/workspace"},
            "request_id": str(uuid4()), "session_id": str(uuid4())}


def decision_proof(event, backend_status):
    decision = event["decision"]
    return {"seq": event["seq"], "id": event["id"], "hash": event["hash"],
            "request_id": event["data"]["request_id"], "session_id": event["session_id"],
            "outcome": decision["outcome"], "rule": decision["rule"], "backend_status": backend_status}


def record_qwen_verdict(decision, provider, report):
    # Keep only the bounded decision fields used by the evidence report.
    if not isinstance(decision, dict) or not isinstance(provider, dict):
        return
    for key, allowed in (
        ("outcome", ("allow", "deny", "review")),
        ("engine", ("local_openai", "deterministic")),
        ("rule", ("local_openai_deny", "local_openai_review_fail_closed",
                  "decision_backend_unavailable_or_invalid", "strict_default_deny", "exact_allowlist")),
    ):
        if decision.get(key) in allowed:
            report[f"decision_{key}"] = decision[key]
    for key, name, allowed in (
        ("backend", "backend", ("local_openai",)),
        ("status", "backend_status", ("evaluated", "skipped_local_policy", "unavailable_or_invalid")),
        ("selection_source", "selection_source", ("cli",)),
        ("requested_model", "requested_model", (QWEN_MODEL,)),
        ("model", "returned_model", (QWEN_MODEL,)),
        ("provider_outcome", "provider_outcome", ("allow", "deny", "review", "unknown")),
        ("outcome", "advisory_outcome", ("deny", "review")),
        ("error", "backend_error", ("invalid_configuration", "request_too_large",
                                    "response_too_large", "transport_unavailable", "invalid_response")),
    ):
        if provider.get(key) in allowed:
            report[name] = provider[key]
    diagnostic = provider.get("diagnostic")
    if (isinstance(diagnostic, dict) and diagnostic.get("hop") == "configured_endpoint"
            and diagnostic.get("phase") in ("send", "response_body")
            and diagnostic.get("kind") in ("timeout", "connection_error", "http_status", "transport_error")):
        evidence = {key: diagnostic[key] for key in ("hop", "phase", "kind")}
        status = diagnostic.get("http_status")
        if type(status) is int and 100 <= status <= 599:
            evidence["http_status"] = status
        report["transport_diagnostic"] = evidence


def verify_real_qwen(decision, provider, report, timeout_seconds):
    require(isinstance(decision, dict) and isinstance(provider, dict), "gateway decision or advice is missing")
    configuration = provider.get("configuration")
    transport = configuration.get("transport") if isinstance(configuration, dict) else None
    require(isinstance(transport, dict) and type(transport.get("timeout_seconds")) is int
            and transport["timeout_seconds"] == timeout_seconds,
            "the audited model deadline differs from the selected deadline")
    report["model_timeout_seconds"] = transport["timeout_seconds"]
    fingerprint = provider.get("config_sha256")
    require(configuration.get("requested_model") == QWEN_MODEL and configuration.get("max_tokens") == QWEN_MAX_TOKENS
            and configuration.get("stream") is False and configuration.get("response_format") == {"type": "json_object"}
            and transport.get("endpoint") == f"http://127.0.0.1:{report['model_relay_port']}/v1/chat/completions"
            and transport.get("credential_present") is False and transport.get("automatic_retry") is False
            and transport.get("allow_loopback_http") is True
            and isinstance(fingerprint, str) and HEAD_HASH.fullmatch(fingerprint),
            "the audited Qwen setup differs from the recipe")
    report["configuration_sha256"] = fingerprint
    require(provider.get("backend") == "local_openai" and provider.get("selection_source") == "cli"
            and provider.get("requested_model") == QWEN_MODEL, "the audited Qwen identity differs from the recipe")
    if provider.get("status") == "unavailable_or_invalid":
        backend_error = report.get("backend_error")
        diagnostic = report.get("transport_diagnostic")
        require(decision.get("outcome") == "deny" and decision.get("engine") == "local_openai"
                and decision.get("rule") == "decision_backend_unavailable_or_invalid"
                and all(field in provider and provider[field] is None for field in ("model", "confidence", "usage"))
                and "outcome" not in provider and "provider_outcome" not in provider
                and backend_error is not None and provider.get("error") == backend_error
                and provider.get("diagnostic") == diagnostic
                and (backend_error == "transport_unavailable") == (diagnostic is not None),
                "an unusable Qwen response did not produce the expected fail-closed decision")
        return {"provider_outcome": None, "advisory_outcome": None, "returned_model": None,
                "configuration_sha256": fingerprint, "model_timeout_seconds": timeout_seconds,
                "backend_error": backend_error, "transport_diagnostic": diagnostic}
    require(provider.get("status") == "evaluated" and provider.get("model") == QWEN_MODEL,
            "Qwen did not return a usable answer")
    choice = provider.get("provider_outcome")
    require(choice in ("allow", "deny", "review", "unknown"), "Qwen advice is missing or invalid")
    advisory = "deny" if choice == "deny" else "review"
    rule = "local_openai_deny" if advisory == "deny" else "local_openai_review_fail_closed"
    require(decision.get("outcome") == "deny" and decision.get("engine") == "local_openai"
            and decision.get("rule") == rule and provider.get("outcome") == advisory
            and provider.get("qualification") == "not_qualified_to_authorize"
            and provider.get("coverage") == "top_level_advisory_only" and "confidence" in provider
            and provider["confidence"] is None and provider.get("usage_provenance") == "provider_declared",
            "the effective decision or recorded Qwen provenance differs from the accepted advice")
    usage = provider.get("usage")
    fields = ("prompt_tokens", "completion_tokens", "total_tokens")
    require(isinstance(usage, dict) and set(usage) == set(fields)
            and all(type(usage[key]) is int and 0 < usage[key] <= (1 << 53) - 1 for key in fields)
            and usage["completion_tokens"] <= QWEN_MAX_TOKENS
            and usage["prompt_tokens"] + usage["completion_tokens"] == usage["total_tokens"],
            "Qwen usage differs from the bounded recorded counters")
    report["usage"] = usage
    return {"provider_outcome": choice, "advisory_outcome": advisory, "configuration_sha256": fingerprint, "usage": usage}


def recorded_decision(port, runner, viewer, argv, outcome=None, rule=None, backend_status="evaluated", *,
                      backend="rust_rules", real_report=None, real_timeout=DEFAULT_MODEL_TIMEOUT):
    payload = decision_payload(argv)
    before, _ = audit_snapshot(port, viewer)
    answer = (request(port, "/api/v1/decide", runner, payload) if real_report is None
              else request_real_decision(port, runner, payload))
    decision, provider = (answer.get(key) for key in ("decision", "jev"))
    if real_report is None:
        verify_expected_answer(decision, provider, outcome, rule, backend, backend_status)
    else:
        record_qwen_verdict(decision, provider, real_report)
    event = read_decision_audit(port, viewer, payload, answer, before)
    require(request(port, "/api/v1/decide", runner, payload) == answer
            and audit_snapshot(port, viewer) == (before + 1, event["hash"]), "exact replay changed the receipt or audit")
    evidence = {}
    if real_report is not None:
        real_report.update(audit_seq=event["seq"], audit_hash=event["hash"], audit_event_id=event["id"], decision_replayed=True)
        evidence = verify_real_qwen(decision, provider, real_report, real_timeout)
        backend_status = provider["status"]
    return {**decision_proof(event, backend_status), **evidence}


def typesafe_proof(event, endpoint):
    provider = event["data"]["jev"]
    require(isinstance(provider, dict) and provider.get("backend") == "typesafe"
            and provider.get("selection_source") == "cli" and provider.get("requested_model") == TYPESAFE_REQUESTED_MODEL,
            "the audit omitted the selected TypeSafe backend")
    configuration = provider.get("configuration")
    transport = configuration.get("transport") if isinstance(configuration, dict) else None
    fingerprint = provider.get("config_sha256")
    require(isinstance(transport, dict) and transport.get("endpoint") == endpoint
            and transport.get("automatic_retry") is False
            and transport.get("credential_present") is True
            and transport.get("allow_loopback_http") is (endpoint != JEV_URL)
            and isinstance(fingerprint, str) and HEAD_HASH.fullmatch(fingerprint),
            "the recorded TypeSafe transport differs from the selected endpoint")
    decision = event["decision"]
    result = tuple(decision.get(field) for field in ("outcome", "engine", "rule")) if isinstance(decision, dict) else None
    status = provider.get("status")
    choice, model = provider.get("outcome"), provider.get("model")
    if status == "unavailable_or_invalid":
        require(result == ("deny", "jev", "jev_unavailable_or_invalid") and model is None and choice is None,
                "unaccepted TypeSafe response did not produce a durable fail-closed decision")
    else:
        confidence = provider.get("confidence")
        require(status == "evaluated" and isinstance(choice, str) and choice in TYPESAFE_RULES and result == TYPESAFE_RULES[choice]
                and isinstance(model, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/@-]{0,127}", model)
                and type(confidence) in (int, float) and MIN_CONFIDENCE <= confidence <= 1
                and provider.get("confidence_provenance") == "provider_declared_not_calibrated",
                "the durable decision differs from the accepted TypeSafe advice")
    return {**decision_proof(event, status), "provider_outcome": choice, "returned_model": model,
            "configuration_sha256": fingerprint}


def record_typesafe_once(command, directory, runner, viewer, endpoint):
    payload = decision_payload(["/usr/bin/id"])
    with gateway(command, directory) as port:
        before, _ = audit_snapshot(port, viewer)
        answer = request(port, "/api/v1/decide", runner, payload)
        event = read_decision_audit(port, viewer, payload, answer, before)
    return payload, answer, event, before, typesafe_proof(event, endpoint)


def verify_from_cli(cli, config, port, viewer, directory):
    output = subprocess.run(
        [cli, "verify-audit", "--config-dir", config, "--gateway", f"http://127.0.0.1:{port}"],
        env={"PATH": os.defpath, "HOME": str(directory), "LANG": "C"},
        capture_output=True, timeout=15, cwd=directory,
    )
    require(output.returncode == 0, "verify-audit returned a failure")
    verdict = json.loads(output.stdout)
    event_count, head = audit_snapshot(port, viewer)
    require(verdict.get("valid") is True and verdict.get("event_count") == event_count
            and verdict.get("head_hash") == head, "CLI audit verdict differs from the gateway")
    return {"event_count": event_count, "head_hash": head}


def run(cli, front_dir, node, evidence, report, judge_mode, model_port=None, model_timeout=None, *,
        typesafe_key_file=None, fixture_outcome=None):
    typesafe_replay = judge_mode in TYPESAFE_REPLAY_MODES
    local_failure = False
    fixture_credential = secrets.token_hex(32) if judge_mode in ("typesafe-fixture", "typesafe-replay-fixture") else None
    relay_timeline = [] if judge_mode == "local-real" else None
    if relay_timeline is not None:
        # The server owns this list until its context joins the handler. A gateway
        # timeout may precede the upstream timeout, so retain the list by reference.
        report["model_relay_timeline"] = relay_timeline
    endpoint = (nullcontext(None) if judge_mode in ("rust", "typesafe-real")
                else model_endpoint(judge_mode, fixture_credential, model_port, fixture_outcome,
                                    relay_timeline=relay_timeline))
    with TemporaryDirectory(prefix="ai-guard-browser-decision-") as temporary, endpoint as local:
        directory = Path(temporary).resolve()
        first, runner_a, viewer_a = tenant_config(directory, TENANT_A)
        second, runner_b, viewer_b = tenant_config(directory, TENANT_B)
        tenants = directory / "tenants.json"
        write_private(tenants, json.dumps({"version": 1, "tenants": [first, second]}).encode())
        policy = ROOT / "policies/default.json"
        report["stage"] = "gateway_startup"
        rust_backend = ["--jev-backend", "rust-rules"]
        model_url = None
        if judge_mode == "typesafe-real":
            model_url = JEV_URL
            backend = ["--jev-backend", "typesafe", "--jev-model", TYPESAFE_REQUESTED_MODEL,
                       "--jev-key-file", typesafe_key_file, "--jev-url", model_url,
                       "--jev-min-confidence", str(MIN_CONFIDENCE)]
        elif local is None:
            backend = rust_backend
        elif fixture_credential is None:
            backend = ["--jev-backend", "local-openai", "--jev-local-model", QWEN_MODEL,
                       "--jev-local-url", f"http://127.0.0.1:{local[0]}/v1/chat/completions", "--jev-allow-local-http"]
            if model_timeout is not None:
                backend.extend(["--jev-local-timeout-seconds", str(model_timeout)])
        else:
            key_file = directory / "typesafe-fixture-key"
            write_private(key_file, fixture_credential.encode())
            model_url = f"http://127.0.0.1:{local[0]}/v1/systemone"
            backend = ["--jev-backend", "typesafe", "--jev-model", TYPESAFE_REQUESTED_MODEL, "--jev-key-file", key_file,
                       "--jev-url", model_url, "--jev-allow-local-http"]
        gateway_command = [cli, "gateway", "--listen", "127.0.0.1:0", "--tenants-file", tenants, "--policy", policy]
        if typesafe_replay:
            report["stage"] = "typesafe_initial_decision"
            payload, answer, original, before, advisory = record_typesafe_once(
                [*gateway_command, *backend], directory, runner_a, viewer_a, model_url,
            )
            report.update(backend="typesafe", selection_source="cli", requested_model=TYPESAFE_REQUESTED_MODEL,
                          returned_model=advisory["returned_model"], provider_outcome=advisory["provider_outcome"],
                          backend_status=advisory["backend_status"], configuration_sha256=advisory["configuration_sha256"],
                          audit_seq=advisory["seq"], audit_event_id=advisory["id"], audit_hash=advisory["hash"],
                          decision_outcome=advisory["outcome"], decision_rule=advisory["rule"], typesafe_gateway_stopped=True)
            backend = rust_backend
        with gateway([*gateway_command, *backend], directory) as port:
            report["stage"] = "durable_decisions"
            if typesafe_replay:
                health = request(port, "/api/overview", viewer_a).get("health")
                details = health.get("jev_details") if isinstance(health, dict) else None
                require(isinstance(details, dict) and details.get("backend") == "rust_rules"
                        and details.get("selection_source") == "cli",
                        "replay requires the offline Rust backend")
                require(request(port, "/api/v1/decide", runner_a, payload) == answer
                        and read_decision_audit(port, viewer_a, payload, answer, before) == original,
                        "the reopened gateway changed the TypeSafe response or durable receipt")
                report["decision_replayed_without_typesafe"] = True
                deny = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/true"], "deny",
                                         "strict_default_deny", "skipped_local_policy")
                foreign = recorded_decision(port, runner_b, viewer_b, ["/usr/bin/true"], "deny",
                                            "strict_default_deny", "skipped_local_policy")
                if judge_mode == "typesafe-replay-fixture":
                    report["fixture_calls"] = len(local[1])
                    require(local[1] == [True], "the synthetic TypeSafe server must receive exactly one valid POST")
                    if fixture_outcome == "unavailable":
                        require(advisory["backend_status"] == "unavailable_or_invalid"
                                and original["data"]["jev"].get("diagnostic", {}).get("http_status") == 503,
                                "the synthetic TypeSafe outage was not recorded")
                    else:
                        require(advisory["backend_status"] == "evaluated" and advisory["provider_outcome"] == fixture_outcome
                                and advisory["returned_model"] == TYPESAFE_RETURNED_MODEL,
                                "the synthetic TypeSafe response differs from its configured advice")
                decisions = [advisory, deny]
            elif local is None:
                allow = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/id"], "allow", "exact_allowlist", "evaluated")
                deny = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/true"], "deny", "strict_default_deny", "skipped_local_policy")
                foreign = recorded_decision(port, runner_b, viewer_b, ["/usr/bin/id"], "allow", "exact_allowlist", "evaluated")
                decisions = [allow, deny]
            elif model_port is not None:
                health = request(port, "/api/overview", viewer_a).get("health")
                details = health.get("jev_details") if isinstance(health, dict) else None
                require(isinstance(details, dict) and details.get("backend") == "local_openai"
                        and details.get("selection_source") == "cli",
                        "gateway did not select the real local model backend")
                report.update(stage="real_qwen_decision", model_port=model_port, model_relay_port=local[0])
                try:
                    advisory = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/id"], real_report=report,
                                                 real_timeout=model_timeout if model_timeout is not None else DEFAULT_MODEL_TIMEOUT)
                    local_failure = advisory["backend_status"] == "unavailable_or_invalid"
                    if local_failure:
                        report["qwen_fail_closed_receipt_verified"] = True
                    deny = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/true"], "deny",
                                             "strict_default_deny", "skipped_local_policy", backend="local_openai")
                    foreign = recorded_decision(port, runner_b, viewer_b, ["/usr/bin/true"], "deny",
                                                "strict_default_deny", "skipped_local_policy", backend="local_openai")
                finally:
                    report["model_endpoint_posts"] = len(local[1])
                require(local[1] == [True], "exactly one validated Qwen POST was required; replay and veto must not invoke it")
                decisions = [advisory, deny]
            elif fixture_credential is None:
                evaluator = "local_openai"
                advisory = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/id"], "deny",
                                             "local_openai_review_fail_closed", "evaluated", backend=evaluator)
                require(local[1] == [True], "the advisory decision and its replay must make one local POST")
                deny = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/true"], "deny",
                                         "strict_default_deny", "skipped_local_policy", backend=evaluator)
                require(local[1] == [True], "a strict policy denial must leave the model uncalled")
                outage = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/id"], "deny",
                                           "decision_backend_unavailable_or_invalid", "unavailable_or_invalid", backend=evaluator)
                foreign = recorded_decision(port, runner_b, viewer_b, ["/usr/bin/true"], "deny",
                                            "strict_default_deny", "skipped_local_policy", backend=evaluator)
                require(local[1] == [True, True],
                        "the local fixture expected one evaluation and one HTTP failure, with no replay or policy-veto call")
                report["fixture_calls"] = len(local[1])
                decisions = [advisory, deny, outage]
            else:
                evaluator = "typesafe"
                allow = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/id"], "allow",
                                          "exact_allowlist", "evaluated", backend=evaluator)
                require(local[1] == [True], "a Jev decision and its replay must make one local POST")
                deny = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/true"], "deny",
                                         "strict_default_deny", "skipped_local_policy", backend=evaluator)
                require(local[1] == [True], "a strict policy denial must leave Jev uncalled")
                veto = recorded_decision(port, runner_a, viewer_a, ["/usr/bin/id"], "deny", "jev_deny",
                                         "evaluated", backend=evaluator)
                foreign = recorded_decision(port, runner_b, viewer_b, ["/usr/bin/true"], "deny",
                                            "strict_default_deny", "skipped_local_policy", backend=evaluator)
                require(local[1] == [True, True], "the two Jev evaluations must remain distinct from veto and replay")
                report["fixture_calls"] = len(local[1])
                decisions = [allow, deny, veto]
            proof = {"version": 1, "tenant_id": TENANT_A, "foreign_tenant_id": TENANT_B,
                     "judge_mode": judge_mode, "decisions": decisions, "foreign_decision": foreign}
            head_bound = model_port is not None or typesafe_replay
            if head_bound:
                proof["audit_head"] = decisions[-1]["hash"]
            proof_path = evidence / "decisions.json"
            write_private(proof_path, json.dumps(proof, indent=2).encode() + b"\n")
            report["stage"] = "cli_audit"
            report["cli_audit"] = verify_from_cli(cli, directory / TENANT_A, port, viewer_a, directory)
            if head_bound:
                require(report["cli_audit"]["head_hash"] == proof["audit_head"], "CLI did not verify the decision chain head")
            report["workers"] = 0
            report["stage"] = "dashboard_startup"
            dashboard_command = [cli, "dashboard", "--listen", "127.0.0.1:0", "--upstream", f"http://127.0.0.1:{port}",
                                 "--front-dir", front_dir, "--viewer-token-file", first["viewer_token_file"]]
            with gateway(dashboard_command, directory, service="Dashboard") as dashboard_port:
                report["stage"] = "chromium"
                env = {key: os.environ[key] for key in BROWSER_ENV if key in os.environ}
                env.update(PATH=os.defpath, LANG="C", HOME=str(directory),
                           AI_GUARD_TEST_DASHBOARD=f"http://127.0.0.1:{dashboard_port}",
                           AI_GUARD_TEST_CONFIG=str(directory / TENANT_A),
                           AI_GUARD_TEST_DECISION_PROOF=str(proof_path),
                           AI_GUARD_TEST_ARTIFACT_DIR=str(evidence))
                oracle = ROOT / "front/tests/browser_decision_live.mjs"
                browser = subprocess.run([node, oracle], env=env, cwd=directory, capture_output=True, timeout=90)
                verdict = json.loads(browser.stdout or browser.stderr)
                require(isinstance(verdict, dict), "Chromium did not produce a JSON verdict")
                report["browser_status"] = verdict.get("status") if verdict.get("status") in ("PASS", "FAIL", "BLOCKED") else "FAIL"
                write_private(evidence / "browser.json", json.dumps(verdict, indent=2).encode() + b"\n")
                if report["browser_status"] == "BLOCKED" and not local_failure:
                    report["status"] = "BLOCKED"
                require(browser.returncode == 0 and report["browser_status"] == "PASS"
                        and verdict.get("tenant_id") == TENANT_A
                        and all(verdict.get(flag) is True for flag in
                                ("audit_verified", "export_verified", "foreign_decision_absent")),
                        "browser decision journey failed")
                if head_bound:
                    modes = {"local-real": ("local_real_model_fail_closed_audit_web" if local_failure
                                            else "local_real_model_decision_audit_web"),
                             "typesafe-real": "typesafe_real_decision_audit_web",
                             "typesafe-replay-fixture": "typesafe_replay_fixture_decision_audit_web"}
                    require(verdict.get("mode") == modes[judge_mode]
                            and verdict.get("audit_head") == report["cli_audit"]["head_hash"]
                            and verdict.get("decisions") == [{key: item[key] for key in ("seq", "id", "hash", "outcome", "rule")}
                                                            for item in decisions],
                            "browser did not verify the same model receipt and CLI audit head")
                    if local_failure:
                        require(verdict.get("backend_status") == "unavailable_or_invalid"
                                and "provider_outcome" in verdict and verdict["provider_outcome"] is None,
                                "the browser did not verify the same absence of accepted Qwen advice")
                    if typesafe_replay:
                        require(verdict.get("provider_outcome") == advisory["provider_outcome"]
                                and verdict.get("returned_model") == advisory["returned_model"],
                                "browser proof differs from the retained TypeSafe response")
        if model_port is not None:
            report["model_endpoint_posts"] = len(local[1])
            require(local[1] == [True], "browser verification changed the number of validated Qwen POSTs")
        if local_failure:
            report.update(stage="local_real_model_verdict", fail_closed_proof_verified=True)
            raise ProbeError("Qwen returned no accepted advice; CLI and browser verified the durable refusal")
        if judge_mode == "typesafe-real" and advisory["backend_status"] != "evaluated":
            report.update(stage="typesafe_verdict", fail_closed_proof_verified=True)
            raise ProbeError("TypeSafe returned no accepted advice; CLI and browser verified the durable refusal")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True, type=Path, help="agentsandbox executable")
    parser.add_argument("--front-dir", required=True, type=Path, help="compiled frontend directory")
    parser.add_argument("--evidence-dir", required=True, type=Path, help="new private output directory")
    parser.add_argument("--judge-mode", choices=("rust", "local-fixture", "typesafe-fixture", "local-real", *TYPESAFE_REPLAY_MODES),
                        default="rust", help="Rust, synthetic provider, real loopback Qwen, or official TypeSafe")
    parser.add_argument("--model-port", type=int, help="existing real Qwen port on 127.0.0.1 (local-real only)")
    parser.add_argument("--model-timeout-seconds", type=model_timeout_seconds,
                        help="gateway model deadline from 1 to 12 seconds (local-real only, default: 5)")
    parser.add_argument("--typesafe-key-file", type=Path, help="private raw key file (typesafe-real only)")
    parser.add_argument("--fixture-typesafe-outcome", choices=("allow", "deny", "review", "unavailable"),
                        help="synthetic replay fixture outcome, default allow (typesafe-replay-fixture only)")
    args = parser.parse_args()
    if (args.judge_mode == "local-real") != (args.model_port is not None):
        parser.error("--model-port is required for local-real and only valid in that mode")
    if args.model_port is not None and not 1 <= args.model_port <= 65_535:
        parser.error("--model-port must be between 1 and 65535")
    if args.model_timeout_seconds is not None and args.judge_mode != "local-real":
        parser.error("--model-timeout-seconds is only valid in local-real mode")
    if (args.judge_mode == "typesafe-real") != (args.typesafe_key_file is not None):
        parser.error("--typesafe-key-file is required for typesafe-real and only valid in that mode")
    if args.fixture_typesafe_outcome is not None and args.judge_mode != "typesafe-replay-fixture":
        parser.error("--fixture-typesafe-outcome is only valid in typesafe-replay-fixture mode")
    report = {"status": "BLOCKED", "stage": "preflight", "judge_mode": args.judge_mode,
              "scope": "real Rust services, one bounded Qwen decision, no worker" if args.model_port is not None
              else "real Rust services, no model or worker"}
    if args.judge_mode == "typesafe-real":
        report["scope"] = "real Rust services, one official TypeSafe decision, no worker"
    evidence = None
    try:
        cli, front_dir, target = args.cli.resolve(), args.front_dir.resolve(), args.evidence_dir.absolute()
        node = shutil.which("node")
        require(cli.is_file() and os.access(cli, os.X_OK), "agentsandbox executable is unavailable")
        require((front_dir / "index.html").is_file(), "compiled frontend is unavailable")
        bundle = sorted((front_dir / "assets").glob("*.js"))
        require(bundle, "compiled frontend JavaScript is unavailable")
        oracle = ROOT / "front/tests/browser_decision_live.mjs"
        require(node and oracle.is_file(), "Node or the browser recipe is unavailable")
        key_file = private_credential_path(args.typesafe_key_file) if args.typesafe_key_file is not None else None
        target.mkdir(mode=0o700)
        evidence = target
        report.update(status="FAIL", cli_sha256=sha256(cli), html_sha256=sha256(front_dir / "index.html"),
                      bundle_js_sha256={asset.name: sha256(asset) for asset in bundle})
        run(cli, front_dir, node, evidence, report, args.judge_mode, args.model_port, args.model_timeout_seconds,
            typesafe_key_file=key_file,
            fixture_outcome=(args.fixture_typesafe_outcome or "allow") if args.judge_mode == "typesafe-replay-fixture" else None)
        report.update(status="PASS", stage="complete")
    except ProbeError as exc:
        report["diagnostic"] = str(exc)
    except (OSError, ValueError, HTTPException, subprocess.SubprocessError):
        report["diagnostic"] = "could not run the local decision and browser journey"
    serialized = json.dumps(report, indent=2) + "\n"
    if evidence is not None:
        write_private(evidence / "result.json", serialized.encode())
    print(serialized, end="")
    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2}[report["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
