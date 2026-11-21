"""Offline contract checks for the managed provider metadata preflight."""

from contextlib import redirect_stdout
from dataclasses import dataclass
from importlib.util import module_from_spec, spec_from_file_location
import io
from itertools import product
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from uuid import uuid4


spec = spec_from_file_location("provider_case", Path(__file__).with_name("run_provider_case.py"))
recipe = module_from_spec(spec)
spec.loader.exec_module(recipe)
RAW = "synthetic-forbidden-report-sentinel"
CASES = tuple(product(recipe.MODELS, ("api", "subscription"), ("batch", "tty")))


def finding(code, status, axis="providers", profile=None):
    return {
        "axis": axis, "code": code, "status": status, "message": RAW,
        "subjects": None if profile is None else {
            "kind": "provider_profiles", "total": 1,
            "ids": [{"id": profile, "masked": False}], "omitted": 0,
        },
    }


def reference(profile, presence="present"):
    return {
        "owner": {"kind": "provider_profile", "id": profile, "masked": False},
        "role": "provider", "consumer": "private_file_reader", "display": RAW,
        "entry_kind": "regular_file" if presence == "present" else None,
        "presence": presence, "private_rule": "satisfied" if presence == "present" else "not_evaluated",
        "content": "not_read",
    }


def diagnostic(client, mode, *, absent=False):
    client_kind = client.replace("-", "_")
    alternate_mode = "api" if mode == "subscription" else "subscription"
    alternate = f"qa-{client}-{alternate_mode}"
    profile = f"qa-{client}-{mode}"
    resolved = {
        "profile": {"id": profile, "masked": False}, "revision": 1, "client": client_kind,
        "auth_mode": mode, "model": recipe.MODELS[client], "chosen_by": "profile",
    }
    reply = {
        "schema": "ai-guard-diagnose-report", "version": 1,
        "mode": {"network": "none", "mutation": "none", "credential_contents_read": "none",
                 "active_checks_executed": [], "files_read_for_content": ["provider_profiles"]},
        "providers": {
            "file": {"state": "loaded", "display": RAW},
            "profiles": [resolved, {**resolved, "auth_mode": alternate_mode,
                                    "profile": {"id": alternate, "masked": False}}],
            "selection": {
                "requested": {"client": "explicit", "auth_mode": "explicit", "profile": "explicit", "model": "explicit"},
                "resolved": resolved,
            },
        },
        "credential_references": [reference(profile, "absent" if absent else "present"), reference(alternate)],
        "findings": [finding("provider.selection_resolved", "ok", profile=profile)],
    }
    if absent:
        code = "provider.login_required" if mode == "subscription" else "provider.credential_missing"
        reply["findings"].append(finding(code, "error", profile=profile))
    return reply


