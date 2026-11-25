#!/usr/bin/env python3
"""Check Docker recovery after a CLI crash, with an optional autonomous deadline probe."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time
from uuid import UUID, uuid4

from run_provider_case import (
    ROOT,
    CaseError,
    audit,
    case_config_lock,
    json_object,
    loopback_origin,
    run_command,
    session_checkpoints,
)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", required=True, type=Path)
    parser.add_argument("--config-dir", required=True, type=Path, help="dedicated private QA configuration")
    parser.add_argument("--gateway", required=True, type=loopback_origin)
    parser.add_argument("--image", required=True, help="local Docker worker image with /usr/bin/sleep")
    parser.add_argument("--evidence-dir", required=True, type=Path, help="new directory under an existing parent")
    parser.add_argument("--runtime-program", type=Path, help="absolute approved local Docker executable")
    parser.add_argument("--runtime-endpoint", help="explicit local Docker Unix socket")
    parser.add_argument("--allow-rootful-runtime", action="store_true")
    parser.add_argument("--start-timeout", default=240, type=int, help="seconds to await the started worker")
    parser.add_argument(
        "--autonomous-deadline", type=int, metavar="SECONDS",
        help="set a worker deadline and observe Docker after SIGKILL before any recovery",
    )
    parser.add_argument("--deadline-grace", type=int, default=5, help="seconds allowed after the worker deadline")
    args = parser.parse_args()
    if args.runtime_program is not None and not args.runtime_program.is_absolute():
        parser.error("--runtime-program must be absolute")
    if args.runtime_endpoint is not None and not args.runtime_endpoint.startswith("unix:///"):
        parser.error("--runtime-endpoint must select a local Unix socket")
    if not 10 <= args.start_timeout <= 600:
        parser.error("--start-timeout must be between 10 and 600 seconds")
    if args.autonomous_deadline is not None:
        if not 5 <= args.autonomous_deadline <= 60 or not 0 <= args.deadline_grace <= 30:
            parser.error("autonomous deadline must be 5–60 seconds and grace 0–30 seconds")
        if args.runtime_program is None or args.runtime_endpoint is None:
            parser.error("autonomous deadline requires --runtime-program and --runtime-endpoint")
    return args


def object_field(value, name):
    entry = value.get(name)
    return entry if isinstance(entry, dict) else {}


def worker_for_session(payload, session, command):
    worker = object_field(payload, "worker")
    launch = object_field(worker, "launch")
    if (
        payload.get("session_id") == session
        and launch.get("session_id") == session
        and launch.get("runtime_kind") == "docker"
        and launch.get("network") == "none"
        and object_field(launch, "command").get("argv") == command
    ):
        return worker
    return None


def instance(worker):
    attempt = object_field(worker, "attempt")
    container = attempt.get("container_id")
    if not isinstance(container, str) or not re.fullmatch(r"[a-f0-9]{64}", container):
        raise CaseError("running worker has no complete Docker container identity")
    try:
        return {
            "worker_id": str(UUID(worker["worker_id"])),
            "attempt_id": str(UUID(attempt["attempt_id"])),
            "start_operation_id": str(UUID(attempt["start_operation_id"])),
            "container_id": container,
        }
    except (KeyError, ValueError, TypeError, AttributeError):
        raise CaseError("running worker has no complete durable start identity") from None


def docker_time(value):
    try:
        timestamp = datetime.fromisoformat(value)
    except (TypeError, ValueError):
        raise CaseError("Docker did not return its expected wall clock") from None
    if timestamp.tzinfo is None or timestamp.year < 2000:
        raise CaseError("Docker did not return a usable absolute timestamp")
    return timestamp


class DockerDeadline:
    # Restrict output to immutable identity, timing and process facts.
    FIELDS = (
        '{"container_id":{{json .Id}},"running":{{json .State.Running}},'
        '"exit_code":{{json .State.ExitCode}},"started_at":{{json .State.StartedAt}},'
        '"finished_at":{{json .State.FinishedAt}},"restart_count":{{json .RestartCount}},'
        '"oom_killed":{{json .State.OOMKilled}}}'
    )

    def __init__(self, args, env, original, worker, report):
        self.cli = [args.runtime_program, "--host", args.runtime_endpoint]
        self.env = env
        self.container = original["container_id"]
        self.limit = args.autonomous_deadline
        self.grace = args.deadline_grace
        self.report = report
        if object_field(worker, "launch").get("timeout_seconds") != self.limit:
            raise CaseError("the durable worker timeout differs from the probe deadline")
        initial = self.inspect()
        if initial["running"] is not True:
            raise CaseError("Docker did not confirm the worker was running before the fault")
        self.started_at = docker_time(initial["started_at"])
        recorded = object_field(worker, "last_observation")
        if docker_time(recorded.get("started_at")) != self.started_at or initial["restart_count"] != 0:
            raise CaseError("Docker start differs from the authenticated observation")
        anchor_before = time.monotonic()
        result = run_command([*self.cli, "info", "--format", "{{json .SystemTime}}"], env, 4)
        anchor_after = time.monotonic()
        try:
            runtime_clock = docker_time(json.loads(result.stdout))
        except ValueError:
            raise CaseError("Docker wall clock response was not valid JSON") from None
        self.age = (runtime_clock - self.started_at).total_seconds()
        if self.age < 0:
            raise CaseError("Docker wall clock precedes worker start")
        self.anchor_before = anchor_before
        self.anchor_after = anchor_after
        self.earliest_deadline = anchor_before + self.limit - self.age
        self.latest_deadline = anchor_after + self.limit - self.age
        self.report.update(
            timeout_seconds=self.limit,
            grace_seconds=self.grace,
            docker_started_at=self.started_at.isoformat(),
            docker_clock_anchor=runtime_clock.isoformat(),
            clock_sample_width_seconds=round(anchor_after - anchor_before, 3),
        )

    def inspect(self):
        result = run_command(
            [*self.cli, "container", "inspect", "--format", self.FIELDS, self.container],
            self.env, 4,
        )
        facts = json_object(result.stdout)
        if (
            facts.get("container_id") != self.container
            or type(facts.get("running")) is not bool
            or type(facts.get("oom_killed")) is not bool
            or type(facts.get("restart_count")) is not int
            or type(facts.get("exit_code")) is not int
            or not isinstance(facts.get("started_at"), str)
            or not isinstance(facts.get("finished_at"), str)
        ):
            raise CaseError("Docker did not return verified facts for the original container")
        return facts

    def observe(self, killed_at):
        kill_elapsed_upper = self.age + killed_at - self.anchor_before
        self.report["kill_elapsed_upper_seconds"] = round(kill_elapsed_upper, 3)
        if killed_at >= self.earliest_deadline:
            raise CaseError("SIGKILL was not confirmed before the worker deadline")
        cutoff = self.latest_deadline + self.grace
        while True:
            sampled_at = time.monotonic()
            facts = self.inspect()
            if docker_time(facts["started_at"]) != self.started_at or facts["restart_count"] != 0:
                raise CaseError("Docker observed a changed start or a restart after the crash")
            if not facts["running"]:
                finished_at = docker_time(facts["finished_at"])
                duration = (finished_at - self.started_at).total_seconds()
                self.report.update(
                    worker_running=False, observed_exit_code=facts["exit_code"],
                    docker_finished_at=finished_at.isoformat(), elapsed_seconds=round(duration, 3),
                    oom_killed=facts["oom_killed"],
                )
                if facts["oom_killed"]:
                    self.report["outcome"] = "terminal_oom"
                    raise CaseError("the worker was OOM-killed during the deadline probe")
                if duration < self.limit:
                    self.report["outcome"] = "terminal_before_deadline"
                    raise CaseError("an early stop did not prove preservation of the original deadline")
                if duration > self.limit + self.grace:
                    self.report["outcome"] = "terminal_after_grace"
                    raise CaseError("worker terminated after the autonomous deadline and grace")
                if duration < kill_elapsed_upper or facts["exit_code"] not in {137, 143}:
                    raise CaseError("Docker did not prove a signalled stop after the CLI crash")
                self.report["outcome"] = "terminal_within_grace"
                return
            if sampled_at >= cutoff:
                self.report.update(
                    outcome="running_after_grace", worker_running=True,
                    elapsed_at_least_seconds=round(self.age + sampled_at - self.anchor_after, 3),
                )
                raise CaseError("the same Docker worker was still running after the autonomous deadline and grace")
            time.sleep(max(0, min(0.5, cutoff - time.monotonic())))


def run_case(args, config, evidence, report):
    binary = args.bin.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise CaseError("the CLI executable is required", status="BLOCKED")
    with binary.open("rb") as executable:
        report["cli_sha256"] = hashlib.file_digest(executable, "sha256").hexdigest()
    control = ["--config-dir", config, "--gateway", args.gateway]
    runtime_program = ["--runtime-program", args.runtime_program] if args.runtime_program else []
    endpoint = ["--runtime-endpoint", args.runtime_endpoint] if args.runtime_endpoint else []
    rootful = ["--allow-rootful-runtime"] if args.allow_rootful_runtime else []
    env = {name: os.environ[name] for name in ("PATH", "HOME", "TMPDIR", "LANG") if name in os.environ}

    report["stage"] = "preflight"
    report["audit_before"] = audit(binary, control, env)
    previous = session_checkpoints(config)
    project = evidence / "project"
    project.mkdir(mode=0o700)
    # A fractional suffix gives each harmless command a unique audit identity.
    command = ["/usr/bin/sleep", f"120.{uuid4().int:039d}"]
    session = None
    cleanup_needed = True

    def operate(action, session_id, extra=(), timeout=20, check=True):
        return run_command(
            [binary, action, *control, "--session", session_id, *extra],
            env, timeout, check=check,
        )

    def discover(timeout):
        candidates = session_checkpoints(config) - previous
        if len(candidates) > 1:
            raise CaseError("a second session used this QA configuration")
        for candidate in candidates:
            response = operate("worker-status", candidate, timeout=timeout, check=False)
            if response.returncode == 0:
                status = json_object(response.stdout)
                worker = worker_for_session(status, candidate, command)
                if worker is not None:
                    return candidate, worker
        return None, None

    report["stage"] = "await_running"
    worker_timeout = args.autonomous_deadline if args.autonomous_deadline is not None else 150
    child = subprocess.Popen(
        [binary, "run", *control, "--project", project, "--runtime", "docker",
         *runtime_program, *endpoint, *rootful, "--profile", "observe", "--network", "none",
         "--image", args.image, "--timeout", str(worker_timeout), "--", *command],
        cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    try:
        deadline = time.monotonic() + args.start_timeout
        while child.poll() is None and (remaining := deadline - time.monotonic()) > 0:
            candidate, worker = discover(min(remaining, 20))
            if candidate is not None:
                session = candidate
                report["session_id"] = session
                observation = object_field(worker, "last_observation")
                if worker.get("state") == "running" and observation.get("running") is True:
                    break
            time.sleep(0.5)
        else:
            raise CaseError("the launched worker was not durably observed running before the deadline")

        # A worker observed as running comes from an authenticated status of this exact command.
        original = instance(worker)
        report["originally_running"] = {"state": "running", **original}
        observation = None
        if args.autonomous_deadline is not None:
            report["stage"] = "autonomous_preflight"
            report["autonomous_deadline"] = {"outcome": "unverified"}
            observation = DockerDeadline(args, env, original, worker, report["autonomous_deadline"])
            if time.monotonic() >= observation.earliest_deadline:
                raise CaseError("the worker deadline was too close to induce a proven pre-deadline crash")

        report["stage"] = "kill_cli"
        child.kill()
        if child.wait(timeout=10) != -signal.SIGKILL:
            raise CaseError("the owned test CLI did not exit on SIGKILL")
        killed_at = time.monotonic()
        report["cli_sigkill_confirmed"] = True
        autonomous_error = None
        if observation is not None:
            report["stage"] = "autonomous_deadline"
            try:
                observation.observe(killed_at)
            except CaseError as error:
                autonomous_error = error
                report["autonomous_deadline"]["diagnostic"] = str(error)

        report["stage"] = "resume"
        resumed = json_object(operate(
            "worker-resume", session, (*runtime_program, "--wait-seconds", "0"), timeout=45,
        ).stdout)
        worker = worker_for_session(resumed, session, command)
        observed = object_field(resumed, "runtime_observation")
        if worker is None or instance(worker) != original:
            raise CaseError("resume did not recover the original instance and start claim")
        running = (
            worker.get("state") == "running" and observed.get("id") == original["container_id"]
            and object_field(observed, "process").get("running") is True
        )
        if not running and not (observation is not None and worker.get("state") == "terminal"):
            raise CaseError("resume did not prove the expected runtime state")
        report["original_worker_reused"] = True
        report["original_container_reused"] = True
        report["original_start_reused"] = True
        image_id = object_field(worker, "launch").get("image_id")
        if isinstance(image_id, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", image_id):
            report["worker_image_sha256"] = image_id

        report["stage"] = "stop"
        operate("worker-stop", session, runtime_program, timeout=90)
        terminal = json_object(operate("worker-status", session).stdout)
        worker = worker_for_session(terminal, session, command)
        if worker is None or instance(worker) != original:
            raise CaseError("worker identity changed after stop")
        observed = object_field(worker, "last_observation")
        report["terminal_state"] = worker.get("state") if worker.get("state") == "terminal" else "unknown"
        report["quota_held"] = worker.get("quota_held") if type(worker.get("quota_held")) is bool else None
        removed = worker.get("cleanup_removed")
        report["cleanup_removed"] = removed if type(removed) is bool else None
        exit_code = observed.get("exit_code")
        report["terminal_exit_code"] = exit_code if type(exit_code) is int else None
        if (
            worker.get("state") != "terminal" or worker.get("quota_held") is not False
            or removed is not True or observed.get("running") is not False
            or report["terminal_exit_code"] not in {137, 143}
        ):
            raise CaseError("stop did not prove a signalled terminal result, cleanup and quota release")
        cleanup_needed = False

        report["stage"] = "audit"
        report["audit_after"] = audit(binary, control, env)
        if report["audit_after"]["event_count"] <= report["audit_before"]["event_count"]:
            raise CaseError("the audit did not record new events")
        if autonomous_error is not None:
            report["stage"] = "autonomous_deadline"
            raise autonomous_error
    finally:
        if child.poll() is None:
            child.kill()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                report["owned_cli_cleanup"] = "unconfirmed"
        if cleanup_needed:
            report["recovery_cleanup"] = "unknown"
            try:
                if session is None:
                    session, _ = discover(10)
                if session is not None:
                    report["session_id"] = session
                    operate("worker-stop", session, runtime_program, timeout=90, check=False)
                    final = json_object(operate("worker-status", session).stdout)
                    worker = worker_for_session(final, session, command)
                    if worker is not None and worker.get("quota_held") is False and (
                        worker.get("state") == "cancelled"
                        or worker.get("state") == "terminal" and worker.get("cleanup_removed") is True
                    ):
                        report["recovery_cleanup"] = "confirmed"
            except (CaseError, OSError):
                pass


def main():
    args = arguments()
    evidence = args.evidence_dir.absolute()
    try:
        evidence.mkdir(mode=0o700)
    except OSError:
        print("Evidence directory must be new with an existing writable parent.")
        return 2

    report = {
        "status": "FAIL",
        "stage": "setup",
        "started_at": datetime.now(timezone.utc).isoformat(),
        "runtime": "docker",
        "rootful_exception_requested": args.allow_rootful_runtime,
    }
    try:
        config = args.config_dir.resolve()
        with case_config_lock(config, exclusive=True):
            run_case(args, config, evidence, report)
    except CaseError as error:
        report.update(status=error.status, diagnostic=str(error))
    except OSError:
        report["diagnostic"] = "a required local file or process could not be accessed"
    except subprocess.TimeoutExpired:
        report["diagnostic"] = "the owned CLI did not exit before the deadline"
    except KeyboardInterrupt:
        report["diagnostic"] = "interrupted; inspect this case's durable session before any retry"
    else:
        report.update(status="PASS", stage="complete")
    report["ended_at"] = datetime.now(timezone.utc).isoformat()
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    with (evidence / "result.json").open("x", encoding="utf8") as file:
        file.write(encoded)
    print(encoded, end="")
    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2}[report["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
