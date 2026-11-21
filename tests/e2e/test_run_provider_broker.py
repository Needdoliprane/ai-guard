"""Offline contract tests for the provider broker recipe's normal CLI invocations."""

import contextlib
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import io
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from uuid import UUID, uuid4


spec = spec_from_file_location("provider_case", Path(__file__).with_name("run_provider_case.py"))
recipe = module_from_spec(spec)
spec.loader.exec_module(recipe)

SESSION = "fc41ff6d-0cd5-4127-924d-ce43aaa02c85"
OTHER_EXPORT_REQUEST = "4012e661-d3d9-495c-9620-d30196b7caa0"
SCAN_DIGEST = "2" * 64


def result(value, code=0):
    return SimpleNamespace(returncode=code, stdout=json.dumps(value).encode())


class BrokerRecipeTests(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        for name in ("source", "exports", "control", "broker", "evidence"):
            (self.root / name).mkdir(mode=0o700)
        self.evidence = self.root / "evidence"
        self.args = SimpleNamespace(
            broker_root=self.root / "broker", broker_profile="qa-broker",
            broker_token_file=self.root / "control/secrets/runner", config_dir=self.root / "control",
            broker_source_root=self.root / "source", broker_export_root=self.root / "exports",
            runtime="docker", client="claude-code", auth_mode="subscription", client_profile="qa-subscription",
            image="sha256:" + "a" * 64, worker_provenance=True, timeout=35,
        )
        self.report = {"audit_before": {"event_count": 1}}
        self.calls = []
        self.operation = None
        self.marker = None
        self.run_outcome = "matched"
        self.durable_state = "terminal"
        self.export_digest_mismatch = False
        self.export_request_mismatch = False
        self.browser_calls = 0
        self.browser_state = "pass"
        self.browser_input = None
        self.browser_response = None

    def job(self):
        outcome = {"state": self.durable_state}
        if self.durable_state == "terminal":
            outcome["result"] = {
                "session_id": SESSION, "volume": "fixture-volume", "container_id": "1" * 64,
                "exit_code": 0, "oom_killed": False, "changed_files": 1, "manifest_digest": SCAN_DIGEST,
                "output_captured": False, "result_audit_seq": 12, "scan_audit_seq": 22, "cleanup_audit_seq": 19,
            }
        return {"version": 1, "operation_id": self.operation, "cancel_requested": False, "outcome": outcome}

    def call(self, command, environment, timeout, *, check):
        self.assertEqual(environment, {})
        self.assertGreater(timeout, 0)
        self.assertFalse(check)
        argv = list(map(str, command))
        self.calls.append(argv)
        operation = argv[argv.index("--operation-id") + 1]
        if self.operation is None:
            self.operation = operation
        self.assertEqual(operation, self.operation)
        self.assertEqual(str(UUID(operation)), operation)
        identity_path = self.evidence / "broker-operation.json"
        identity = json.loads(identity_path.read_text(encoding="utf8"))
        self.assertEqual(identity_path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(identity["operation_id"], operation)

        if argv[1] == "run":
            self.assertEqual(argv[argv.index("--model") + 1], recipe.MODELS[self.args.client])
            self.assertEqual(argv[argv.index("--auth-mode") + 1], self.args.auth_mode)
            self.assertEqual(argv[argv.index("--client-profile") + 1], self.args.client_profile)
            self.assertEqual(argv[argv.index("--project") + 1], identity["project"])
            for legacy in ("--interactive", "--image", "--runtime", "--gateway", "--providers-file", "--config-dir"):
                self.assertNotIn(legacy, argv)
            prompt = argv[argv.index("--prompt") + 1]
            self.marker = prompt.split("no trailing newline: ", 1)[1].splitlines()[0].encode("ascii")
            if self.run_outcome == "lost":
                raise recipe.CaseError("command deadline exceeded")
            if self.run_outcome == "invalid":
                return result({**self.job(), "operation_id": SESSION})
            return result(self.job(), code=0 if self.durable_state == "terminal" else 1)

        self.assertEqual(argv[1], "broker")
        if argv[2] == "status":
            status = {"operation_id": self.operation, "state": "absent"}
            if self.marker is not None:
                status.update(state="accepted", worker_state="terminal", quota_held=False, runtime_status="exited")
            return result({"status": {"status": status}})
        if argv[2] == "result":
            return result({"result": {"result": self.job()}})

        self.assertEqual(argv[2], "export")
        export_command = argv[argv.index("--command-id") + 1]
        self.assertEqual(export_command, identity["export_command_id"])
        destination = Path(argv[argv.index("--destination") + 1])
        self.assertEqual(str(destination), identity["destination"])
        self.assertFalse(destination.exists())
        destination.mkdir()
        original = recipe.ORIGINAL_BYTES
        for name, data in ((recipe.ORIGINAL_FILE, original), (recipe.RESULT_FILE, self.marker)):
            target = destination / name
            target.write_bytes(data)
            target.chmod(0o644)
        original_hash = hashlib.sha256(original).hexdigest()
        result_hash = hashlib.sha256(self.marker).hexdigest()
        manifest = (
            f'{{"original.txt":{{"sha256":"{original_hash}","bytes":{len(original)},"mode":420}},'
            f'"result.txt":{{"sha256":"{result_hash}","bytes":{len(self.marker)},"mode":420}}}}'
        )
        digest = hashlib.sha256(b"ai-guard:workspace-manifest:v1\0" + manifest.encode()).hexdigest()
        self.assertNotEqual(SCAN_DIGEST, digest)
        request_id = OTHER_EXPORT_REQUEST if self.export_request_mismatch else export_command
        return result({"export_acknowledged": {"operation_id": operation, "request_id": request_id,
            "manifest_digest": SCAN_DIGEST if self.export_digest_mismatch else digest, "files": 2}})

    def browser(self, node, environment, report, *, preflight=False):
        self.browser_calls += 1
        self.assertEqual(node, "fixture-node")
        self.assertFalse(preflight)
        self.assertFalse(report.get("worker_id"))
        self.assertFalse(report.get("worker_image_id"))
        self.assertFalse(report.get("authority_id"))
        path = self.evidence / "broker-browser-proof.json"
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        supplied = Path(environment["AI_GUARD_TEST_BROKER_PROOF"])
        self.assertTrue(supplied.is_absolute())
        self.assertEqual(supplied, path)
        self.browser_input = json.loads(supplied.read_text(encoding="utf8"))
        self.assertEqual(set(environment), {"AI_GUARD_TEST_BROKER_PROOF"})
        proof = self.browser_input
        self.assertEqual(proof["operation_id"], self.operation)
        self.assertEqual(proof["export_command_id"], proof["export_request_id"])
        self.assertEqual(proof["scan_manifest_sha256"], SCAN_DIGEST)
        self.assertNotEqual(proof["export_manifest_sha256"], SCAN_DIGEST)
        self.assertEqual(proof["export_file_count"], 2)
        self.assertEqual(proof["file_name"], recipe.RESULT_FILE)
        if self.browser_state == "blocked":
            raise recipe.CaseError("browser fixture unavailable", status="BLOCKED")
        self.browser_response = {
            "status": "PASS", "mode": "broker_session_trace", "executionPath": "broker_batch", "transport": "batch",
            "operationId": proof["operation_id"], "session": SESSION,
            "containerId": proof["container_id"], "workerImageId": proof["worker_image_id"],
            "workerId": "34982a3c-cfe2-4087-b117-fc4c7c237b82", "authorityId": "09cd8a6c-463f-4ceb-863a-ce51424e6e1d",
            "client": proof["client"], "authMode": proof["auth_mode"], "clientProfile": proof["client_profile"],
            "requestedModel": proof["model_requested"], "runtime": proof["runtime"],
            "exportRequestId": proof["export_request_id"], "fileSha256": proof["file_sha256"],
            "scanManifestSha256": proof["scan_manifest_sha256"], "exportManifestSha256": proof["export_manifest_sha256"],
            "decisionAuditSeq": 10, "resultAuditSeq": 12, "cleanupAuditSeq": 19, "scanAuditSeq": 22, "exportAuditSeq": 23,
            **{f"{kind}EventId": str(uuid4()) for kind in ("decision", "terminal", "cleanup", "scan", "export")},
            "count": 5, "auditExportSha256": "5" * 64, "browser": "157.0.8103.79",
            "workerProvenance": "displayed_fields_match_registry", "untrusted_extra": "fixture-marker-not-for-output",
        }
        return self.browser_response

    def exercise(self, status):
        with patch.object(recipe, "run_command", self.call), patch.object(
            recipe, "audit", return_value={"event_count": 24}
        ), patch.object(recipe, "browser_command", self.browser):
            def run():
                recipe.run_broker_case(self.args, self.evidence, self.report, "fixture-bin", [], {},
                                       node="fixture-node", browser_env={})
            if status is None:
                run()
            else:
                with self.assertRaises(recipe.CaseError) as failure:
                    run()
                self.assertEqual(failure.exception.status, status)
        self.assertEqual(sum(command[1] == "run" for command in self.calls), 1)
        self.assertEqual(sum(command[1:3] == ["broker", "result"] for command in self.calls), 1)
        requested = list((self.root / "source").iterdir())
        self.assertEqual(len(requested), 1)
        self.assertEqual((requested[0] / recipe.ORIGINAL_FILE).read_bytes(), recipe.ORIGINAL_BYTES)

    def test_normal_run_binds_export_and_browser_to_the_same_terminal_job(self):
        self.exercise(None)
        self.assertEqual(self.report["cli_exit_status"], 0)
        self.assertEqual(self.report["broker_run_reply"], "matched")
        self.assertEqual(self.report["broker_result_source"], "broker_result")
        self.assertEqual(self.report["broker_cli_proof"], "terminal_and_export_verified")
        self.assertEqual(self.report["session_id"], SESSION)
        self.assertEqual(self.report["broker_export_request_id"], self.report["broker_export_command_id"])
        self.assertNotEqual(self.report["broker_export_manifest_sha256"], self.report["broker_terminal"]["manifest_digest"])
        self.assertEqual(self.browser_calls, 1)
        trace = self.report["broker_browser"]
        self.assertEqual(self.report["worker_id"], trace["workerId"])
        self.assertEqual(self.report["worker_image_id"], trace["workerImageId"])
        self.assertEqual(self.report["authority_id"], trace["authorityId"])
        self.assertNotIn("browser", self.report)
        self.assertNotIn("untrusted_extra", trace)

    def test_blocked_browser_keeps_the_private_proof_for_read_only_replay(self):
        self.browser_state = "blocked"
        self.exercise("BLOCKED")
        self.assertEqual(self.browser_calls, 1)
        self.assertEqual(self.report["stage"], "broker_browser")
        self.assertTrue((self.evidence / "broker-browser-proof.json").is_file())
        self.assertNotIn("broker_browser", self.report)
        self.assertNotIn("worker_id", self.report)

    def test_browser_trace_rejects_wrong_correlations(self):
        self.exercise(None)
        for field, value in [
            ("status", "FAIL"), ("executionPath", "legacy"), ("containerId", "6" * 64),
            ("clientProfile", "different-profile"), ("requestedModel", "alias"), ("session", str(uuid4())),
            ("cleanupAuditSeq", self.browser_response["scanAuditSeq"]), ("resultAuditSeq", 13),
            ("scanEventId", self.browser_response["terminalEventId"]), ("authorityId", str(UUID(int=0))),
            ("exportRequestId", OTHER_EXPORT_REQUEST), ("exportManifestSha256", SCAN_DIGEST),
            ("workerProvenance", "incomplete"), ("count", 4),
        ]:
            with self.subTest(field=field), self.assertRaises(recipe.CaseError):
                recipe.broker_browser_evidence({**self.browser_response, field: value}, self.browser_input,
                                               worker_provenance=True)

    def test_lost_run_reply_can_read_and_export_same_job_once_without_turning_green(self):
        self.run_outcome = "lost"
        self.args.client, self.args.auth_mode = "codex", "api"
        self.exercise("FAIL")
        self.assertIsNone(self.report["cli_exit_status"])
        self.assertEqual(self.report["broker_run_reply"], "unavailable")
        self.assertEqual(self.report["broker_export_request_id"], self.report["broker_export_command_id"])
        self.assertEqual(sum(command[1:3] == ["broker", "export"] for command in self.calls), 1)
        self.assertEqual(self.browser_calls, 0)
        self.assertFalse((self.evidence / "broker-browser-proof.json").exists())

    def test_pending_reread_never_exports_or_resubmits(self):
        self.durable_state = "pending"
        self.exercise("FAIL")
        self.assertEqual(self.report["broker_outcome"], "pending")
        self.assertTrue(all(argv[1:3] != ["broker", "export"] for argv in self.calls))

    def test_uncorrelated_initial_reply_is_rejected(self):
        self.run_outcome = "invalid"
        self.exercise("FAIL")
        self.assertEqual(self.report["broker_run_reply"], "invalid")
        self.assertTrue(all(argv[1:3] != ["broker", "export"] for argv in self.calls))

    def test_export_ack_must_match_published_manifest(self):
        self.export_digest_mismatch = True
        self.exercise("FAIL")
        self.assertEqual(self.report["stage"], "broker_export")
        self.assertNotIn("broker_cli_proof", self.report)

    def test_export_ack_must_match_the_persisted_command_id(self):
        self.export_request_mismatch = True
        self.exercise("FAIL")
        self.assertEqual(self.report["stage"], "broker_export")
        self.assertNotIn("broker_export_request_id", self.report)
        self.assertNotIn("broker_cli_proof", self.report)
        self.assertEqual(sum(command[1:3] == ["broker", "export"] for command in self.calls), 1)

    def test_broker_tty_and_partial_broker_selection_are_refused_during_parsing(self):
        common = ["recipe", "--bin", "/fixture-bin", "--config-dir", "/config", "--gateway",
                  "http://127.0.0.1:18787", "--dashboard", "http://127.0.0.1:14000", "--runtime", "docker",
                  "--image", "sha256:" + "a" * 64, "--client", "codex", "--auth-mode", "api", "--client-profile",
                  "qa-api", "--evidence-dir", "/fixture-evidence"]
        cases = [(["--broker-root", "/broker", "--transport", "tty"], "supports batch only"),
                 (["--broker-root", "/broker"], "requires the profile and all four absolute broker paths")]
        for extra, diagnostic in cases:
            with self.subTest(extra=extra), patch.object(sys, "argv", common + extra), contextlib.redirect_stderr(io.StringIO()) as stderr:
                with self.assertRaises(SystemExit) as error:
                    recipe.arguments()
                self.assertEqual(error.exception.code, 2)
                self.assertIn(diagnostic, stderr.getvalue())
        with patch.object(sys, "argv", common + ["--transport", "tty"]):
            self.assertIsNone(recipe.arguments().broker_root)


if __name__ == "__main__":
    unittest.main()
