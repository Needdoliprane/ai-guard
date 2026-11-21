#!/usr/bin/env python3
"""Run one managed provider case through the CLI and a live QA dashboard."""

import argparse
from contextlib import contextmanager
from dataclasses import asdict
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import sys
from urllib.parse import urlsplit
from uuid import UUID, uuid4


MODELS = {"codex": "gpt-6-luna", "claude-code": "claude-haiku-5-5"}
ROOT = Path(__file__).resolve().parents[2]
BROWSER = ROOT / "front/tests/browser_provider_live.mjs"
TTY_DRIVER = Path(__file__).with_name("interactive_driver.py").resolve()
RESULT_FILE = "result.txt"
ORIGINAL_FILE = "original.txt"
ORIGINAL_BYTES = b"ai-guard E2E original project\n"
PREFLIGHT_TIMEOUT = 90
DIAGNOSE_TIMEOUT = 10
EXPORT_TIMEOUT = 240
PTY_SELF_TEST_TIMEOUT = 90
PROVIDER_SELECTION_REFUSALS = {
    "provider.not_inspected": "not_checked",
    "provider.file_absent": "info",
    "provider.file_unavailable_or_not_private": "error",
    "provider.configuration_invalid": "error",
    "provider.configuration_ambiguous": "error",
    "provider.selection_ambiguous": "error",
    "provider.selection_no_profile": "error",
    "provider.selection_invalid": "error",
    "provider.selection_not_resolved": "not_checked",
}
PROVIDER_REFERENCE_REFUSALS = {
    "provider.login_required", "provider.credential_missing", "provider.credential_unsafe",
}
TERMINAL_STOP_REASONS = {
    "authorized worker duration elapsed",
    "runtime terminal transport ended",
    "physical terminal disconnected; worker stop requested",
    "operator requested worker stop with Ctrl-X Ctrl-X",
    "external operator signal requested worker stop; terminal suspension is not supported",
}
BROWSER_STAGES = {
    "configuration",
    "services HTTP",
    "Playwright et Chromium",
    "identité viewer",
    "journal de la session",
    "sélection de la sandbox",
    "application du filtre de la session",
    "détail de la décision",
    "ouverture du résultat terminal",
    "code de sortie dans le détail Commande",
    "données brutes du résultat terminal",
    "fichier et export dans la vue Fichiers",
    "export dans la vue Fichiers",
    "téléchargement JSONL de la session",
    "provenance du worker",
}


class CaseError(Exception):
    def __init__(self, message, *, status="FAIL"):
        super().__init__(message)
        self.status = status


def loopback_origin(value):
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError:
        raise argparse.ArgumentTypeError("expected an HTTP(S) loopback origin") from None
    if (
        parsed.scheme not in {"http", "https"}
        or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}
        or parsed.username is not None
        or parsed.path not in {"", "/"}
        or parsed.netloc.endswith(":")
        or port == 0
        or any(character.isspace() or ord(character) < 32 or character in "?#" for character in value)
    ):
        raise argparse.ArgumentTypeError("expected an HTTP(S) loopback origin without credentials or path")
    return f"{parsed.scheme}://{parsed.netloc}"


def immutable_image_id(value):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        raise argparse.ArgumentTypeError("expected an immutable worker image ID (sha256:…)")
    return value


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", required=True, type=Path, help="path to the agentsandbox executable")
    parser.add_argument("--diagnose-bin", type=Path,
                        help="offline diagnostic companion (default: ai-guard-diagnose next to --bin)")
    parser.add_argument("--config-dir", required=True, type=Path, help="prepared private QA configuration")
    parser.add_argument("--gateway", required=True, type=loopback_origin)
    parser.add_argument("--dashboard", required=True, type=loopback_origin)
    parser.add_argument("--runtime", required=True, choices=("docker", "podman"))
    parser.add_argument("--runtime-program", type=Path)
    parser.add_argument("--runtime-endpoint", help="explicit local Unix socket, or local for Podman")
    parser.add_argument("--allow-rootful-runtime", action="store_true")
    parser.add_argument("--image", required=True, type=immutable_image_id,
                        help="immutable local worker image ID built with the provider client")
    parser.add_argument("--client", required=True, choices=MODELS)
    parser.add_argument("--auth-mode", required=True, choices=("api", "subscription"))
    parser.add_argument(
        "--transport", default="batch", choices=("batch", "tty"),
        help="tty requires exclusive use of the prepared QA configuration",
    )
    parser.add_argument("--client-profile", required=True, help="prepared provider profile ID")
    parser.add_argument("--providers-file", type=Path,
                        help="private profile configuration used for diagnostics and, in legacy mode, run")
    parser.add_argument("--broker-root", type=Path, help="select an already running broker for a batch")
    parser.add_argument("--broker-profile", help="trusted broker profile for this batch")
    parser.add_argument("--broker-token-file", type=Path, help="private broker runner credential file")
    parser.add_argument("--broker-source-root", type=Path, help="existing allowed root for a fresh fixture project")
    parser.add_argument("--broker-export-root", type=Path, help="existing allowed root for a fresh export")
    parser.add_argument("--evidence-dir", required=True, type=Path, help="new directory under an existing parent")
    parser.add_argument("--timeout", type=int, default=120, help="worker deadline in seconds, defaults to 120")
    parser.add_argument("--node", default="node", help="Node executable for the live browser test")
    parser.add_argument("--worker-provenance", action="store_true",
                        help="verify runtime provenance in the dashboard worker detail")
    args = parser.parse_args()
    if not 1 <= args.timeout <= 300:
        parser.error("--timeout must be between 1 and 300 seconds")
    if args.runtime_program and not args.runtime_program.is_absolute():
        parser.error("--runtime-program must be absolute")
    if args.runtime_endpoint and not (
        args.runtime_endpoint.startswith("unix:///")
        or args.runtime == "podman" and args.runtime_endpoint == "local"
    ):
        parser.error("--runtime-endpoint must select a local Unix socket or Podman local")
    if args.providers_file is not None:
        args.providers_file = args.providers_file.absolute()
    broker_paths = (args.broker_root, args.broker_token_file, args.broker_source_root, args.broker_export_root)
    broker_selection = (*broker_paths, args.broker_profile)
    if any(value is not None for value in broker_selection):
        if args.transport != "batch":
            parser.error("the broker provider recipe supports batch only")
        if not all(broker_selection) or not all(path.is_absolute() for path in broker_paths):
            parser.error("broker batch requires the profile and all four absolute broker paths")
        if args.runtime_program or args.runtime_endpoint:
            parser.error("the broker registry selects the runtime program and endpoint")
    return args