class ProviderPreflightTests(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.binary = self.root / "agentsandbox"
        self.companion = self.root / "ai-guard-diagnose"
        self.config = self.root / "config"
        self.config.mkdir(mode=0o700)
        for executable in (self.binary, self.companion):
            executable.touch(mode=0o700)

    def invoke(self, client, mode, transport, payload, *, code=3, explicit=None, failure=None, browser_expected=False):
        evidence = self.root / str(uuid4())
        profile = f"qa-{client}-{mode}"
        argv = [
            "run_provider_case.py", "--bin", str(self.binary), "--config-dir", str(self.config),
            "--gateway", "http://127.0.0.1:8010", "--dashboard", "http://127.0.0.1:8011",
            "--runtime", "docker", "--image", "sha256:" + "a" * 64,
            "--client", client, "--auth-mode", mode, "--transport", transport,
            "--client-profile", profile, "--evidence-dir", str(evidence),
        ]
        providers = []
        if explicit:
            argv += ["--diagnose-bin", str(explicit), "--providers-file", str(self.config / "providers.qa.json")]
            providers = ["--providers-file", self.config / "providers.qa.json"]

        def offline_command(command, env, timeout, *, check):
            self.assertEqual(command, [explicit or self.companion, "--config-dir", self.config, *providers,
                                       "--client", client, "--auth-mode", mode, "--client-profile", profile,
                                       "--model", recipe.MODELS[client], "--json"])
            self.assertTrue(0 < timeout <= 10)
            self.assertFalse(check)
            self.assertLessEqual(set(env), {"PATH", "HOME", "TMPDIR", "LANG"})
            if failure is not None:
                raise failure
            return SimpleNamespace(returncode=code, stdout=json.dumps(payload).encode(), stderr=RAW.encode())

        output = io.StringIO()
        with (patch.object(sys, "argv", argv), patch.dict(os.environ, {"OPENAI_API_KEY": RAW}),
              patch.object(recipe, "run_command", side_effect=offline_command) as commands,
              patch.object(recipe.shutil, "which", return_value="fixture-node"),
              patch.object(recipe, "browser_command", side_effect=recipe.CaseError("offline browser boundary")) as browser,
              patch.object(recipe, "audit") as audit, redirect_stdout(output)):
            exit_code = recipe.main()
        self.assertEqual(commands.call_count, int((explicit or self.companion).is_file()
                                                 and os.access(explicit or self.companion, os.X_OK)))
        audit.assert_not_called()
        if browser_expected:
            self.assertEqual(exit_code, 1)
            self.assertEqual(browser.call_count, 1)
            self.assertTrue(browser.call_args.kwargs["preflight"])
        else:
            self.assertEqual(exit_code, 2)
            browser.assert_not_called()
        self.assertEqual(list(evidence.iterdir()), [evidence / "result.json"])
        encoded = (evidence / "result.json").read_text(encoding="utf8")
        self.assertNotIn(RAW, encoded)
        self.assertEqual(output.getvalue(), encoded)
        return json.loads(encoded)

    def assert_blocked(self, report, code, *, selection_present=False):
        self.assertEqual((report["status"], report["stage"]), ("BLOCKED", "provider_preflight"))
        proof = report["provider_preflight"]
        self.assertEqual(proof["code"], code)
        self.assertEqual(set(proof), {"code", "selection"} if selection_present else {"code"})

    def test_missing_selected_credential_blocks_all_eight_cases_before_browser(self):
        for client, mode, transport in CASES:
            with self.subTest(client=client, mode=mode, transport=transport):
                report = self.invoke(client, mode, transport, diagnostic(client, mode, absent=True))
                code = "provider.login_required" if mode == "subscription" else "provider.credential_missing"
                self.assert_blocked(report, code, selection_present=True)
                self.assertEqual(report["provider_preflight"]["selection"], {
                    "client": client.replace("-", "_"), "auth_mode": mode,
                    "profile": f"qa-{client}-{mode}", "model": recipe.MODELS[client],
                })

    def test_all_eight_exact_selections_reach_browser_boundary_despite_unrelated_findings(self):
        for index, (client, mode, transport) in enumerate(CASES):
            with self.subTest(client=client, mode=mode, transport=transport):
                reply = diagnostic(client, mode)
                alternate = reply["credential_references"][1]["owner"]["id"]
                reply["credential_references"][1] = reference(alternate, "absent")
                reply["findings"].append(finding("provider.unselected_profile_reference_absent", "warning", profile=alternate))
                global_exit = (0, 3, 4)[index % 3]
                if global_exit == 3:
                    reply["findings"].append(finding("layout.target_absent", "error", "layout"))
                elif global_exit == 4:
                    reply["findings"].append(finding("reference.metadata_unavailable", "unknown", "credential_references"))
                report = self.invoke(client, mode, transport, reply, code=global_exit, browser_expected=True)
                self.assertEqual(report["stage"], "browser_preflight")
                self.assertEqual(report["provider_preflight"]["code"], "selected_reference_metadata_verified")

    def test_mode_model_and_profile_substitutions_are_rejected_for_all_eight_selections(self):
        aliases = {"codex": "gpt-6", "claude-code": "haiku"}
        for client, mode, transport in CASES:
            alternate_mode = "api" if mode == "subscription" else "subscription"
            profile = f"qa-{client}-{mode}"
            other_model = next(model for name, model in recipe.MODELS.items() if name != client)
            substitutions = (
                ("auth_mode", "alternate_mode", alternate_mode),
                ("model", "unpinned_alias", aliases[client]),
                ("model", "other_client_model", other_model),
                ("profile", "alternate_profile", {"id": f"qa-{client}-{alternate_mode}", "masked": False}),
                ("profile", "masked_profile", {"id": profile, "masked": True}),
                ("profile", "unknown_profile", {"id": RAW, "masked": False}),
                ("resolved", "coherent_fallback", None),
            )
            for key, label, value in substitutions:
                with self.subTest(client=client, mode=mode, transport=transport, substitution=label):
                    reply = diagnostic(client, mode)
                    selection = reply["providers"]["selection"]
                    if key == "resolved":
                        # The diagnostic silently resolves the other configured public profile and mode.
                        selection["resolved"] = dict(reply["providers"]["profiles"][1])
                    else:
                        selection["resolved"][key] = value
                    report = self.invoke(client, mode, transport, reply)
                    self.assert_blocked(report, "diagnostic_selection_mismatch")
                    # The report keeps the public requested selection, never the substituted one.
                    self.assertEqual(
                        {name: report[name] for name in ("client", "auth_mode", "transport", "model_requested", "profile")},
                        {"client": client, "auth_mode": mode, "transport": transport,
                         "model_requested": recipe.MODELS[client], "profile": profile})

    def test_explicit_selection_unavailable_reports_only_closed_code(self):
        reply = diagnostic("claude-code", "subscription")
        reply["providers"]["selection"]["resolved"] = None
        reply["findings"] = [finding(RAW, "error"), finding("provider.selection_no_profile", "error")]
        self.assert_blocked(self.invoke("claude-code", "subscription", "batch", reply), "provider.selection_no_profile")

    def test_selected_unsafe_and_unverified_metadata_block(self):
        for unsafe in (False, True):
            with self.subTest(unsafe=unsafe):
                reply = diagnostic("codex", "api")
                reply["credential_references"][0].update(private_rule="violated" if unsafe else "not_evaluated")
                code = "provider.credential_unsafe" if unsafe else "diagnostic_reference_not_verified"
                if unsafe:
                    reply["findings"].append(finding(code, "error", profile="qa-codex-api"))
                self.assert_blocked(self.invoke("codex", "api", "batch", reply), code, selection_present=True)

    def test_unavailable_diagnostic_companion_blocks(self):
        for missing in (False, True):
            with self.subTest(missing=missing):
                if missing:
                    self.companion.unlink()
                else:
                    self.companion.chmod(0o600)
                self.assert_blocked(self.invoke("codex", "api", "batch", None), "diagnostic_executable_unavailable")

    def test_explicit_companion_and_providers_path_are_used(self):
        explicit = self.root / "qa-dedicated-diagnose"
        explicit.touch(mode=0o700)
        self.companion.unlink()
        reply = diagnostic("codex", "subscription", absent=True)
        self.assert_blocked(self.invoke("codex", "subscription", "batch", reply, explicit=explicit),
                            "provider.login_required", selection_present=True)

    def test_command_timeout_and_nonreport_exit_remain_blocked(self):
        self.assert_blocked(self.invoke("codex", "api", "batch", None, code=1), "diagnostic_report_unavailable")
        self.assert_blocked(self.invoke("codex", "api", "batch", None, failure=recipe.CaseError(RAW)),
                            "diagnostic_report_unavailable")

    def test_report_of_credential_content_read_bars_the_browser(self):
        reply = diagnostic("claude-code", "api")
        reply["mode"]["credential_contents_read"] = "detected_after_read"
        self.assert_blocked(self.invoke("claude-code", "api", "batch", reply), "diagnostic_report_invalid")


class ProviderBrowserStageTests(unittest.TestCase):
    """A batch case that passed its run, export and audit reaches the live dashboard check."""

    def test_control_deadline_prevents_clean_result_even_with_natural_exit(self):
        summary = {
            "cleanup": {"state": "removed"},
            "exit_code": 0,
            "timed_out": False,
            "output_captured": False,
            "changed_files": 0,
            "postprocessing_error": None,
            "transport_stop_reason": None,
        }
        recipe.require_run_summary(summary, changes=0)
        summary["control_deadline"] = {
            "deadline": "original_execution_deadline", "stop": "already_stopped",
        }
        with self.assertRaises(recipe.CaseError):
            recipe.require_run_summary(summary, changes=0)

    def test_batch_case_hands_the_dashboard_client_kind_to_the_live_browser(self):
        for client in recipe.MODELS:
            with self.subTest(client=client), TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                binary, companion, config = root / "agentsandbox", root / "ai-guard-diagnose", root / "config"
                config.mkdir(mode=0o700)
                for executable in (binary, companion):
                    executable.touch(mode=0o700)
                evidence, session, worker_id = root / "evidence", str(uuid4()), str(uuid4())
                argv = [
                    "run_provider_case.py", "--bin", str(binary), "--config-dir", str(config),
                    "--gateway", "http://127.0.0.1:8010", "--dashboard", "http://127.0.0.1:8011",
                    "--runtime", "docker", "--image", "sha256:" + "a" * 64, "--client", client,
                    "--auth-mode", "api", "--client-profile", f"qa-{client}-api", "--evidence-dir", str(evidence),
                ]

                def reply(value):
                    return SimpleNamespace(returncode=0, stdout=json.dumps(value).encode(), stderr=b"")

                def offline_command(command, env, timeout, *, check=True):
                    if command[0] == companion:
                        return reply(diagnostic(client, "api"))
                    self.assertEqual(command[0], binary)
                    if command[1] == "run":
                        return reply({"session_id": session, "cleanup": {"state": "removed"}})
                    if command[1] == "worker-status":
                        return reply({"session_id": session,
                                      "worker": {"state": "terminal", "quota_held": False, "worker_id": worker_id}})
                    self.assertEqual(command[1], "export-workspace")
                    return reply({})

                browsers = []

                def browser(node, env, report, *, preflight=False):
                    browsers.append((preflight, dict(env)))
                    return {"status": "PASS", "mode": "preflight"} if preflight else {"status": "PASS"}

                audits = [{"event_count": count, "head_hash": "b" * 64, "valid": True} for count in (1, 2)]
                with (patch.object(sys, "argv", argv), patch.dict(os.environ, {"ANTHROPIC_API_KEY": RAW}),
                      patch.object(recipe, "run_command", side_effect=offline_command),
                      patch.object(recipe.shutil, "which", return_value="fixture-node"),
                      patch.object(recipe, "browser_command", side_effect=browser),
                      patch.object(recipe, "audit", side_effect=audits),
                      patch.object(recipe, "require_run_summary"), patch.object(recipe, "check_files"),
                      patch.object(recipe, "observed_worker_image", return_value="sha256:" + "a" * 64),
                      patch.object(recipe, "browser_evidence", return_value={"session_id": session}) as evidence_check,
                      redirect_stdout(io.StringIO())):
                    exit_code = recipe.main()
                report = json.loads((evidence / "result.json").read_text(encoding="utf8"))
                self.assertEqual((exit_code, report["status"], report["stage"]), (0, "PASS", "complete"))
                self.assertEqual([preflight for preflight, _ in browsers], [True, False])
                live = browsers[1][1]
                self.assertEqual(live["AI_GUARD_TEST_CLIENT"], client.replace("-", "_"))
                self.assertEqual((live["AI_GUARD_TEST_SESSION_ID"], live["AI_GUARD_TEST_MODEL"]),
                                 (session, recipe.MODELS[client]))
                self.assertNotIn(RAW, json.dumps(live))
                self.assertEqual(evidence_check.call_args.args[1:3], (session, worker_id))

    def test_failed_tty_retains_only_matched_terminal_facts(self):
        @dataclass(frozen=True)
        class IncompleteConversation:
            cli_exit_status: int = 1
            passed: bool = False
            stage: str = "first_receipt"

        session, other, worker_id = str(uuid4()), str(uuid4()), str(uuid4())
        image = "sha256:" + "a" * 64
        volume = "fixture-volume"
        deadline = "authorized worker duration elapsed"
        summary = {"session_id": session, "volume": volume, "exit_code": 137,
                   "timed_out": True, "transport_stop_reason": deadline, "untrusted_detail": RAW}
        control = {"deadline": "original_execution_deadline", "stop": "engine_acknowledged"}
        expired = {**summary, "timed_out": False, "transport_stop_reason": None, "control_deadline": control}
        observation_expired = {"deadline": "observation_window", "stop": "unconfirmed"}
        for result, inspected_session, inspected_worker_id, expected in (
            (summary, session, worker_id, (True, deadline, None)),
            ({**summary, "timed_out": False, "transport_stop_reason": RAW},
             session, worker_id, (False, "unclassified", None)),
            (expired, session, worker_id, (False, None, control)),
            ({**expired, "control_deadline": observation_expired},
             session, worker_id, (False, None, observation_expired)),
            ({**summary, "control_deadline": {**control, "stop": {"detail": RAW}}},
             session, worker_id, (True, deadline, None)),
            (None, session, worker_id, (None, None, None)),
            ({**expired, "session_id": other}, session, worker_id, (None, None, None)),
            ({**expired, "volume": other}, session, worker_id, (None, None, None)),
            ({**expired, "exit_code": 0}, session, worker_id, (None, None, None)),
            ({**expired, "volume": None, "exit_code": None}, session, worker_id, (None, None, None)),
            (summary, other, worker_id, None),
            (expired, session, None, None),
            (expired, session, RAW, None),
            (expired, session, "00000000-0000-0000-0000-000000000000", None),
        ):
            with self.subTest(inspected_session=inspected_session, expected=expected), TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                binary, config, evidence = root / "agentsandbox", root / "control", root / "evidence"
                config.mkdir(mode=0o700)
                binary.touch(mode=0o700)
                args = SimpleNamespace(
                    bin=binary, config_dir=config, evidence_dir=evidence, providers_file=None,
                    gateway="http://127.0.0.1:8010", dashboard="http://127.0.0.1:8011",
                    client="claude-code", auth_mode="api", transport="tty", client_profile="qa-claude-api",
                    runtime="docker", image=image, allow_rootful_runtime=True, worker_provenance=True,
                    runtime_program=None, runtime_endpoint=None, node="fixture-node", broker_root=None, timeout=120,
                )
                launch = {
                    "session_id": session, "runtime_kind": "docker", "image_id": image, "authority_id": "fixture-authority",
                    "provider_resolution": {"client": "claude_code", "auth_mode": "api", "model": "claude-haiku-5-5",
                                            "profile_id": args.client_profile, "transport": "interactive_tty"},
                }
                inspection = {**launch, "container_id": "fixture-container", "spec_hash": "fixture-spec"}
                worker = {
                    "state": "terminal", "quota_held": False, "cleanup_removed": True, "spec_hash": "fixture-spec",
                    "attempt": {"container_id": "fixture-container", "volume_name": volume}, "launch": launch,
                    "last_observation": {"running": False, "status": "exited", "inspection": inspection,
                                         "exit_code": 137, "oom_killed": False},
                }
                if inspected_worker_id is not None:
                    worker["worker_id"] = inspected_worker_id
                status = {"session_id": inspected_session, "worker": worker, "result": result, "detail": RAW}
                driver = SimpleNamespace(run_provider=lambda *args, **kwargs: IncompleteConversation())
                replies = [SimpleNamespace(returncode=0), SimpleNamespace(stdout=json.dumps(status).encode())]
                with (patch.object(recipe, "arguments", return_value=args),
                      patch.object(recipe, "provider_preflight"), patch.object(recipe, "audit", return_value={}),
                      patch.object(recipe.shutil, "which", return_value="fixture-node"),
                      patch.object(recipe, "browser_command") as browser,
                      patch.object(recipe, "run_command", side_effect=replies),
                      patch.object(recipe, "session_checkpoints", side_effect=[set(), {session}]),
                      patch.dict(sys.modules, {"interactive_driver": driver}), redirect_stdout(io.StringIO())):
                    exit_code = recipe.main()
                text = (evidence / "result.json").read_text(encoding="utf8")
                report = json.loads(text)
                self.assertEqual((exit_code, report["status"], report["stage"]), (1, "FAIL", "interactive_conversation"))
                self.assertNotIn(RAW, text)
                self.assertTrue(browser.call_args.kwargs["preflight"])
                browser.assert_called_once()
                if expected is None:
                    self.assertNotIn("tty_terminal_result", report)
                    self.assertNotIn("worker_id", report)
                    self.assertEqual(report["runtime_result"], "unknown")
                else:
                    self.assertEqual(report["worker_id"], worker_id)
                    self.assertEqual(report["tty_terminal_result"], {
                        "exit_code": 137, "oom_killed": False, "cleanup_removed": True,
                        "timed_out": expected[0], "transport_stop_reason": expected[1],
                        "control_deadline": expected[2],
                    })
                    self.assertEqual(report["runtime_result"], "terminal")


class InstalledDiagnosticTests(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("AI_GUARD_QA_PROVIDER_CLI"), "set AI_GUARD_QA_PROVIDER_CLI to the absolute CLI path")
    def test_synthetic_profiles_with_installed_companion(self):
        binary = Path(os.environ["AI_GUARD_QA_PROVIDER_CLI"])
        self.assertTrue(binary.is_absolute(), "AI_GUARD_QA_PROVIDER_CLI must be an absolute path")
        companion = binary.resolve().with_name("ai-guard-diagnose")
        for executable in (binary, companion):
            self.assertTrue(executable.is_file() and os.access(executable, os.X_OK), f"missing executable: {executable}")
        models = {"codex": "gpt-6-luna", "claude-code": "claude-haiku-5-5"}
        subscription_kinds = {"codex": "codex_auth_file", "claude-code": "claude_oauth_env_file"}
        run_local = recipe.run_command
        with TemporaryDirectory(prefix="ai-guard-diagnose-qa-") as directory:
            root = Path(directory).resolve()
            for (client, mode, transport), present in product(CASES, (False, True)):
                with self.subTest(client=client, mode=mode, transport=transport, selected_reference_present=present):
                    config = root / f"{client}-{mode}-{transport}-{int(present)}"
                    config.mkdir(mode=0o700)
                    client_kind = client.replace("-", "_")
                    profile = f"qa-{mode}"
                    profiles = []
                    for auth_mode, kind in (("api", "api_env_file"), ("subscription", subscription_kinds[client])):
                        reference_path = config / f"{auth_mode}.fixture"
                        if present or auth_mode != mode:
                            reference_path.touch(mode=0o600)
                        profiles.append({"id": f"qa-{auth_mode}", "revision": 1, "client": client_kind,
                                         "auth_mode": auth_mode, "credential": {"kind": kind, "path": str(reference_path)}})
                    alternate = "subscription" if mode == "api" else "api"
                    fd = os.open(config / "providers.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    with os.fdopen(fd, "w", encoding="utf8") as stream:
                        json.dump({"version": 1, "default_profile": f"qa-{alternate}", "profiles": profiles}, stream)

                    evidence = config / "evidence"
                    model = models[client]
                    argv = ["run_provider_case.py", "--bin", str(binary), "--config-dir", str(config),
                            "--node", str(root / "no-browser-node"), "--gateway", "http://127.0.0.1:1",
                            "--dashboard", "http://127.0.0.1:1", "--image", "sha256:" + "a" * 64,
                            "--runtime", "docker", "--client", client, "--auth-mode", mode,
                            "--transport", transport, "--client-profile", profile, "--evidence-dir", str(evidence)]

                    def only_diagnostic(command, env, timeout, *, check):
                        self.assertEqual(command, [companion, "--config-dir", config, "--client", client,
                                                   "--auth-mode", mode, "--client-profile", profile,
                                                   "--model", model, "--json"])
                        return run_local(command, env, timeout, check=check)

                    with (patch.object(sys, "argv", argv), patch.dict(os.environ, {"HOME": str(root)}, clear=True),
                          patch.object(recipe, "run_command", side_effect=only_diagnostic) as commands,
                          patch.object(recipe, "browser_command") as browser, redirect_stdout(io.StringIO())):
                        self.assertEqual(recipe.main(), 2)
                    self.assertEqual(commands.call_count, 1)
                    browser.assert_not_called()
                    self.assertEqual(list(evidence.iterdir()), [evidence / "result.json"])
                    report = json.loads((evidence / "result.json").read_text(encoding="utf8"))
                    stage = "browser_preflight" if present else "provider_preflight"
                    code = {"api": "provider.credential_missing", "subscription": "provider.login_required"}[mode]
                    self.assertEqual((report["status"], report["stage"], report["model_requested"]), ("BLOCKED", stage, model))
                    self.assertEqual(report["provider_preflight"], {
                        "code": "selected_reference_metadata_verified" if present else code,
                        "selection": {"client": client_kind, "auth_mode": mode, "profile": profile, "model": model},
                    })


if __name__ == "__main__":
    unittest.main()