def run_command(command, env, timeout, *, check=True):
    try:
        result = subprocess.run(
            command, cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        raise CaseError("command deadline exceeded") from None
    except OSError:
        raise CaseError("could not start the test executable", status="BLOCKED") from None
    if check and result.returncode != 0:
        raise CaseError(f"command returned exit status {result.returncode}")
    return result


def json_object(output):
    try:
        value = json.loads(output)
    except ValueError:
        raise CaseError("command did not return the expected JSON object") from None
    if not isinstance(value, dict):
        raise CaseError("command did not return the expected JSON object")
    return value


def provider_preflight(args, binary, config, providers, cli_env, report):
    report["stage"] = "provider_preflight"
    proof = report["provider_preflight"] = {}

    def block(code):
        proof["code"] = code
        raise CaseError(f"offline provider preflight blocked ({code})", status="BLOCKED") from None

    companion = args.diagnose_bin or binary.with_name("ai-guard-diagnose")
    if not companion.is_file() or not os.access(companion, os.X_OK):
        block("diagnostic_executable_unavailable")
    model = MODELS[args.client]
    try:
        result = run_command(
            [companion.absolute(), "--config-dir", config, *providers, "--client", args.client,
             "--auth-mode", args.auth_mode, "--client-profile", args.client_profile, "--model", model, "--json"],
            cli_env, DIAGNOSE_TIMEOUT, check=False,
        )
        diagnostic = json_object(result.stdout) if result.returncode in (0, 3, 4) else None
    except CaseError:
        block("diagnostic_report_unavailable")
    if diagnostic is None:
        block("diagnostic_report_unavailable")
    mode = diagnostic.get("mode")
    findings = diagnostic.get("findings")
    if (diagnostic.get("schema") != "ai-guard-diagnose-report"
            or type(diagnostic.get("version")) is not int or diagnostic["version"] != 1
            or not isinstance(mode, dict) or not isinstance(findings, list)
            or (mode.get("network"), mode.get("mutation"), mode.get("credential_contents_read"),
                mode.get("active_checks_executed")) != ("none", "none", "none", [])):
        block("diagnostic_report_invalid")

    provider = diagnostic.get("providers")
    selection = provider.get("selection") if isinstance(provider, dict) else None
    resolved = selection.get("resolved") if isinstance(selection, dict) else None
    provider_findings = [finding for finding in findings if isinstance(finding, dict)
                         and finding.get("axis") == "providers" and isinstance(finding.get("code"), str)]
    if resolved is None:
        for finding in provider_findings:
            code = finding["code"]
            if code in PROVIDER_SELECTION_REFUSALS and finding.get("status") == PROVIDER_SELECTION_REFUSALS[code]:
                block(code)

    profile = {"id": args.client_profile, "masked": False}
    expected = {"client": args.client.replace("-", "_"), "auth_mode": args.auth_mode,
                "profile": args.client_profile, "model": model}
    if (not isinstance(resolved, dict)
            or selection.get("requested") != dict.fromkeys(expected, "explicit")
            or resolved.get("chosen_by") != "profile" or resolved.get("profile") != profile
            or any(resolved.get(key) != expected[key] for key in ("client", "auth_mode", "model"))):
        block("diagnostic_selection_mismatch")
    proof["selection"] = expected

    for finding in provider_findings:
        subjects = finding.get("subjects")
        if (finding["code"] in PROVIDER_REFERENCE_REFUSALS and finding.get("status") == "error"
                and isinstance(subjects, dict) and subjects.get("kind") == "provider_profiles"
                and subjects.get("ids") == [profile]):
            block(finding["code"])

    references = diagnostic.get("credential_references")
    if not isinstance(references, list):
        block("diagnostic_report_invalid")
    owner = {"kind": "provider_profile", **profile}
    selected = [reference for reference in references if isinstance(reference, dict)
                and reference.get("role") == "provider" and reference.get("owner") == owner]
    if (len(selected) != 1
            or (selected[0].get("consumer"), selected[0].get("presence"), selected[0].get("entry_kind"),
                selected[0].get("private_rule"), selected[0].get("content"))
            != ("private_file_reader", "present", "regular_file", "satisfied", "not_read")):
        block("diagnostic_reference_not_verified")
    proof["code"] = "selected_reference_metadata_verified"


def audit(agentsandbox, control, cli_env):
    integrity = json_object(run_command([agentsandbox, "verify-audit", *control], cli_env, 20).stdout)
    count, digest = integrity.get("event_count"), integrity.get("head_hash")
    if (
        integrity.get("valid") is not True
        or not isinstance(count, int)
        or count < 0
        or not isinstance(digest, str)
        or not re.fullmatch(r"[0-9a-f]{64}", digest)
    ):
        raise CaseError("audit verification returned invalid integrity evidence")
    return {"event_count": count, "head_hash": digest, "valid": True}


def browser_command(node, env, report, *, preflight=False):
    command = [node, BROWSER]
    if preflight:
        command.append("--preflight")
    result = run_command(command, env, PREFLIGHT_TIMEOUT, check=False)
    if result.returncode != 0:
        try:
            failure = json_object(result.stdout)
        except CaseError:
            failure = {}
        stage = failure.get("stage")
        if isinstance(stage, str) and stage in BROWSER_STAGES:
            report["browser_stage"] = stage
        artifacts = failure.get("artifacts")
        if isinstance(artifacts, dict):
            folder, files = artifacts.get("directory"), artifacts.get("files")
            if (isinstance(folder, str) and re.fullmatch(r"browser-failure-[a-zA-Z0-9]+", folder)
                    and isinstance(files, list) and files
                    and all(name in ("page.html", "page.png") for name in files)):
                report["browser_artifacts"] = [f"{folder}/{name}" for name in files]
        status = "BLOCKED" if result.returncode == 2 else "FAIL"
        raise CaseError(f"live browser check returned exit status {result.returncode}", status=status)
    proof = json_object(result.stdout)
    if proof.get("status") != "PASS" or preflight and proof.get("mode") != "preflight":
        raise CaseError("live browser check did not confirm its result")
    return proof


def browser_evidence(proof, session, worker_id, file_digest, args):
    expected = {
        "mode": "session_trace",
        "session": session,
        "fileSha256": file_digest,
        "transport": args.transport,
        "client": args.client.replace("-", "_"),
        "authMode": args.auth_mode,
        "requestedModel": MODELS[args.client],
        "runtime": args.runtime,
        "workerId": worker_id,
    }
    if any(proof.get(field) != value for field, value in expected.items()):
        raise CaseError("live browser evidence does not match the CLI session, provider, runtime and workspace")
    result = {"session_id": session, "worker_id": worker_id}
    fields = ["decisionEventId", "terminalEventId", "exportEventId"]
    if file_digest is not None:
        result["file_sha256"] = file_digest
        fields.append("fileEventId")
    for field in fields:
        try:
            result[field] = str(UUID(proof[field]))
        except (KeyError, TypeError, ValueError, AttributeError):
            raise CaseError("live browser did not identify the correlated events") from None
    ids = {result[field] for field in fields}
    if len(ids) != len(fields) or str(UUID(int=0)) in ids:
        raise CaseError("live browser returned ambiguous audit event identities")
    audit_digest = proof.get("auditExportSha256")
    if not isinstance(audit_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", audit_digest):
        raise CaseError("live browser did not verify the JSONL export")
    result["audit_export_sha256"] = audit_digest
    manifest = proof.get("exportManifestSha256")
    if not isinstance(manifest, str) or not re.fullmatch(r"[0-9a-f]{64}", manifest):
        raise CaseError("live browser did not identify the workspace export manifest")
    result["export_manifest_sha256"] = manifest
    count = proof.get("count")
    if type(count) is not int or count < len(fields):
        raise CaseError("live browser returned an invalid count of audited events")
    result["audit_export_event_count"] = count
    browser_version = proof.get("browser")
    if isinstance(browser_version, str) and re.fullmatch(r"[0-9]{1,6}(?:\.[0-9]{1,6}){1,3}", browser_version):
        result["chromium_version"] = browser_version
    if args.worker_provenance:
        if proof.get("workerProvenance") != "displayed_fields_match_registry":
            raise CaseError("live browser did not confirm the requested Workers detail")
        result["worker_provenance"] = "displayed_fields_match_registry"
    return result


def check_files(directory, expected):
    if not stat.S_ISDIR(directory.lstat().st_mode):
        raise CaseError("workspace is not a directory")
    if {path.name for path in directory.iterdir()} != expected.keys():
        raise CaseError("workspace entries differ from the expected files")
    for name, content in expected.items():
        path = directory / name
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size != len(content):
            raise CaseError(f"{name} is not the expected independent regular file")
        with path.open("rb") as file:
            if file.read(len(content) + 1) != content:
                raise CaseError(f"{name} content differs from the expected bytes")


@contextmanager
def case_config_lock(config, *, exclusive):
    # One caller cannot source a TTY session from a config actively used by
    # another case. Direct operator commands also must leave the config alone.
    try:
        directory = config.stat()
        if not (
            stat.S_ISDIR(directory.st_mode) and stat.S_IMODE(directory.st_mode) == 0o700
            and directory.st_uid == os.geteuid()
        ):
            raise CaseError("prepared private QA configuration is required", status="BLOCKED")
        fd = os.open(
            config / ".e2e-provider-case.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600,
        )
    except OSError:
        raise CaseError("prepared private QA configuration is required", status="BLOCKED") from None
    with os.fdopen(fd, "rb") as lock:
        metadata = os.fstat(lock.fileno())
        if not (
            stat.S_ISREG(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o600
            and metadata.st_nlink == 1 and metadata.st_uid == os.geteuid()
        ):
            raise CaseError("QA configuration lock is not a private regular file", status="BLOCKED")
        try:
            mode = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
            fcntl.flock(lock.fileno(), mode | fcntl.LOCK_NB)
        except BlockingIOError:
            raise CaseError("QA configuration is in use by another provider case", status="BLOCKED") from None
        yield


def session_checkpoints(config):
    try:
        entries = list((config / "sessions").iterdir())
    except OSError:
        raise CaseError("prepared QA configuration lacks its sessions directory", status="BLOCKED") from None
    return {
        path.stem for path in entries
        if path.suffix == ".json" and stat.S_ISREG(path.lstat().st_mode)
        and re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", path.stem)
    }


def require_run_summary(summary, *, changes):
    cleanup = summary.get("cleanup") if isinstance(summary, dict) else None
    if not (
        isinstance(cleanup, dict) and cleanup.get("state") == "removed"
        and summary.get("exit_code") == 0
        and summary.get("timed_out") is False
        and summary.get("output_captured") is False
        and summary.get("changed_files") == changes
        and summary.get("postprocessing_error") is None
        and summary.get("transport_stop_reason") is None
        and summary.get("control_deadline") is None
    ):
        raise CaseError(f"run did not prove a clean result with {changes} changed files")


def observed_worker_image(status, worker, args, session, model):
    """Require the chosen session, launch and terminal inspection to agree."""
    if status.get("session_id") != session or not isinstance(worker, dict):
        return None
    launch = worker.get("launch")
    attempt = worker.get("attempt")
    observation = worker.get("last_observation")
    if not isinstance(launch, dict) or not isinstance(attempt, dict) or not isinstance(observation, dict):
        return None
    resolution = launch.get("provider_resolution")
    inspection = observation.get("inspection")
    client = args.client.replace("-", "_")
    if (
        not isinstance(resolution, dict)
        or not isinstance(inspection, dict)
        or launch.get("session_id") != session
        or launch.get("runtime_kind") != args.runtime
        or inspection.get("runtime_kind") != args.runtime
        or launch.get("image_id") != args.image
        or inspection.get("image_id") != args.image
        or not attempt.get("container_id")
        or inspection.get("container_id") != attempt["container_id"]
        or not isinstance(launch.get("authority_id"), str)
        or inspection.get("authority_id") != launch["authority_id"]
        or not isinstance(worker.get("spec_hash"), str)
        or inspection.get("spec_hash") != worker["spec_hash"]
        or observation.get("running") is not False
        or observation.get("status") not in ("exited", "dead")
        or (resolution.get("client"), resolution.get("auth_mode"), resolution.get("model"))
        != (client, args.auth_mode, model)
        or resolution.get("profile_id") != args.client_profile
        or resolution.get("transport") != ("interactive_tty" if args.transport == "tty" else "batch")
    ):
        return None
    return inspection["image_id"]


def matched_tty_summary(status, worker, session):
    """Match the durable CLI result to the inspected worker before joining facts."""
    summary = status.get("result")
    volume = worker["attempt"].get("volume_name")
    exit_code = worker["last_observation"].get("exit_code")
    if (
        isinstance(summary, dict) and summary.get("session_id") == session
        and isinstance(volume, str) and volume and summary.get("volume") == volume
        and type(exit_code) is int and -(1 << 63) <= exit_code < (1 << 63)
        and type(summary.get("exit_code")) is int and summary["exit_code"] == exit_code
    ):
        return summary
    return None


def tty_terminal_result(worker, summary):
    observation = worker["last_observation"]
    exit_code = observation.get("exit_code")
    oom_killed = observation.get("oom_killed")
    cleanup_removed = worker.get("cleanup_removed")
    timed_out = summary.get("timed_out")
    reason = summary.get("transport_stop_reason")
    if not isinstance(reason, str) or not reason:
        reason = None
    elif reason not in TERMINAL_STOP_REASONS:
        reason = "unclassified"
    control_deadline = summary.get("control_deadline")
    if not (isinstance(control_deadline, dict) and set(control_deadline) == {"deadline", "stop"}
            and control_deadline["deadline"] in ("original_execution_deadline", "observation_window")
            and control_deadline["stop"] in ("no_owned_worker", "already_stopped", "engine_acknowledged", "unconfirmed")):
        control_deadline = None
    return {
        "exit_code": exit_code if type(exit_code) is int and -(1 << 63) <= exit_code < (1 << 63) else None,
        "oom_killed": oom_killed if type(oom_killed) is bool else None,
        "cleanup_removed": cleanup_removed if type(cleanup_removed) is bool else None,
        "timed_out": timed_out if type(timed_out) is bool else None,
        "transport_stop_reason": reason,
        "control_deadline": control_deadline,
    }


def batch_prompt(args):
    marker = f"ai-guard e2e {args.runtime}/{args.client}/{args.auth_mode} {uuid4().hex[:12]}"
    prompt = (
        f"Create only a new file named {RESULT_FILE} in the workspace root. "
        f"Its content must be exactly the following ASCII text with no trailing newline: {marker}\n"
        "Leave all other files unchanged. Do not start subagents or run tests."
    )
    return marker, prompt


def broker_reply(result, kind):
    if result.returncode != 0:
        raise CaseError(f"broker {kind} request failed")
    value = json_object(result.stdout).get(kind)
    if not isinstance(value, dict):
        raise CaseError(f"broker {kind} reply is missing")
    return value


def broker_job_result(value, operation):
    outcome = value.get("outcome")
    if (type(value.get("version")) is not int or value["version"] != 1
            or value.get("operation_id") != operation or type(value.get("cancel_requested")) is not bool
            or not isinstance(outcome, dict) or outcome.get("state") not in (
                "pending", "review", "refused", "cancelled", "terminal", "ambiguous",
            )):
        raise CaseError("broker result does not identify the original operation and expected protocol")
    return outcome


def broker_uuid(value):
    try:
        parsed = UUID(value)
    except (ValueError, TypeError, AttributeError):
        return None
    return value if parsed.int and str(parsed) == value else None


def broker_terminal_facts(terminal):
    if not isinstance(terminal, dict):
        raise CaseError("broker result has no terminal evidence")
    session = broker_uuid(terminal.get("session_id"))
    cid, digest = terminal.get("container_id"), terminal.get("manifest_digest")
    seq_names = ("result_audit_seq", "scan_audit_seq", "cleanup_audit_seq")
    sequences = tuple(terminal.get(name) for name in seq_names)
    if not (
        session and isinstance(cid, str) and re.fullmatch(r"[0-9a-f]{64}", cid)
        and isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest)
        and type(terminal.get("exit_code")) is int and terminal["exit_code"] == 0
        and terminal.get("oom_killed") is False and terminal.get("output_captured") is False
        and type(terminal.get("changed_files")) is int and terminal["changed_files"] == 1
        and all(type(seq) is int and seq > 0 for seq in sequences) and len(set(sequences)) == 3
    ):
        raise CaseError("broker terminal result does not prove one change, clean exit and three durable audit receipts")
    return {"session_id": session, "container_id": cid, "manifest_digest": digest,
            **dict(zip(seq_names, sequences))}


def write_broker_json(evidence, name, value):
    path = evidence / name
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf8") as file:
        json.dump(value, file, separators=(",", ":"))
        file.write("\n")
        file.flush()
        os.fsync(file.fileno())
    directory = os.open(evidence, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return path


def broker_export_digest(output, expected):
    check_files(output, expected)
    manifest = {}
    for name, content in sorted(expected.items()):
        mode = stat.S_IMODE((output / name).lstat().st_mode)
        if mode != 0o644:
            raise CaseError("broker export did not publish the expected regular file mode")
        manifest[name] = {"sha256": hashlib.sha256(content).hexdigest(), "bytes": len(content), "mode": mode}
    # The publication manifest records mode 0644, even when the worker's scan recorded other modes.
    encoded = json.dumps(manifest, separators=(",", ":")).encode("utf8")
    return hashlib.sha256(b"ai-guard:workspace-manifest:v1\0" + encoded).hexdigest()


def broker_browser_input(args, report):
    terminal = report["broker_terminal"]
    return {
        "version": 1,
        "operation_id": report["broker_operation_id"],
        "session_id": terminal["session_id"],
        "container_id": terminal["container_id"],
        "result_audit_seq": terminal["result_audit_seq"],
        "scan_audit_seq": terminal["scan_audit_seq"],
        "cleanup_audit_seq": terminal["cleanup_audit_seq"],
        "scan_manifest_sha256": terminal["manifest_digest"],
        "export_command_id": report["broker_export_command_id"],
        "export_request_id": report["broker_export_request_id"],
        "export_manifest_sha256": report["broker_export_manifest_sha256"],
        "export_file_count": 2,
        "file_name": RESULT_FILE,
        "file_sha256": report["result_sha256"],
        "client": args.client.replace("-", "_"),
        "auth_mode": args.auth_mode,
        "client_profile": args.client_profile,
        "model_requested": MODELS[args.client],
        "runtime": args.runtime,
        "worker_image_id": args.image,
    }


def broker_browser_evidence(proof, expected, *, worker_provenance):
    matched = {
        "status": "PASS", "mode": "broker_session_trace", "executionPath": "broker_batch", "transport": "batch",
        "operationId": expected["operation_id"], "session": expected["session_id"],
        "containerId": expected["container_id"], "workerImageId": expected["worker_image_id"],
        "client": expected["client"], "authMode": expected["auth_mode"],
        "clientProfile": expected["client_profile"], "requestedModel": expected["model_requested"],
        "runtime": expected["runtime"], "exportRequestId": expected["export_request_id"],
        "fileSha256": expected["file_sha256"], "scanManifestSha256": expected["scan_manifest_sha256"],
        "exportManifestSha256": expected["export_manifest_sha256"],
    }
    if any(proof.get(field) != value for field, value in matched.items()):
        raise CaseError("browser proof does not match the broker operation, worker or export")
    ids = ["decisionEventId", "terminalEventId", "cleanupEventId", "scanEventId", "exportEventId"]
    event_ids = [broker_uuid(proof.get(field)) for field in ids]
    if not all(event_ids) or len(set(event_ids)) != len(ids):
        raise CaseError("browser did not identify all five broker audit events")
    sequence_names = ["decisionAuditSeq", "resultAuditSeq", "cleanupAuditSeq", "scanAuditSeq", "exportAuditSeq"]
    sequences = [proof.get(field) for field in sequence_names]
    if (not all(type(seq) is int and 0 < seq <= 2**53 - 1 for seq in sequences)
            or sequences != sorted(set(sequences))
            or sequences[1:4] != [expected["result_audit_seq"], expected["cleanup_audit_seq"], expected["scan_audit_seq"]]):
        raise CaseError("browser audit sequences differ from the durable broker receipts")
    worker_id, authority_id = broker_uuid(proof.get("workerId")), broker_uuid(proof.get("authorityId"))
    audit_digest, count = proof.get("auditExportSha256"), proof.get("count")
    if (not worker_id or not authority_id or not isinstance(audit_digest, str)
            or not re.fullmatch(r"[0-9a-f]{64}", audit_digest) or type(count) is not int or count < 5):
        raise CaseError("browser did not prove the worker authority and filtered audit export")
    recorded = {**matched, **dict(zip(ids, event_ids)), **dict(zip(sequence_names, sequences)),
                "workerId": worker_id, "authorityId": authority_id, "auditExportSha256": audit_digest, "count": count}
    browser_version = proof.get("browser")
    if isinstance(browser_version, str) and re.fullmatch(r"[0-9]{1,6}(?:\.[0-9]{1,6}){1,3}", browser_version):
        recorded["browser"] = browser_version
    if worker_provenance:
        if proof.get("workerProvenance") != "displayed_fields_match_registry":
            raise CaseError("browser did not confirm the requested Workers detail")
        recorded["workerProvenance"] = "displayed_fields_match_registry"
    return recorded


def run_broker_case(args, evidence, report, binary, control, cli_env, *, node, browser_env):
    report.update(stage="broker_preflight", execution_path="broker_batch")
    source = args.broker_source_root.resolve()
    exports = args.broker_export_root.resolve()
    if (not source.is_dir() or not exports.is_dir()
            or source.is_relative_to(exports) or exports.is_relative_to(source)):
        raise CaseError("existing disjoint broker source and export roots are required", status="BLOCKED")
    for reserved in (args.broker_root.resolve(), args.config_dir.resolve()):
        if any(root.is_relative_to(reserved) or reserved.is_relative_to(root) for root in (source, exports)):
            raise CaseError("broker source and export roots must be separate from control data", status="BLOCKED")
    operation, export_command = str(uuid4()), str(uuid4())
    project = source / f"provider-case-{operation}"
    output = exports / f"provider-case-{operation}"
    if os.path.lexists(project) or os.path.lexists(output):
        raise CaseError("broker fixture project and export destination must be new", status="BLOCKED")
    write_broker_json(evidence, "broker-operation.json", {
        "operation_id": operation, "export_command_id": export_command,
        "project": str(project), "destination": str(output),
    })
    report.update(broker_operation_id=operation, broker_export_command_id=export_command)
    connection = [
        "--root", args.broker_root, "--profile", args.broker_profile,
        "--token-file", args.broker_token_file, "--operation-id", operation,
    ]
    try:
        before = broker_reply(run_command([binary, "broker", "status", *connection], cli_env, 20, check=False),
                              "status").get("status")
        if not isinstance(before, dict) or before.get("operation_id") != operation or before.get("state") != "absent":
            raise CaseError("new operation identity was not confirmed absent")
    except CaseError:
        raise CaseError("broker could not confirm a fresh operation for the selected profile", status="BLOCKED") from None

    project.mkdir(mode=0o700)
    (project / ORIGINAL_FILE).write_bytes(ORIGINAL_BYTES)
    marker, prompt = batch_prompt(args)
    wait_seconds = args.timeout + 180
    run = [
        binary, "run", "--broker-root", args.broker_root, "--broker-profile", args.broker_profile,
        "--broker-token-file", args.broker_token_file, "--operation-id", operation,
        "--project", project, "--timeout", str(args.timeout), "--broker-wait-seconds", str(wait_seconds),
        "--client", args.client, "--auth-mode", args.auth_mode, "--client-profile", args.client_profile,
        "--model", MODELS[args.client], "--prompt", prompt,
    ]
    report.update(stage="broker_run", runtime_result="unknown", cli_exit_status=None, broker_run_reply="unavailable")
    initial = None
    try:
        launched = run_command(run, cli_env, wait_seconds + 60, check=False)
        report["cli_exit_status"] = launched.returncode
    except CaseError:
        # The broker may have committed the operation even though this wait failed.
        launched = None
    if launched is not None and launched.stdout:
        try:
            initial = json_object(launched.stdout)
            broker_job_result(initial, operation)
        except CaseError:
            initial = None
            report["broker_run_reply"] = "invalid"

    report["stage"] = "broker_result"
    replay = run_command([binary, "broker", "result", *connection], cli_env, 30, check=False)
    report["broker_result_exit_status"] = replay.returncode
    durable = broker_reply(replay, "result").get("result")
    if not isinstance(durable, dict):
        raise CaseError("durable result reread did not return the original operation")
    outcome = broker_job_result(durable, operation)
    report.update(broker_result_source="broker_result", broker_outcome=outcome["state"])
    candidate_terminal = outcome.get("result")
    if outcome["state"] == "terminal" and isinstance(candidate_terminal, dict):
        session = broker_uuid(candidate_terminal.get("session_id"))
        if session:
            report["session_id"] = session
    if report["broker_run_reply"] == "invalid":
        raise CaseError("run returned an invalid result; inspect the saved operation and reread")
    if initial is not None:
        if initial != durable:
            report["broker_run_reply"] = "diverged"
            raise CaseError("run result differs from the durable result for the same operation")
        report["broker_run_reply"] = "matched"
    if outcome["state"] != "terminal" or durable["cancel_requested"]:
        raise CaseError("broker did not confirm a non-cancelled terminal job; inspect the saved operation")
    terminal = broker_terminal_facts(candidate_terminal)
    report["broker_terminal"] = terminal

    observed = broker_reply(run_command([binary, "broker", "status", *connection], cli_env, 20, check=False), "status").get("status")
    if not isinstance(observed, dict) or (
        observed.get("operation_id") != operation or observed.get("state") != "accepted"
        or observed.get("worker_state") != "terminal" or observed.get("quota_held") is not False
        or observed.get("runtime_status") not in ("exited", "dead")
    ):
        raise CaseError("broker status did not confirm the same terminal job and released quota")
    report.update(runtime_result="terminal", quota_released=True, cleanup="removed")

    report["stage"] = "broker_export"
    if os.path.lexists(output):
        raise CaseError("broker export destination already exists")
    exported = run_command([binary, "broker", "export", *connection, "--command-id", export_command,
                            "--destination", output], cli_env, EXPORT_TIMEOUT, check=False)
    report["broker_export_exit_status"] = exported.returncode
    receipt = broker_reply(exported, "export_acknowledged")
    if (receipt.get("operation_id") != operation or receipt.get("request_id") != export_command
            or type(receipt.get("files")) is not int or receipt["files"] != 2):
        raise CaseError("broker export acknowledgement did not match the original operation and export command")
    original = {ORIGINAL_FILE: ORIGINAL_BYTES}
    expected = marker.encode("ascii")
    check_files(project, original)
    export_digest = broker_export_digest(output, {**original, RESULT_FILE: expected})
    if receipt.get("manifest_digest") != export_digest:
        raise CaseError("broker export acknowledgement did not match the published manifest")
    report.update(original_unchanged=True, result_sha256=hashlib.sha256(expected).hexdigest(),
                  broker_export_request_id=receipt["request_id"], broker_export_manifest_sha256=export_digest)

    report["stage"] = "audit"
    report["audit_after"] = audit(binary, control, cli_env)
    if report["audit_after"]["event_count"] <= report["audit_before"]["event_count"]:
        raise CaseError("no newly audited events were found")
    report["broker_cli_proof"] = "terminal_and_export_verified"
    report["stage"] = "broker_browser"
    if report["cli_exit_status"] != 0 or report["broker_run_reply"] != "matched":
        raise CaseError("original run did not confirm success; the same durable job and export were verified")
    browser_input = broker_browser_input(args, report)
    proof_path = write_broker_json(evidence, "broker-browser-proof.json", browser_input)
    proof = browser_command(node, {
        **browser_env,
        "AI_GUARD_TEST_BROKER_PROOF": str(proof_path.resolve(strict=True)),
    }, report)
    verified = broker_browser_evidence(proof, browser_input, worker_provenance=args.worker_provenance)
    report.update(broker_browser=verified, worker_id=verified["workerId"],
                  authority_id=verified["authorityId"], worker_image_id=verified["workerImageId"])


def run_case(args, evidence, report, config):
    binary = args.bin.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK) or not config.is_dir():
        raise CaseError("the CLI and prepared private QA configuration are required", status="BLOCKED")

    # Use only the selected private profile for credentials. Do not forward
    # ambient API keys, proxies, or Node preload options to either child.
    cli_env = {name: os.environ[name] for name in ("PATH", "HOME", "TMPDIR", "LANG") if name in os.environ}
    binary_hash = hashlib.sha256()
    with binary.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            binary_hash.update(chunk)
    report["cli_sha256"] = binary_hash.hexdigest()
    control = ["--config-dir", str(config), "--gateway", args.gateway]
    providers = ["--providers-file", args.providers_file] if args.providers_file is not None else []
    provider_preflight(args, binary, config, providers, cli_env, report)

    report["stage"] = "browser_preflight"
    node = shutil.which(args.node)
    if not node or not BROWSER.is_file():
        raise CaseError("Node and the live browser checks are required", status="BLOCKED")
    browser_env = dict(cli_env)
    for name in ("CODEX_PRIMARY_RUNTIME_NODE_MODULES", "PLAYWRIGHT_CHROMIUM_EXECUTABLE", "PLAYWRIGHT_BROWSERS_PATH"):
        if name in os.environ:
            browser_env[name] = os.environ[name]
    browser_env.update(
        AI_GUARD_TEST_GATEWAY=args.gateway,
        AI_GUARD_TEST_DASHBOARD=args.dashboard,
        AI_GUARD_TEST_CONFIG=str(config),
        AI_GUARD_TEST_TRANSPORT=args.transport,
        AI_GUARD_TEST_ARTIFACT_DIR=str(evidence),
        AI_GUARD_TEST_WORKER_PROVENANCE="1" if args.worker_provenance else "0",
    )
    browser_command(node, browser_env, report, preflight=True)
    report["stage"] = "audit_preflight"
    report["audit_before"] = audit(binary, control, cli_env)
    if args.broker_root is not None:
        return run_broker_case(args, evidence, report, binary, control, cli_env, node=node, browser_env=browser_env)

    interactive = args.transport == "tty"
    if interactive:
        report["stage"] = "pty_preflight"
        run_command([sys.executable, "-B", TTY_DRIVER, "--self-test"], cli_env, PTY_SELF_TEST_TIMEOUT)

    project = evidence / "project"
    project.mkdir(mode=0o700)
    (project / ORIGINAL_FILE).write_bytes(ORIGINAL_BYTES)
    runtime_program = ["--runtime-program", str(args.runtime_program)] if args.runtime_program else []
    runtime_endpoint = ["--runtime-endpoint", args.runtime_endpoint] if args.runtime_endpoint else []
    rootful = ["--allow-rootful-runtime"] if args.allow_rootful_runtime else []
    model = MODELS[args.client]

    report["stage"] = "run"
    report["runtime_result"] = "unknown"
    command = [
        binary, "run", *control, "--project", project, "--runtime", args.runtime,
        *runtime_program, *runtime_endpoint, *rootful, "--image", args.image,
        "--profile", "observe", "--network", "bridge", "--timeout", str(args.timeout),
        "--client", args.client, "--auth-mode", args.auth_mode, "--client-profile", args.client_profile,
        *providers, "--model", model,
    ]
    if interactive:
        from interactive_driver import run_provider

        checkpoints = session_checkpoints(config)
        tty = run_provider(command, timeout=args.timeout + 240, env=cli_env, cwd=ROOT, client=args.client)
        report["tty"] = asdict(tty)
        report["cli_exit_status"] = tty.cli_exit_status
        candidates = session_checkpoints(config) - checkpoints
        report["new_checkpoint_count"] = len(candidates)
        if len(candidates) != 1:
            raise CaseError("no unique new durable session; consult the QA configuration before any retry")
        session = candidates.pop()
    else:
        marker, prompt = batch_prompt(args)
        result = run_command([*command, "--prompt", prompt], cli_env, args.timeout + 240, check=False)
        report["cli_exit_status"] = result.returncode
        summary = None
        if result.stdout:
            summary = json_object(result.stdout)
            try:
                session = str(UUID(summary["session_id"]))
            except (KeyError, TypeError, ValueError, AttributeError):
                raise CaseError("CLI did not identify the resulting session") from None
            report["session_id"] = session
            cleanup = summary.get("cleanup")
            report["cleanup"] = (
                "removed" if isinstance(cleanup, dict) and cleanup.get("state") == "removed" else "unconfirmed"
            )
        if result.returncode != 0:
            raise CaseError(f"run returned exit status {result.returncode}; consult the durable session state")
        require_run_summary(summary, changes=1)
    report["session_id"] = session

    report["stage"] = "worker_status"
    status = json_object(
        run_command([binary, "worker-status", *control, "--session", session], cli_env, 30).stdout,
    )
    worker = status.get("worker")
    terminal_worker = (isinstance(worker, dict) and worker.get("state") == "terminal"
                       and worker.get("quota_held") is False)
    worker_image_id = observed_worker_image(status, worker, args, session, model) if terminal_worker else None
    worker_id = broker_uuid(worker.get("worker_id")) if worker_image_id else None
    tty_summary = None
    if worker_id:
        report["runtime_result"] = "terminal"
        report["quota_released"] = True
        report["worker_image_id"] = worker_image_id
        report["worker_id"] = worker_id
        if interactive:
            tty_summary = matched_tty_summary(status, worker, session)
            report["tty_terminal_result"] = tty_terminal_result(worker, tty_summary or {})

    if interactive and not tty.passed:
        report["stage"] = "interactive_conversation"
        raise CaseError(
            f"TTY did not prove two replies, a native exit and restored modes at stage {tty.stage}; "
            "consult the durable session state before any retry"
        )
    if not terminal_worker:
        raise CaseError("durable worker state does not prove terminal completion and quota release")
    if not worker_image_id:
        raise CaseError("terminal inspection does not match the selected session, engine, image and profile")
    if worker_id is None:
        raise CaseError("the terminal worker has no valid identity")
    if interactive:
        if tty_summary is None:
            raise CaseError("terminal CLI result does not match the session, worker volume and exit code")
        require_run_summary(tty_summary, changes=0)
        report["cleanup"] = "removed"

    report["stage"] = "workspace_export"
    output = evidence / "export"
    run_command(
        [binary, "export-workspace", *control, *runtime_program, "--session", session, "--output", output],
        cli_env, EXPORT_TIMEOUT,
    )
    original = {ORIGINAL_FILE: ORIGINAL_BYTES}
    check_files(project, original)
    if interactive:
        check_files(output, original)
    else:
        expected = marker.encode("ascii")
        check_files(output, {**original, RESULT_FILE: expected})
        report["result_sha256"] = hashlib.sha256(expected).hexdigest()
    report["original_unchanged"] = True

    report["stage"] = "audit"
    report["audit_after"] = audit(binary, control, cli_env)
    if report["audit_after"]["event_count"] <= report["audit_before"]["event_count"]:
        raise CaseError("no newly audited events were found")

    report["stage"] = "browser"
    browser_env.update(
        AI_GUARD_TEST_SESSION_ID=session,
        AI_GUARD_TEST_CLIENT=args.client.replace("-", "_"),
        AI_GUARD_TEST_MODEL=model,
        AI_GUARD_TEST_AUTH_MODE=args.auth_mode,
        AI_GUARD_TEST_RUNTIME=args.runtime,
        AI_GUARD_TEST_EXPECT_WORKSPACE_EXPORT="1",
    )
    if not interactive:
        browser_env.update(
            AI_GUARD_TEST_EXPECTED_FILE=RESULT_FILE,
            AI_GUARD_TEST_EXPECTED_FILE_SHA256=report["result_sha256"],
        )
    proof = browser_command(node, browser_env, report)
    report["browser"] = browser_evidence(proof, session, worker_id, report.get("result_sha256"), args)


def main():
    args = arguments()
    try:
        evidence = args.evidence_dir.absolute()
        evidence.mkdir(mode=0o700)
    except OSError:
        print("Evidence directory must be new with an existing writable parent.", file=sys.stderr)
        return 2

    report = {
        "status": "FAIL",
        "stage": "setup",
        "started_at": datetime.now(timezone.utc).isoformat(),
        "host": f"{platform.system()}/{platform.machine()}",
        "client": args.client,
        "auth_mode": args.auth_mode,
        "transport": args.transport,
        "model_requested": MODELS[args.client],
        "profile": args.client_profile,
        "runtime": args.runtime,
        "image_requested": args.image,
        "rootful_exception_requested": args.allow_rootful_runtime,
    }
    if args.broker_root is not None:
        report.update(execution_path="broker_batch", worker_id=None, worker_image_id=None, authority_id=None)
    try:
        config = args.config_dir.resolve()
        with case_config_lock(config, exclusive=args.transport == "tty"):
            run_case(args, evidence, report, config)
    except CaseError as error:
        report.update(status=error.status, diagnostic=str(error))
    except OSError:
        report["diagnostic"] = "an expected local file could not be inspected or written"
    except KeyboardInterrupt:
        report["diagnostic"] = "interrupted; consult the durable session state before any retry"
    else:
        report.update(status="PASS", stage="complete")
    report["ended_at"] = datetime.now(timezone.utc).isoformat()
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    with (evidence / "result.json").open("x", encoding="utf8") as file:
        file.write(encoded)
    print(encoded, end="")
    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2}[report["status"]]


if __name__ == "__main__":
    sys.exit(main())
